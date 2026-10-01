//! AG077 old-API facts. These tests intentionally preserve the legacy run-only
//! contract; they are not a claim that a nonexistent new API failed to compile.
use evo_core::{Context, Role, fingerprint, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};

fn admin() -> Context {
    Context::new("n", "import-admin", Role::Admin).unwrap()
}

fn host() -> Context {
    Context::new("n", "model-host", Role::Host).unwrap()
}

async fn database() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    store
        .authorize_root_budget(
            &admin(),
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "scope-1".into(),
                allowed_namespaces: vec!["n".into()],
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-1".into(),
                authorization_receipt_digest: hash(b"real-test-authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    (dir, store)
}

async fn put(store: &Store, kind: &str, id: &str, body: &Value) {
    let mut session = store.session().await.unwrap();
    session
        .put(&admin(), kind, id, admin().actor(), body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

fn source(id: &str) -> Value {
    json!({
        "schema_version": "rsia.e16.import_source.v1", "id": id,
        "namespace": "n", "owner_actor": "import-admin", "request_key": format!("request-{id}"),
        "input_digest": hash(id.as_bytes()), "created_at": 1, "updated_at": 1,
        "source_refs": [], "revoke_watermark": 0,
        "payload": {"raw_blob_digest": null, "status": "prepared"}
    })
}

fn reservation(call_id: &str, source_id: &str) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: "scope-1".into(), call_id: call_id.into(),
        dispatch_group_id: format!("group-{call_id}"), stage: BudgetStage::Reflection,
        actual_input_digest: hash(format!("input-{call_id}").as_bytes()),
        request_artifact: Some(BudgetArtifact::from_serializable(
            "rsia.model_request.artifact.v1",
            &json!({"request_id": call_id, "source_closure": [{"id":source_id, "digest":hash(source_id.as_bytes())}], "input":"REQUEST-MARKER"}),
        ).unwrap()),
        max_cost_micros: 20, lease_token: format!("lease-{call_id}"), lease_until: 100, now: 2,
    }
}

fn fence(call: &BudgetCallRecord, now: i64) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    }
}

async fn revoke(store: &Store, kind: &str, id: &str) {
    LifecycleStore::begin_revoke(
        &admin(),
        store,
        TypedObjectRef {
            kind: kind.into(),
            id: id.into(),
        },
        "test revocation",
        5,
    )
    .await
    .unwrap();
}

fn evidence() -> ModelCallSettlementEvidence {
    ModelCallSettlementEvidence {
        provenance: BudgetExecutionProvenance::Fixture,
        actual_model_digest: Some(hash(b"model")),
        transport_artifact: BudgetArtifact::from_serializable(
            "test.transport.v1",
            &json!({"content":"TRANSPORT-MARKER"}),
        )
        .unwrap(),
        usable_response: Some(
            BudgetArtifact::from_serializable(
                "test.response.v1",
                &json!({"content":"RESPONSE-MARKER"}),
            )
            .unwrap(),
        ),
        blocked_response: BudgetArtifact::from_serializable(
            "test.response.v1",
            &json!({"content":"BLOCKED-MARKER"}),
        )
        .unwrap(),
        forced_block_reason: None,
    }
}

fn charge(call_id: &str, amount: i64) -> UsageCharge {
    UsageCharge {
        amount_micros: amount,
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        provider_request_id: format!("provider-{call_id}"),
        usage_record_id: format!("usage-{call_id}"),
        output_digest: hash(b"output"),
    }
}

#[tokio::test]
async fn baseline_artifact_id_on_legacy_run_only_is_not_an_artifact_gate() {
    let (_dir, store) = database().await;
    put(&store, "artifact", "source-a", &source("source-a")).await;
    let request = reservation("legacy-artifact", "source-a");
    let call = store
        .reserve_budget_call_with_sources(&host(), &request, &["source-a".into()])
        .await
        .unwrap();
    revoke(&store, "artifact", "source-a").await;
    let decision = store
        .begin_budget_dispatch(&host(), &fence(&call, 6))
        .await
        .unwrap();
    assert!(decision.new_dispatch);
    assert_eq!(decision.call.state, BudgetCallState::Dispatched);
    let mut session = store.session().await.unwrap();
    let ref_digest =
        fingerprint(&("rsia.budget_call_ref.v1", "n", "scope-1", "legacy-artifact")).unwrap();
    let edges = session
        .dependents(&admin(), "run", "source-a")
        .await
        .unwrap();
    assert!(
        edges
            .iter()
            .any(|(kind, id)| kind == "artifact"
                && id == &format!("budget-ref-{}", &ref_digest[..32]))
    );
    assert!(
        session
            .dependents(&admin(), "artifact", "source-a")
            .await
            .unwrap()
            .is_empty()
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn baseline_deleted_legacy_ref_can_leave_revoked_late_output_usable() {
    let (_dir, store) = database().await;
    put(
        &store,
        "run",
        "run-b",
        &json!({"id":"run-b", "schema_version":"rsia.optimization.source.v1", "body":"run"}),
    )
    .await;
    let call = store
        .reserve_budget_call_with_sources(
            &host(),
            &reservation("legacy-missing", "run-b"),
            &["run-b".into()],
        )
        .await
        .unwrap();
    let dispatched = store
        .begin_budget_dispatch(&host(), &fence(&call, 3))
        .await
        .unwrap()
        .call;
    let ref_digest =
        fingerprint(&("rsia.budget_call_ref.v1", "n", "scope-1", "legacy-missing")).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .delete(
            &admin(),
            "artifact",
            &format!("budget-ref-{}", &ref_digest[..32]),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    revoke(&store, "run", "run-b").await;
    let settlement = store
        .settle_model_budget_call(
            &host(),
            &fence(&dispatched, 6),
            &charge("legacy-missing", 9),
            &evidence(),
        )
        .await
        .unwrap();
    assert!(settlement.response_usable);
    assert!(
        settlement
            .response_artifact
            .body
            .contains("RESPONSE-MARKER")
    );
    assert_eq!(settlement.call.actual_cost_micros, Some(9));
    assert_eq!(settlement.call.dispatch_id, dispatched.dispatch_id);
    let root = store
        .root_budget(&admin(), "scope-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((root.reserved_micros, root.spent_micros), (0, 9));
}

mod e16_mode {
    use super::*;
    use evo_core::Error;
    use evo_storage::budget::{
        REGISTERED_EXECUTION_SETTLEMENT_SCHEMA, RegisteredExecutionProvenance,
        RegisteredExecutionSettlement,
    };
    use evo_storage::typed_budget::{
        E16_BUDGET_REF_SCHEMA, E16_BUDGET_REQUEST_SCHEMA, E16BudgetCallRef, E16BudgetRequest,
        e16_budget_ref_id,
    };
    use serde::Serialize;
    use sqlx::Connection;
    use std::collections::BTreeMap;

    fn unavailable<T: std::fmt::Debug>(result: evo_core::Result<T>) {
        assert!(
            matches!(&result, Err(Error::Conflict(code)) if code=="e16_budget_sources_unavailable"),
            "{result:?}"
        );
    }
    fn mismatch<T: std::fmt::Debug>(result: evo_core::Result<T>) {
        assert!(
            matches!(&result, Err(Error::Conflict(code)) if code=="e16_budget_mode_mismatch"),
            "{result:?}"
        );
    }
    fn invalid<T: std::fmt::Debug>(result: evo_core::Result<T>) {
        assert!(
            matches!(&result, Err(Error::Invalid(code)) if code=="e16_budget_request_invalid"),
            "{result:?}"
        );
    }
    fn keys(ids: &[&str]) -> Vec<TypedObjectRef> {
        ids.iter()
            .map(|id| TypedObjectRef {
                kind: "artifact".into(),
                id: (*id).into(),
            })
            .collect()
    }
    async fn new_request(store: &Store, call_id: &str, ids: &[&str]) -> BudgetCallReservation {
        let refs = store
            .snapshot_e16_budget_sources(&host(), &keys(ids))
            .await
            .unwrap();
        let input = BudgetArtifact::from_serializable(
            "test.actual_input.v1",
            &json!({"primary":ids[0], "content":"REQUEST-MARKER"}),
        )
        .unwrap();
        let artifact = E16BudgetRequest {
            schema_version: E16_BUDGET_REQUEST_SCHEMA.into(),
            namespace: "n".into(),
            billing_scope: "scope-1".into(),
            call_id: call_id.into(),
            object_refs: refs,
            input_artifact: input.clone(),
        }
        .artifact()
        .unwrap();
        BudgetCallReservation {
            actual_input_digest: input.digest,
            request_artifact: Some(artifact),
            ..reservation(call_id, ids[0])
        }
    }
    async fn new_call(store: &Store, call_id: &str, ids: &[&str]) -> BudgetCallRecord {
        store
            .reserve_e16_budget_call(&host(), &new_request(store, call_id, ids).await)
            .await
            .unwrap()
    }
    async fn dispatch(store: &Store, call: &BudgetCallRecord) -> BudgetCallRecord {
        store
            .begin_budget_dispatch(&host(), &fence(call, 3))
            .await
            .unwrap()
            .call
    }
    async fn ref_value(store: &Store, call: &BudgetCallRecord) -> Value {
        let mut session = store.session().await.unwrap();
        let value = session
            .need(
                &admin(),
                "artifact",
                &e16_budget_ref_id("n", "scope-1", &call.call_id).unwrap(),
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        value
    }
    async fn delete_ref(store: &Store, call: &BudgetCallRecord) {
        let mut session = store.session().await.unwrap();
        session
            .delete(
                &admin(),
                "artifact",
                &e16_budget_ref_id("n", "scope-1", &call.call_id).unwrap(),
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
    }
    async fn all_rows(dir: &tempfile::TempDir) -> BTreeMap<String, Vec<String>> {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(dir.path().join("rsia.sqlite3"))
            .read_only(true);
        let mut db = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        let tables:Vec<String>=sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").fetch_all(&mut db).await.unwrap();
        let mut rows = BTreeMap::new();
        for table in tables {
            let quoted = table.replace('"', "\"\"");
            let columns: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT name FROM pragma_table_info('{}') ORDER BY cid",
                table.replace('\'', "''")
            ))
            .fetch_all(&mut db)
            .await
            .unwrap();
            let expressions = columns
                .iter()
                .map(|c| format!("quote(\"{}\")", c.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" || '|' || ");
            let values: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT {expressions} FROM \"{quoted}\" ORDER BY 1"
            ))
            .fetch_all(&mut db)
            .await
            .unwrap();
            rows.insert(table, values);
        }
        rows
    }
    fn assert_three_redacted(call: &BudgetCallRecord) {
        for artifact in [
            &call.request_artifact,
            &call.transport_artifact,
            &call.response_artifact,
        ] {
            let artifact = artifact.as_ref().unwrap();
            assert_eq!(artifact.schema_version, "rsia.redacted.v1");
            assert_eq!(hash(artifact.body.as_bytes()), artifact.digest);
            for marker in [
                "REQUEST-MARKER",
                "TRANSPORT-MARKER",
                "RESPONSE-MARKER",
                "BLOCKED-MARKER",
            ] {
                assert!(!artifact.body.contains(marker), "retained {marker}");
            }
        }
    }
    async fn put_serialized<T: Serialize>(store: &Store, id: &str, body: &T) {
        let mut session = store.session().await.unwrap();
        session
            .put(&admin(), "artifact", id, admin().actor(), body)
            .await
            .unwrap();
        session.commit().await.unwrap();
    }
    struct DuplicateId<'a>(&'a Value);
    impl Serialize for DuplicateId<'_> {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            use serde::ser::SerializeMap;
            let object = self.0.as_object().unwrap();
            let mut map = serializer.serialize_map(Some(object.len() + 1))?;
            for (k, v) in object {
                map.serialize_entry(k, v)?;
            }
            map.serialize_entry("id", &self.0["id"])?;
            map.end()
        }
    }

    #[tokio::test]
    async fn snapshots_keep_original_owner_raw_digest_and_allow_payload_extensions() {
        let (_dir, store) = database().await;
        let mut body = source("source-a");
        body["legal_extension"] = json!({"nested":{"v":1}});
        put(&store, "artifact", "source-a", &body).await;
        let refs = store
            .snapshot_e16_budget_sources(&host(), &keys(&["source-a"]))
            .await
            .unwrap();
        assert_eq!(refs[0].owner_actor, "import-admin");
        assert_ne!(refs[0].owner_actor, host().actor());
        assert_eq!(
            refs[0].storage_body_digest,
            hash(serde_json::to_string(&body).unwrap().as_bytes())
        );
        let request = new_request(&store, "raw-call", &["source-a"]).await;
        assert_ne!(
            request.actual_input_digest,
            request.request_artifact.as_ref().unwrap().digest
        );
        let call = store
            .reserve_e16_budget_call(&host(), &request)
            .await
            .unwrap();
        let reference: E16BudgetCallRef =
            serde_json::from_value(ref_value(&store, &call).await).unwrap();
        assert_eq!(reference.object_refs, refs);
        assert_eq!(reference.request_schema, E16_BUDGET_REQUEST_SCHEMA);
        assert_eq!(reference.schema_version, E16_BUDGET_REF_SCHEMA);
        let mut session = store.session().await.unwrap();
        let legacy = fingerprint(&("rsia.budget_call_ref.v1", "n", "scope-1", "raw-call")).unwrap();
        assert!(
            session
                .get::<Value>(
                    &admin(),
                    "artifact",
                    &format!("budget-ref-{}", &legacy[..32])
                )
                .await
                .unwrap()
                .is_none()
        );
        session.commit().await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_cardinality_canonicalization_header_and_role_bounds() {
        let (dir, store) = database().await;
        let before = all_rows(&dir).await;
        invalid(store.snapshot_e16_budget_sources(&host(), &[]).await);
        invalid(
            store
                .snapshot_e16_budget_sources(&host(), &keys(&["same", "same"]))
                .await,
        );
        invalid(
            store
                .snapshot_e16_budget_sources(
                    &host(),
                    &[TypedObjectRef {
                        kind: "run".into(),
                        id: "r".into(),
                    }],
                )
                .await,
        );
        let huge = (0..204)
            .map(|i| TypedObjectRef {
                kind: "artifact".into(),
                id: format!("over-limit-{i:03}"),
            })
            .collect::<Vec<_>>();
        invalid(
            store
                .snapshot_e16_budget_sources(&host(), &huge[..203])
                .await,
        );
        invalid(store.snapshot_e16_budget_sources(&host(), &huge).await);
        assert_eq!(before, all_rows(&dir).await);
        for i in 0..202 {
            let id = format!("source-{i:03}");
            put(&store, "artifact", &id, &source(&id)).await;
        }
        let ids = (0..202)
            .rev()
            .map(|i| TypedObjectRef {
                kind: "artifact".into(),
                id: format!("source-{i:03}"),
            })
            .collect::<Vec<_>>();
        let refs = store
            .snapshot_e16_budget_sources(&host(), &ids)
            .await
            .unwrap();
        assert_eq!(refs.len(), 202);
        assert!(refs.windows(2).all(|p| p[0] < p[1]));
        let input = BudgetArtifact::from_serializable(
            "test.input.v1",
            &json!({"content":"202 complete materials"}),
        )
        .unwrap();
        let artifact = E16BudgetRequest {
            schema_version: E16_BUDGET_REQUEST_SCHEMA.into(),
            namespace: "n".into(),
            billing_scope: "scope-1".into(),
            call_id: "all-202".into(),
            object_refs: refs.clone(),
            input_artifact: input.clone(),
        }
        .artifact()
        .unwrap();
        let call = store
            .reserve_e16_budget_call(
                &host(),
                &BudgetCallReservation {
                    actual_input_digest: input.digest,
                    request_artifact: Some(artifact),
                    ..reservation("all-202", "source-000")
                },
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_value::<E16BudgetCallRef>(ref_value(&store, &call).await)
                .unwrap()
                .object_refs
                .len(),
            202
        );
        assert!(
            store
                .begin_budget_dispatch(&host(), &fence(&call, 3))
                .await
                .unwrap()
                .new_dispatch
        );
        for role in [Role::Agent, Role::Worker, Role::Evaluator] {
            assert!(matches!(
                store
                    .snapshot_e16_budget_sources(&Context::new("n", "caller", role).unwrap(), &ids)
                    .await,
                Err(Error::Forbidden)
            ));
        }
        for (field, value) in [
            ("id", json!(null)),
            ("namespace", json!("other")),
            ("owner_actor", json!("other-owner")),
            ("schema_version", json!("rsia.e16.import_source.v99")),
        ] {
            let mut bad = source("bad");
            bad[field] = value;
            put(&store, "artifact", "bad", &bad).await;
            unavailable(
                store
                    .snapshot_e16_budget_sources(&host(), &keys(&["bad"]))
                    .await,
            );
        }
        put_serialized(&store, "bad", &DuplicateId(&source("bad"))).await;
        unavailable(
            store
                .snapshot_e16_budget_sources(&host(), &keys(&["bad"]))
                .await,
        );
        unavailable(
            store
                .snapshot_e16_budget_sources(&host(), &keys(&["missing"]))
                .await,
        );
    }

    #[tokio::test]
    async fn snapshot_has_a_per_call_body_bound_without_changing_raw_get() {
        let (dir, store) = database().await;
        put(&store, "artifact", "large", &source("large")).await;
        let mut large = source("large");
        large["extra"] = json!("x".repeat(4 * 1024 * 1024));
        // The existing put cap rejects this shape. Inject only raw source bytes
        // into a real object to check the new reader's independent processing cap.
        let mut db = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("rsia.sqlite3")),
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE objects SET body=? WHERE namespace='n' AND kind='artifact' AND id='large'",
        )
        .bind(serde_json::to_string(&large).unwrap())
        .execute(&mut db)
        .await
        .unwrap();
        drop(db);
        unavailable(
            store
                .snapshot_e16_budget_sources(&host(), &keys(&["large"]))
                .await,
        );
        let mut session = store.session().await.unwrap();
        let raw: Value = session.need(&admin(), "artifact", "large").await.unwrap();
        assert!(raw["extra"].as_str().unwrap().len() >= 4 * 1024 * 1024);
        session.commit().await.unwrap();
    }

    #[tokio::test]
    async fn new_request_rejects_unknown_duplicate_noncanonical_refs_and_nested_hash() {
        let (dir, store) = database().await;
        put(&store, "artifact", "a", &source("a")).await;
        put(&store, "artifact", "b", &source("b")).await;
        let request = new_request(&store, "strict", &["a", "b"]).await;
        let original: Value =
            serde_json::from_str(&request.request_artifact.as_ref().unwrap().body).unwrap();
        let mut variants = Vec::new();
        let mut unknown = original.clone();
        unknown["unknown"] = json!(true);
        variants.push(unknown);
        let mut wrong_ns = original.clone();
        wrong_ns["namespace"] = json!("other");
        variants.push(wrong_ns);
        let mut wrong_call = original.clone();
        wrong_call["call_id"] = json!("different");
        variants.push(wrong_call);
        let mut wrong_scope = original.clone();
        wrong_scope["billing_scope"] = json!("other-root");
        variants.push(wrong_scope);
        let mut wrong_input = original.clone();
        wrong_input["input_artifact"]["digest"] = json!(hash(b"not-input"));
        variants.push(wrong_input);
        let mut wrong_kind = original.clone();
        wrong_kind["object_refs"][0]["kind"] = json!("blob");
        variants.push(wrong_kind);
        let mut unknown_ref = original.clone();
        unknown_ref["object_refs"][0]["unknown"] = json!(true);
        variants.push(unknown_ref);
        let mut duplicate = original.clone();
        duplicate["object_refs"][1] = duplicate["object_refs"][0].clone();
        variants.push(duplicate);
        let mut unsorted = original.clone();
        unsorted["object_refs"].as_array_mut().unwrap().reverse();
        variants.push(unsorted);
        let mut empty = original.clone();
        empty["object_refs"] = json!([]);
        variants.push(empty);
        let before = all_rows(&dir).await;
        for value in variants {
            let artifact =
                BudgetArtifact::from_serializable(E16_BUDGET_REQUEST_SCHEMA, &value).unwrap();
            invalid(
                store
                    .reserve_e16_budget_call(
                        &host(),
                        &BudgetCallReservation {
                            request_artifact: Some(artifact),
                            ..request.clone()
                        },
                    )
                    .await,
            );
            assert_eq!(before, all_rows(&dir).await);
        }
        let artifact = request.request_artifact.as_ref().unwrap();
        let duplicate_body = artifact.body.replacen(
            "{",
            r#"{"schema_version":"rsia.e16.model_budget_request.v1","#,
            1,
        );
        invalid(
            store
                .reserve_e16_budget_call(
                    &host(),
                    &BudgetCallReservation {
                        request_artifact: Some(BudgetArtifact {
                            schema_version: artifact.schema_version.clone(),
                            digest: hash(duplicate_body.as_bytes()),
                            body: duplicate_body,
                        }),
                        ..request.clone()
                    },
                )
                .await,
        );
        let call = store
            .reserve_e16_budget_call(&host(), &request)
            .await
            .unwrap();
        assert_eq!(call.actual_input_digest, request.actual_input_digest);
    }

    #[tokio::test]
    async fn all_legacy_reserve_entrances_refuse_new_mode_and_ignore_no_old_sources() {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let request = new_request(&store, "new", &["s"]).await;
        let before = all_rows(&dir).await;
        mismatch(store.reserve_budget_call(&host(), &request).await);
        mismatch(store.reserve_budget_call_typed(&host(), &request).await);
        mismatch(
            store
                .reserve_budget_call_with_sources(&host(), &request, &["old-run".into()])
                .await,
        );
        mismatch(
            store
                .reserve_budget_call_with_sources_typed(&host(), &request, &["old-run".into()])
                .await,
        );
        assert_eq!(before, all_rows(&dir).await);
        let call = store
            .reserve_e16_budget_call(&host(), &request)
            .await
            .unwrap();
        let legacy_request = reservation("new", "old-run");
        let before = all_rows(&dir).await;
        mismatch(store.reserve_budget_call(&host(), &legacy_request).await);
        mismatch(
            store
                .reserve_budget_call_typed(&host(), &legacy_request)
                .await,
        );
        mismatch(
            store
                .reserve_budget_call_with_sources(&host(), &legacy_request, &["old-run".into()])
                .await,
        );
        mismatch(
            store
                .reserve_budget_call_with_sources_typed(
                    &host(),
                    &legacy_request,
                    &["old-run".into()],
                )
                .await,
        );
        assert_eq!(before, all_rows(&dir).await);
        assert_eq!(call.state, BudgetCallState::Reserved);
    }

    #[tokio::test]
    async fn repeated_reserve_never_rebuilds_a_missing_ref_or_returns_stale_request() {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let request = new_request(&store, "repeat", &["s"]).await;
        let call = store
            .reserve_e16_budget_call(&host(), &request)
            .await
            .unwrap();
        assert_eq!(
            store
                .reserve_e16_budget_call(&host(), &request)
                .await
                .unwrap(),
            call
        );
        delete_ref(&store, &call).await;
        let before = all_rows(&dir).await;
        unavailable(store.reserve_e16_budget_call(&host(), &request).await);
        assert_eq!(before, all_rows(&dir).await);
        unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 3)).await);
        assert_eq!(before, all_rows(&dir).await);
        let released = store
            .release_undispatched_budget_call(&host(), &fence(&call, 4), "explicit-release")
            .await
            .unwrap();
        assert_eq!(released.state, BudgetCallState::Released);
    }

    #[tokio::test]
    async fn nonprimary_counted_material_blocks_reserve_and_all_shared_begin_returns() {
        let (dir, store) = database().await;
        for id in ["primary", "secondary"] {
            put(&store, "artifact", id, &source(id)).await;
        }
        let request = new_request(&store, "late-count", &["primary", "secondary"]).await;
        let call = store
            .reserve_e16_budget_call(&host(), &request)
            .await
            .unwrap();
        let dispatched = dispatch(&store, &call).await;
        assert!(
            !store
                .begin_budget_dispatch(&host(), &fence(&call, 4))
                .await
                .unwrap()
                .new_dispatch
        );
        revoke(&store, "artifact", "secondary").await;
        let before = all_rows(&dir).await;
        unavailable(store.reserve_e16_budget_call(&host(), &request).await);
        unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 6)).await);
        unavailable(
            store
                .begin_budget_dispatch_typed(&host(), &fence(&call, 6))
                .await,
        );
        let mut session = store.session().await.unwrap();
        unavailable(
            session
                .begin_budget_dispatch(&host(), &fence(&call, 6))
                .await,
        );
        drop(session);
        assert_eq!(before, all_rows(&dir).await);
        assert_eq!(
            store
                .budget_call(&host(), "scope-1", "late-count")
                .await
                .unwrap()
                .unwrap()
                .dispatch_id,
            dispatched.dispatch_id
        );
        let later = new_request(&store, "before-reserve", &["primary", "secondary"]).await;
        unavailable(store.reserve_e16_budget_call(&host(), &later).await);
        assert_eq!(before, all_rows(&dir).await);
    }

    #[tokio::test]
    async fn reserved_begin_refusal_preserves_its_reservation_and_dispatch_absence() {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let call = new_call(&store, "reserved", &["s"]).await;
        revoke(&store, "artifact", "s").await;
        let before = all_rows(&dir).await;
        unavailable(
            store
                .begin_budget_dispatch_typed(&host(), &fence(&call, 6))
                .await,
        );
        assert_eq!(before, all_rows(&dir).await);
        let persisted = store
            .budget_call(&host(), "scope-1", "reserved")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted, call);
        let root = store
            .root_budget(&admin(), "scope-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!((root.reserved_micros, root.spent_micros), (20, 0));
    }

    #[tokio::test]
    async fn first_late_settlement_books_real_cost_and_persists_three_redactions() {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let call = new_call(&store, "late", &["s"]).await;
        let dispatched = dispatch(&store, &call).await;
        revoke(&store, "artifact", "s").await;
        let result = store
            .settle_model_budget_call(
                &host(),
                &fence(&dispatched, 6),
                &charge("late", 9),
                &evidence(),
            )
            .await
            .unwrap();
        assert!(!result.response_usable);
        assert_eq!(
            result.response_block_reason.as_deref(),
            Some("e16_source_unavailable")
        );
        assert_three_redacted(&result.call);
        assert_eq!(
            result.response_artifact,
            result.call.response_artifact.clone().unwrap()
        );
        assert_eq!(result.call.actual_cost_micros, Some(9));
        assert_eq!(result.call.dispatch_id, dispatched.dispatch_id);
        assert!(!result.call.execution_closed);
        assert_eq!(
            result
                .call
                .request_artifact
                .as_ref()
                .map(
                    |a| serde_json::from_str::<Value>(&a.body).unwrap()["original_schema"].clone()
                ),
            Some(json!(E16_BUDGET_REQUEST_SCHEMA))
        );
        let root = store
            .root_budget(&admin(), "scope-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!((root.reserved_micros, root.spent_micros), (0, 9));
        let before = all_rows(&dir).await;
        unavailable(
            store
                .settle_model_budget_call(
                    &host(),
                    &fence(&dispatched, 7),
                    &charge("late", 9),
                    &evidence(),
                )
                .await,
        );
        unavailable(
            store
                .consume_e16_budget_response(&host(), "scope-1", "late")
                .await,
        );
        assert_eq!(before, all_rows(&dir).await);
        let wrong = store
            .settle_model_budget_call(
                &host(),
                &fence(&dispatched, 7),
                &charge("late", 10),
                &evidence(),
            )
            .await;
        assert!(
            matches!(wrong,Err(Error::Conflict(code)) if code=="finalized_call_reused_with_different_charge")
        );
        store
            .close_budget_call_execution(
                &host(),
                "scope-1",
                "late",
                dispatched.dispatch_id.as_deref().unwrap(),
                "recorded-close",
                8,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .root_budget(&admin(), "scope-1")
                .await
                .unwrap()
                .unwrap()
                .spent_micros,
            9
        );
    }

    #[tokio::test]
    async fn live_repeat_settlement_and_consumer_return_the_actual_stored_response_once() {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let call = dispatch(&store, &new_call(&store, "live", &["s"]).await).await;
        let result = store
            .settle_model_budget_call(&host(), &fence(&call, 4), &charge("live", 9), &evidence())
            .await
            .unwrap();
        assert!(result.response_usable);
        let before = all_rows(&dir).await;
        assert_eq!(
            store
                .settle_model_budget_call(
                    &host(),
                    &fence(&call, 5),
                    &charge("live", 9),
                    &evidence()
                )
                .await
                .unwrap(),
            result
        );
        assert_eq!(
            store
                .consume_e16_budget_response(&host(), "scope-1", "live")
                .await
                .unwrap(),
            result.response_artifact
        );
        assert_eq!(before, all_rows(&dir).await);
        for role in [Role::Agent, Role::Evaluator, Role::Worker] {
            assert!(matches!(
                store
                    .consume_e16_budget_response(
                        &Context::new("n", "caller", role).unwrap(),
                        "scope-1",
                        "live"
                    )
                    .await,
                Err(Error::Forbidden)
            ));
        }
        let mut changed = source("s");
        changed["extension"] = json!("changed bytes");
        put(&store, "artifact", "s", &changed).await;
        let before = all_rows(&dir).await;
        unavailable(
            store
                .consume_e16_budget_response(&host(), "scope-1", "live")
                .await,
        );
        unavailable(
            store
                .settle_model_budget_call(
                    &host(),
                    &fence(&call, 6),
                    &charge("live", 9),
                    &evidence(),
                )
                .await,
        );
        unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 6)).await);
        assert_eq!(before, all_rows(&dir).await);
    }

    #[tokio::test]
    async fn bad_persisted_refs_are_known_unavailable_but_first_charge_is_not_lost() {
        for field in [
            "missing",
            "id",
            "schema_version",
            "namespace",
            "billing_scope",
            "call_id",
            "request_schema",
            "request_digest",
            "actual_input_digest",
            "duplicate_sources",
            "unknown",
            "duplicate_key",
        ] {
            let (_dir, store) = database().await;
            put(&store, "artifact", "s", &source("s")).await;
            let call = dispatch(&store, &new_call(&store, "bad-ref", &["s"]).await).await;
            let mut value = ref_value(&store, &call).await;
            let id = e16_budget_ref_id("n", "scope-1", "bad-ref").unwrap();
            match field {
                // A non-null wrong id is rejected by the existing SQLite CHECK;
                // null is persisted malformed header shape, not a product error.
                "id" => {
                    value[field] = json!(null);
                    put(&store, "artifact", &id, &value).await;
                }
                "missing" => delete_ref(&store, &call).await,
                "duplicate_sources" => {
                    let first = value["object_refs"][0].clone();
                    value["object_refs"].as_array_mut().unwrap().push(first);
                    put(&store, "artifact", &id, &value).await;
                }
                "unknown" => {
                    value["unknown"] = json!(true);
                    put(&store, "artifact", &id, &value).await;
                }
                "duplicate_key" => put_serialized(&store, &id, &DuplicateId(&value)).await,
                "request_digest" | "actual_input_digest" => {
                    value[field] = json!(hash(b"wrong"));
                    put(&store, "artifact", &id, &value).await;
                }
                other => {
                    value[other] = json!("wrong");
                    put(&store, "artifact", &id, &value).await;
                }
            }
            unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 4)).await);
            let result = store
                .settle_model_budget_call(
                    &host(),
                    &fence(&call, 4),
                    &charge("bad-ref", 9),
                    &evidence(),
                )
                .await
                .unwrap();
            assert!(!result.response_usable, "{field}");
            assert_three_redacted(&result.call);
            assert_eq!(result.call.actual_cost_micros, Some(9), "{field}");
            unavailable(
                store
                    .settle_model_budget_call(
                        &host(),
                        &fence(&call, 5),
                        &charge("bad-ref", 9),
                        &evidence(),
                    )
                    .await,
            );
        }
    }

    #[tokio::test]
    async fn higher_priority_forced_and_overrun_reasons_still_redact_all_content() {
        for forced in [false, true] {
            let (_dir, store) = database().await;
            put(&store, "artifact", "s", &source("s")).await;
            let call = dispatch(&store, &new_call(&store, "priority", &["s"]).await).await;
            delete_ref(&store, &call).await;
            let mut facts = evidence();
            if forced {
                facts.forced_block_reason = Some("operator_block".into());
            }
            let result = store
                .settle_model_budget_call(
                    &host(),
                    &fence(&call, 4),
                    &charge("priority", 21),
                    &facts,
                )
                .await
                .unwrap();
            assert_eq!(
                result.response_block_reason.as_deref(),
                Some(if forced {
                    "operator_block"
                } else {
                    "cost_overrun"
                })
            );
            assert_three_redacted(&result.call);
            assert_eq!(
                store
                    .root_budget(&admin(), "scope-1")
                    .await
                    .unwrap()
                    .unwrap()
                    .spent_micros,
                21
            );
        }
    }

    #[tokio::test]
    async fn invalid_charge_or_evidence_does_not_manufacture_a_settlement() {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let call = dispatch(&store, &new_call(&store, "invalid-facts", &["s"]).await).await;
        delete_ref(&store, &call).await;
        let before = all_rows(&dir).await;
        let mut bad = charge("invalid-facts", 9);
        bad.amount_micros = -1;
        assert!(
            store
                .settle_model_budget_call(&host(), &fence(&call, 4), &bad, &evidence())
                .await
                .is_err()
        );
        let mut facts = evidence();
        facts.transport_artifact.digest = hash(b"wrong");
        assert!(
            store
                .settle_model_budget_call(
                    &host(),
                    &fence(&call, 4),
                    &charge("invalid-facts", 9),
                    &facts
                )
                .await
                .is_err()
        );
        assert_eq!(before, all_rows(&dir).await);
        let reconciled = store
            .reconcile_budget_call_cost(
                &admin(),
                "scope-1",
                "invalid-facts",
                &charge("invalid-facts", 9),
                5,
            )
            .await
            .unwrap();
        assert_eq!(reconciled.actual_cost_micros, Some(9));
        assert_eq!(reconciled.dispatch_id, call.dispatch_id);
    }

    #[tokio::test]
    async fn registered_execution_cannot_settle_or_close_e16_in_either_development_stage() {
        for stage in [
            BudgetStage::DevelopmentExecution,
            BudgetStage::DevelopmentScoring,
        ] {
            let (dir, store) = database().await;
            put(&store, "artifact", "s", &source("s")).await;
            let mut request = new_request(&store, "registered", &["s"]).await;
            request.stage = stage;
            let call = dispatch(
                &store,
                &store
                    .reserve_e16_budget_call(&host(), &request)
                    .await
                    .unwrap(),
            )
            .await;
            let zero = charge("registered", 0);
            let settlement = RegisteredExecutionSettlement {
                schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA.into(),
                provenance: RegisteredExecutionProvenance::RegisteredPureFunction,
                call_id: call.call_id.clone(),
                dispatch_id: call.dispatch_id.clone().unwrap(),
                request_digest: call.actual_input_digest.clone(),
                output_digest: zero.output_digest.clone(),
                target_id: "target".into(),
                target_digest: hash(b"target"),
                runner_digest: hash(b"runner"),
            };
            let before = all_rows(&dir).await;
            mismatch(
                store
                    .settle_registered_execution_call(&host(), &fence(&call, 4), &zero, &settlement)
                    .await,
            );
            assert_eq!(before, all_rows(&dir).await);
            let reconciled = store
                .reconcile_budget_call_cost(&admin(), "scope-1", "registered", &zero, 5)
                .await
                .unwrap();
            assert!(!reconciled.execution_closed);
            store
                .close_budget_call_execution(
                    &host(),
                    "scope-1",
                    "registered",
                    call.dispatch_id.as_deref().unwrap(),
                    "cost-reconciled",
                    6,
                )
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn independent_ref_domain_detects_a_low_level_legacy_request_replacement() {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let call = dispatch(&store, &new_call(&store, "mode-switch", &["s"]).await).await;
        let old = reservation("mode-switch", "s").request_artifact.unwrap();
        let mut db = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("rsia.sqlite3")),
        )
        .await
        .unwrap();
        // Fault injection changes only an artifact triple of a real call, never
        // its monetary, dispatch, fence or close columns.
        sqlx::query("UPDATE root_budget_calls SET request_artifact_schema=?,request_artifact_digest=?,request_artifact_body=? WHERE billing_scope=? AND call_id=?")
            .bind(old.schema_version).bind(old.digest).bind(old.body).bind("scope-1").bind("mode-switch").execute(&mut db).await.unwrap();
        drop(db);
        let before = all_rows(&dir).await;
        mismatch(store.begin_budget_dispatch(&host(), &fence(&call, 4)).await);
        mismatch(
            store
                .settle_model_budget_call(
                    &host(),
                    &fence(&call, 4),
                    &charge("mode-switch", 9),
                    &evidence(),
                )
                .await,
        );
        mismatch(
            store
                .consume_e16_budget_response(&host(), "scope-1", "mode-switch")
                .await,
        );
        mismatch(
            store
                .reserve_budget_call(&host(), &reservation("mode-switch", "s"))
                .await,
        );
        assert_eq!(before, all_rows(&dir).await);
        let result = store
            .reconcile_budget_call_cost(
                &admin(),
                "scope-1",
                "mode-switch",
                &charge("mode-switch", 9),
                5,
            )
            .await
            .unwrap();
        assert_eq!(result.actual_cost_micros, Some(9));
    }

    #[tokio::test]
    async fn upstream_cycles_missing_typed_nodes_and_tombstone_shape_fail_closed() {
        let (dir, store) = database().await;
        for id in ["s", "up"] {
            put(&store, "artifact", id, &source(id)).await;
        }
        let mut session = store.session().await.unwrap();
        session
            .put_edge(&admin(), "artifact", "s", "artifact", "up")
            .await
            .unwrap();
        session
            .put_edge(&admin(), "artifact", "up", "artifact", "s")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let request = new_request(&store, "cycle", &["s"]).await;
        let call = store
            .reserve_e16_budget_call(&host(), &request)
            .await
            .unwrap();
        let mut session = store.session().await.unwrap();
        session
            .put_edge(&admin(), "artifact", "up", "run", "missing-run")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let before = all_rows(&dir).await;
        unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 3)).await);
        assert_eq!(before, all_rows(&dir).await);
        put(&store, "run", "missing-run", &json!({"id":"missing-run"})).await;
        assert!(
            store
                .begin_budget_dispatch(&host(), &fence(&call, 3))
                .await
                .unwrap()
                .new_dispatch
        );
        put(
            &store,
            "tombstone",
            "missing-run",
            &json!({"schema_version":"unknown","source_kind":"artifact"}),
        )
        .await;
        let before = all_rows(&dir).await;
        unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 4)).await);
        assert_eq!(before, all_rows(&dir).await);
        let mut session = store.session().await.unwrap();
        session
            .delete(&admin(), "tombstone", "missing-run")
            .await
            .unwrap();
        session.delete(&admin(), "artifact", "up").await.unwrap();
        session.commit().await.unwrap();
        unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 4)).await);
    }

    async fn source_with_run_upstream(store: &Store) {
        put(store, "artifact", "s", &source("s")).await;
        put(
            store,
            "run",
            "upstream-run",
            &json!({"id":"upstream-run","content":"UPSTREAM-RUN-MARKER"}),
        )
        .await;
        let mut session = store.session().await.unwrap();
        session
            .put_edge(&admin(), "artifact", "s", "run", "upstream-run")
            .await
            .unwrap();
        session.commit().await.unwrap();
    }

    async fn redact_upstream_run_without_tombstone(store: &Store) {
        put(store,"run","upstream-run",&json!({"schema_version":"rsia.redacted.v1","id":"upstream-run","original_schema":"test.run.v1","digest":hash(b"UPSTREAM-RUN-MARKER"),"reason":"test redaction"})).await;
        let mut session = store.session().await.unwrap();
        assert!(
            session
                .get::<Value>(&admin(), "tombstone", "upstream-run")
                .await
                .unwrap()
                .is_none()
        );
        session.commit().await.unwrap();
    }

    #[tokio::test]
    async fn run_body_redaction_without_tombstone_blocks_repeat_reserve_and_begin() {
        let (dir, store) = database().await;
        source_with_run_upstream(&store).await;
        let request = new_request(&store, "redacted-run-begin", &["s"]).await;
        let call = store
            .reserve_e16_budget_call(&host(), &request)
            .await
            .unwrap();
        let dispatched = dispatch(&store, &call).await;
        redact_upstream_run_without_tombstone(&store).await;
        let before = all_rows(&dir).await;
        unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 4)).await);
        unavailable(store.reserve_e16_budget_call(&host(), &request).await);
        assert_eq!(before, all_rows(&dir).await);
        assert_eq!(
            store
                .budget_call(&host(), "scope-1", "redacted-run-begin")
                .await
                .unwrap()
                .unwrap(),
            dispatched
        );
    }

    #[tokio::test]
    async fn run_body_redaction_without_tombstone_books_late_cost_and_clears_three_bodies() {
        let (_dir, store) = database().await;
        source_with_run_upstream(&store).await;
        let call = dispatch(&store, &new_call(&store, "redacted-run-late", &["s"]).await).await;
        redact_upstream_run_without_tombstone(&store).await;
        let result = store
            .settle_model_budget_call(
                &host(),
                &fence(&call, 4),
                &charge("redacted-run-late", 9),
                &evidence(),
            )
            .await
            .unwrap();
        assert!(!result.response_usable);
        assert_eq!(
            result.response_block_reason.as_deref(),
            Some("e16_source_unavailable")
        );
        assert_three_redacted(&result.call);
        assert_eq!(
            result.response_artifact,
            result.call.response_artifact.clone().unwrap()
        );
        assert_eq!(result.call.actual_cost_micros, Some(9));
        assert_eq!(result.call.dispatch_id, call.dispatch_id);
        let root = store
            .root_budget(&admin(), "scope-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!((root.reserved_micros, root.spent_micros), (0, 9));
    }

    #[tokio::test]
    async fn run_body_redaction_without_tombstone_blocks_finalized_consume_and_repeat_settle() {
        let (dir, store) = database().await;
        source_with_run_upstream(&store).await;
        let call = dispatch(
            &store,
            &new_call(&store, "redacted-run-final", &["s"]).await,
        )
        .await;
        let live = store
            .settle_model_budget_call(
                &host(),
                &fence(&call, 4),
                &charge("redacted-run-final", 9),
                &evidence(),
            )
            .await
            .unwrap();
        assert!(live.response_usable);
        assert_eq!(
            store
                .consume_e16_budget_response(&host(), "scope-1", "redacted-run-final")
                .await
                .unwrap(),
            live.response_artifact
        );
        redact_upstream_run_without_tombstone(&store).await;
        let before = all_rows(&dir).await;
        unavailable(
            store
                .consume_e16_budget_response(&host(), "scope-1", "redacted-run-final")
                .await,
        );
        unavailable(
            store
                .settle_model_budget_call(
                    &host(),
                    &fence(&call, 5),
                    &charge("redacted-run-final", 9),
                    &evidence(),
                )
                .await,
        );
        unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 5)).await);
        assert_eq!(before, all_rows(&dir).await);
        assert_eq!(
            store
                .root_budget(&admin(), "scope-1")
                .await
                .unwrap()
                .unwrap()
                .spent_micros,
            9
        );
    }

    #[tokio::test]
    async fn legacy_upstream_api_keeps_run_false_and_artifact_true_for_redacted_bodies() {
        let (_dir, store) = database().await;
        source_with_run_upstream(&store).await;
        redact_upstream_run_without_tombstone(&store).await;
        put(
            &store,
            "artifact",
            "redacted-artifact",
            &json!({"id":"redacted-artifact","schema_version":"rsia.redacted.v1"}),
        )
        .await;
        let mut session = store.session().await.unwrap();
        let nodes = session
            .upstream_closure(
                &host(),
                &[("artifact", "s"), ("artifact", "redacted-artifact")],
                10_000,
            )
            .await
            .unwrap();
        assert_eq!(
            nodes
                .iter()
                .map(|n| (n.kind.as_str(), n.id.as_str(), n.redacted))
                .collect::<Vec<_>>(),
            vec![
                ("artifact", "redacted-artifact", true),
                ("artifact", "s", false),
                ("run", "upstream-run", false)
            ]
        );
        assert!(nodes.iter().all(|n| n.tombstone.is_none()));
        session.commit().await.unwrap();
    }

    #[tokio::test]
    async fn bounded_walk_counts_all_kinds_and_refuses_10001_without_truncation() {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let mut session = store.session().await.unwrap();
        let mut prior = "s".to_owned();
        let mut prior_kind = "artifact";
        for i in 1..10_000 {
            let id = format!("opaque-{i:05}");
            session
                .put_edge(&admin(), prior_kind, &prior, "blob", &id)
                .await
                .unwrap();
            prior = id;
            prior_kind = "blob";
        }
        session.commit().await.unwrap();
        let request = new_request(&store, "bounded", &["s"]).await;
        let call = store
            .reserve_e16_budget_call(&host(), &request)
            .await
            .unwrap();
        let mut session = store.session().await.unwrap();
        session
            .put_edge(&admin(), "blob", &prior, "blob", "node-10001")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let before = all_rows(&dir).await;
        unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 3)).await);
        unavailable(store.reserve_e16_budget_call(&host(), &request).await);
        assert_eq!(before, all_rows(&dir).await);
    }

    #[tokio::test]
    async fn same_id_other_kind_tombstone_does_not_revoke_an_artifact() {
        let (_dir, store) = database().await;
        put(&store, "artifact", "shared", &source("shared")).await;
        put(&store, "run", "shared", &json!({"id":"shared"})).await;
        revoke(&store, "run", "shared").await;
        let call = new_call(&store, "kind-isolation", &["shared"]).await;
        assert!(
            store
                .begin_budget_dispatch(&host(), &fence(&call, 6))
                .await
                .unwrap()
                .new_dispatch
        );
    }

    #[tokio::test]
    async fn source_sql_fault_is_an_outer_error_and_admin_accounting_remains_reachable() {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let call = dispatch(&store, &new_call(&store, "sql-fault", &["s"]).await).await;
        let mut db = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("rsia.sqlite3")),
        )
        .await
        .unwrap();
        sqlx::query("ALTER TABLE objects RENAME TO saved_objects")
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::query("CREATE VIEW objects AS SELECT namespace,kind,id,owner,CASE WHEN id='s' THEN json_extract('not-json','$') ELSE body END AS body,revision FROM saved_objects").execute(&mut db).await.unwrap();
        drop(db);
        assert!(matches!(
            store
                .settle_model_budget_call(
                    &host(),
                    &fence(&call, 4),
                    &charge("sql-fault", 9),
                    &evidence()
                )
                .await,
            Err(Error::Internal)
        ));
        let persisted = store
            .budget_call(&host(), "scope-1", "sql-fault")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted, call);
        let root = store
            .root_budget(&admin(), "scope-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!((root.reserved_micros, root.spent_micros), (20, 0));
        let mut db = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("rsia.sqlite3")),
        )
        .await
        .unwrap();
        sqlx::query("DROP VIEW objects")
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE saved_objects RENAME TO objects")
            .execute(&mut db)
            .await
            .unwrap();
        drop(db);
        let settled = store
            .reconcile_budget_call_cost(
                &admin(),
                "scope-1",
                "sql-fault",
                &charge("sql-fault", 9),
                5,
            )
            .await
            .unwrap();
        assert_eq!(settled.dispatch_id, call.dispatch_id);
        assert_eq!(settled.actual_cost_micros, Some(9));
        store
            .close_budget_call_execution(
                &host(),
                "scope-1",
                "sql-fault",
                call.dispatch_id.as_deref().unwrap(),
                "actual-cost-reconciled",
                6,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn nested_input_and_outer_envelope_have_separate_json_byte_bounds() {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let original = new_request(&store, "bounds", &["s"]).await;
        let request: E16BudgetRequest =
            serde_json::from_str(&original.request_artifact.as_ref().unwrap().body).unwrap();
        let mut malformed = request.clone();
        malformed.input_artifact.body = "not-json".into();
        malformed.input_artifact.digest = hash(b"not-json");
        invalid(malformed.artifact());
        let mut oversized = request.clone();
        oversized.input_artifact.body =
            serde_json::to_string(&"x".repeat(4 * 1024 * 1024)).unwrap();
        oversized.input_artifact.digest = hash(oversized.input_artifact.body.as_bytes());
        invalid(oversized.artifact());
        // The input alone fits, but escaping its raw JSON again does not fit
        // the independent outer envelope cap.
        let mut outer = request;
        outer.input_artifact =
            BudgetArtifact::from_serializable("test.input.v1", &"\"".repeat(1_100_000)).unwrap();
        assert!(outer.input_artifact.body.len() < 4 * 1024 * 1024);
        assert!(serde_json::to_string(&outer).unwrap().len() > 4 * 1024 * 1024);
        invalid(outer.artifact());
        let body = serde_json::to_string(&outer).unwrap();
        let before = all_rows(&dir).await;
        invalid(
            store
                .reserve_e16_budget_call(
                    &host(),
                    &BudgetCallReservation {
                        actual_input_digest: outer.input_artifact.digest,
                        request_artifact: Some(BudgetArtifact {
                            schema_version: E16_BUDGET_REQUEST_SCHEMA.into(),
                            digest: hash(body.as_bytes()),
                            body,
                        }),
                        ..original
                    },
                )
                .await,
        );
        assert_eq!(before, all_rows(&dir).await);
    }

    #[tokio::test]
    async fn shared_billing_scope_cannot_borrow_same_id_sources_from_another_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
        store
            .authorize_root_budget(
                &admin(),
                &RootBudgetAuthorization {
                    allowed_namespaces: vec!["n".into(), "other".into()],
                    root_budget_id: "root-1".into(),
                    billing_scope: "scope-1".into(),
                    currency: "USD".into(),
                    pricing_version: "price-v1".into(),
                    payment_subject: "payer-1".into(),
                    authorization_receipt_digest: hash(b"real-test-authorization"),
                    per_call_cap_micros: 100,
                    total_limit_micros: 1000,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
        put(&store, "artifact", "shared", &source("shared")).await;
        let other_admin = Context::new("other", "import-admin", Role::Admin).unwrap();
        let other_host = Context::new("other", "model-host", Role::Host).unwrap();
        unavailable(
            store
                .snapshot_e16_budget_sources(&other_host, &keys(&["shared"]))
                .await,
        );
        let nrefs = store
            .snapshot_e16_budget_sources(&host(), &keys(&["shared"]))
            .await
            .unwrap();
        let input = BudgetArtifact::from_serializable(
            "test.input.v1",
            &json!({"content":"REQUEST-MARKER"}),
        )
        .unwrap();
        let cross = E16BudgetRequest {
            schema_version: E16_BUDGET_REQUEST_SCHEMA.into(),
            namespace: "other".into(),
            billing_scope: "scope-1".into(),
            call_id: "cross".into(),
            object_refs: nrefs.clone(),
            input_artifact: input.clone(),
        };
        invalid(cross.artifact());
        let body = serde_json::to_string(&cross).unwrap();
        let before = all_rows(&dir).await;
        invalid(
            store
                .reserve_e16_budget_call(
                    &other_host,
                    &BudgetCallReservation {
                        actual_input_digest: input.digest.clone(),
                        request_artifact: Some(BudgetArtifact {
                            schema_version: E16_BUDGET_REQUEST_SCHEMA.into(),
                            digest: hash(body.as_bytes()),
                            body,
                        }),
                        ..reservation("cross", "shared")
                    },
                )
                .await,
        );
        assert_eq!(before, all_rows(&dir).await);
        let mut other_source = source("shared");
        other_source["namespace"] = json!("other");
        let mut s = store.session().await.unwrap();
        s.put(
            &other_admin,
            "artifact",
            "shared",
            other_admin.actor(),
            &other_source,
        )
        .await
        .unwrap();
        s.commit().await.unwrap();
        let refs = store
            .snapshot_e16_budget_sources(&other_host, &keys(&["shared"]))
            .await
            .unwrap();
        assert_ne!(refs[0].storage_body_digest, nrefs[0].storage_body_digest);
        let artifact = E16BudgetRequest {
            object_refs: refs,
            ..cross
        }
        .artifact()
        .unwrap();
        let other = store
            .reserve_e16_budget_call(
                &other_host,
                &BudgetCallReservation {
                    actual_input_digest: input.digest,
                    request_artifact: Some(artifact),
                    ..reservation("cross", "shared")
                },
            )
            .await
            .unwrap();
        let ncall = new_call(&store, "n-call", &["shared"]).await;
        revoke(&store, "artifact", "shared").await;
        unavailable(
            store
                .begin_budget_dispatch(&host(), &fence(&ncall, 6))
                .await,
        );
        assert!(
            store
                .begin_budget_dispatch(&other_host, &fence(&other, 6))
                .await
                .unwrap()
                .new_dispatch
        );
    }

    #[tokio::test]
    async fn malformed_persisted_new_request_is_known_unavailable_without_losing_real_first_charge()
    {
        let (dir, store) = database().await;
        put(&store, "artifact", "s", &source("s")).await;
        let call = dispatch(&store, &new_call(&store, "bad-request", &["s"]).await).await;
        let mut body: Value =
            serde_json::from_str(&call.request_artifact.as_ref().unwrap().body).unwrap();
        body["unknown"] = json!(true);
        let body = serde_json::to_string(&body).unwrap();
        let mut db = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("rsia.sqlite3")),
        )
        .await
        .unwrap();
        // Only a persisted artifact's bytes and matching raw hash change. The
        // call, dispatch, reservation and all money remain real API outputs.
        sqlx::query("UPDATE root_budget_calls SET request_artifact_body=?,request_artifact_digest=? WHERE billing_scope='scope-1' AND call_id='bad-request'").bind(&body).bind(hash(body.as_bytes())).execute(&mut db).await.unwrap();
        drop(db);
        let before = all_rows(&dir).await;
        unavailable(store.begin_budget_dispatch(&host(), &fence(&call, 4)).await);
        assert_eq!(before, all_rows(&dir).await);
        let result = store
            .settle_model_budget_call(
                &host(),
                &fence(&call, 4),
                &charge("bad-request", 9),
                &evidence(),
            )
            .await
            .unwrap();
        assert!(!result.response_usable);
        assert_three_redacted(&result.call);
        assert_eq!(result.call.actual_cost_micros, Some(9));
        assert_eq!(result.call.dispatch_id, call.dispatch_id);
        assert_eq!(
            store
                .root_budget(&admin(), "scope-1")
                .await
                .unwrap()
                .unwrap()
                .spent_micros,
            9
        );
    }

    // AG077-R4: preserve the legacy run-body read boundary under a SQL view fault.
    async fn inject_run_json_read_fault(dir: &tempfile::TempDir) {
        let mut db = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("rsia.sqlite3")),
        )
        .await
        .unwrap();
        sqlx::query("ALTER TABLE objects RENAME TO saved_objects")
            .execute(&mut db)
            .await
            .unwrap();
        // Only the selected upstream run's read view is malformed. All original
        // stored rows remain valid; no budget, fee, lease or dispatch row changes.
        sqlx::query("CREATE VIEW objects AS SELECT namespace,kind,id,owner,CASE WHEN kind='run' AND id='upstream-run' THEN 'not-json' ELSE body END AS body,revision FROM saved_objects").execute(&mut db).await.unwrap();
    }

    async fn remove_run_json_read_fault(dir: &tempfile::TempDir) {
        let mut db = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("rsia.sqlite3")),
        )
        .await
        .unwrap();
        sqlx::query("DROP VIEW objects")
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE saved_objects RENAME TO objects")
            .execute(&mut db)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn legacy_upstream_api_does_not_parse_a_run_body_read_fault() {
        let (dir, store) = database().await;
        source_with_run_upstream(&store).await;
        inject_run_json_read_fault(&dir).await;
        let mut session = store.session().await.unwrap();
        let result = session
            .upstream_closure(&host(), &[("artifact", "s")], 10_000)
            .await;
        assert!(
            result.is_ok(),
            "legacy run-body parsing changed: {result:?}"
        );
        let nodes = result.unwrap();
        assert_eq!(
            nodes
                .iter()
                .map(|n| (n.kind.as_str(), n.id.as_str(), n.redacted))
                .collect::<Vec<_>>(),
            vec![("artifact", "s", false), ("run", "upstream-run", false)]
        );
        assert!(nodes.iter().all(|n| n.tombstone.is_none()));
        session.commit().await.unwrap();
        remove_run_json_read_fault(&dir).await;
    }

    #[tokio::test]
    async fn new_mode_upstream_run_json_sql_fault_rolls_back_until_admin_reconciliation() {
        let (dir, store) = database().await;
        source_with_run_upstream(&store).await;
        let request = new_request(&store, "run-json-fault", &["s"]).await;
        let reserved = store
            .reserve_e16_budget_call(&host(), &request)
            .await
            .unwrap();
        let call = dispatch(&store, &reserved).await;
        inject_run_json_read_fault(&dir).await;
        assert!(matches!(
            store.begin_budget_dispatch(&host(), &fence(&call, 4)).await,
            Err(Error::Internal)
        ));
        assert!(matches!(
            store.reserve_e16_budget_call(&host(), &request).await,
            Err(Error::Internal)
        ));
        assert!(matches!(
            store
                .consume_e16_budget_response(&host(), "scope-1", "run-json-fault")
                .await,
            Err(Error::Internal)
        ));
        assert!(matches!(
            store
                .settle_model_budget_call(
                    &host(),
                    &fence(&call, 4),
                    &charge("run-json-fault", 9),
                    &evidence()
                )
                .await,
            Err(Error::Internal)
        ));
        assert_eq!(
            store
                .budget_call(&host(), "scope-1", "run-json-fault")
                .await
                .unwrap()
                .unwrap(),
            call
        );
        let root = store
            .root_budget(&admin(), "scope-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!((root.reserved_micros, root.spent_micros), (20, 0));
        remove_run_json_read_fault(&dir).await;
        let reconciled = store
            .reconcile_budget_call_cost(
                &admin(),
                "scope-1",
                "run-json-fault",
                &charge("run-json-fault", 9),
                5,
            )
            .await
            .unwrap();
        assert_eq!(reconciled.actual_cost_micros, Some(9));
        assert_eq!(reconciled.dispatch_id, call.dispatch_id);
        let closed = store
            .close_budget_call_execution(
                &host(),
                "scope-1",
                "run-json-fault",
                call.dispatch_id.as_deref().unwrap(),
                "fault removed and receipt reconciled",
                6,
            )
            .await
            .unwrap();
        assert!(closed.execution_closed);
        let root = store
            .root_budget(&admin(), "scope-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!((root.reserved_micros, root.spent_micros), (0, 9));
    }

    #[tokio::test]
    async fn legacy_upstream_api_keeps_artifact_json_sql_errors_outer() {
        let (dir, store) = database().await;
        source_with_run_upstream(&store).await;
        let mut db = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("rsia.sqlite3")),
        )
        .await
        .unwrap();
        sqlx::query("ALTER TABLE objects RENAME TO saved_objects")
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::query("CREATE VIEW objects AS SELECT namespace,kind,id,owner,CASE WHEN kind='artifact' AND id='s' THEN 'not-json' ELSE body END AS body,revision FROM saved_objects").execute(&mut db).await.unwrap();
        drop(db);
        let mut session = store.session().await.unwrap();
        assert!(matches!(
            session
                .upstream_closure(&host(), &[("artifact", "s")], 10_000)
                .await,
            Err(Error::Internal)
        ));
        drop(session);
        remove_run_json_read_fault(&dir).await;
    }
}
