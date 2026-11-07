use evo_core::capacity::CapacityItem as I;
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallReservation, BudgetStage, RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};

fn admin(namespace: &str) -> Context {
    Context::new(namespace, "admin", Role::Admin).unwrap()
}
async fn database() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("rsia.sqlite3")).await.unwrap();
    (dir, store)
}
async fn put(store: &Store, ctx: &Context, kind: &str, id: &str, body: &Value) {
    let mut session = store.session().await.unwrap();
    session.put(ctx, kind, id, ctx.actor(), body).await.unwrap();
    session.commit().await.unwrap();
}
fn authority(id: &str) -> Value {
    json!({"schema_version":"rsia.optimization.source.v1",
      "record":{"id":id,"body":id.as_bytes(),"parent_family":format!("family-{id}"),"task_origin":"trusted_run","execution_attestation":"trusted_host","purpose":"development"},
      "trace":{"run_id":id,"parent_family":format!("family-{id}"),"source_digest":hash(id.as_bytes()),"purpose":"development","outcome":"success","diagnosis":null,"excerpt":id,"seed":1},"excerpt_start":0,"excerpt_end":id.len()})
}
async fn stage(store: &Store, ctx: &Context, id: &str, source: &str) -> Value {
    let native = authority(source);
    put(store, ctx, "run", source, &native).await;
    let bytes = format!("projection-{id}").into_bytes();
    let blob = store.put_blob(ctx, &bytes).await.unwrap();
    let body = json!({"schema_version":"rsia.e16.staged_asset.v1","id":id,"namespace":ctx.namespace(),"owner_actor":ctx.actor(),
      "request_key":format!("request-{id}"),"input_digest":hash(id.as_bytes()),"created_at":1,"updated_at":1,
      "source_refs":[{"kind":"run","id":source,"digest":fingerprint(&native).unwrap()}],"revoke_watermark":0,
      "payload":{"publisher":"fixture","asset_id":id,"package_kind":"skill","manifest_digest":hash(b"manifest"),
      "content_blob_digest":blob,"content_bytes":bytes.len(),"baseline_digest":hash(b"baseline"),"local_digest":hash(b"local"),"upstream_digest":null,
      "environment_digest":hash(b"environment"),"package_schema_version":"rsia.asset_package_manifest.v1","compiler_version":"fixture",
      "state":"staged","quarantine_reason":null,"candidate_material":null,"candidate_ref":null}});
    let mut session = store.session().await.unwrap();
    session
        .put(ctx, "artifact", id, ctx.actor(), &body)
        .await
        .unwrap();
    session
        .put_edge(ctx, "artifact", id, "run", source)
        .await
        .unwrap();
    session
        .put_edge(
            ctx,
            "artifact",
            id,
            "blob",
            body["payload"]["content_blob_digest"].as_str().unwrap(),
        )
        .await
        .unwrap();
    session
        .audit(ctx, "capacity.fixture.stage", id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    body
}
async fn used(store: &Store, item: I) -> u64 {
    let mut session = store.session().await.unwrap();
    let used = session.capacity_used(item).await.unwrap();
    session.commit().await.unwrap();
    used
}
async fn cleanup(store: &Store, ctx: &Context, source: &str) {
    let mut status = LifecycleStore::begin_revoke(
        ctx,
        store,
        TypedObjectRef {
            kind: "run".into(),
            id: source.into(),
        },
        "capacity fixture",
        10,
    )
    .await
    .unwrap();
    for now in 11..200 {
        if status.state == CleanupState::Complete {
            return;
        }
        assert_ne!(status.state, CleanupState::Failed, "{status:?}");
        status = LifecycleStore::cleanup_step(ctx, store, &status.job_id, 32, now)
            .await
            .unwrap();
    }
    panic!("cleanup did not complete: {status:?}");
}
fn cycle(id: &str, report: &str) -> Value {
    json!({"id":id,"schema_version":"rsia.monitoring.development_cycle.v2","scope_id":"scope","ordinal":1,
      "report_fact_id":report,"report_digest":hash(report.as_bytes()),"request_id":format!("request-{id}"),"candidate_bundle_digest":hash(b"bundle"),
      "execution_receipt_id":format!("receipt-{id}"),"usage_record_ids":[],"provenance":"fixture","pairs":[],"has_contrast":false,"sources":[]})
}

#[tokio::test]
async fn instance_counts_merge_namespaces_in_one_session() {
    let (_dir, store) = database().await;
    stage(&store, &admin("a"), "stage-a", "source-a").await;
    stage(&store, &admin("b"), "stage-b", "source-b").await;
    let mut session = store.session().await.unwrap();
    assert_eq!(session.capacity_used(I::StagingPackage).await.unwrap(), 2);
    assert_eq!(session.capacity_usage_v41(0).await.unwrap().runs, 2);
    match session.admit_capacity(&[(I::StagingPackage, 19)]).await {
        Err(Error::Capacity(detail)) => {
            assert_eq!(detail.item, I::StagingPackage);
            assert_eq!((detail.used, detail.limit, detail.delta), (2, 20, 19));
        }
        other => panic!("whole intent must be refused: {other:?}"),
    }
    session.commit().await.unwrap();
    assert_eq!(used(&store, I::StagingPackage).await, 2);
}

#[tokio::test]
async fn competing_limit_minus_one_creations_have_one_winner() {
    let (_dir, store) = database().await;
    let ctx = admin("a");
    let mut session = store.session().await.unwrap();
    for index in 0..999 {
        let id = format!("cycle-{index}");
        session
            .put(
                &ctx,
                "artifact",
                &id,
                ctx.actor(),
                &cycle(&id, &format!("report-{index}")),
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let create = |id: &'static str| {
        let store = store.clone();
        async move {
            let ctx = admin(id);
            let mut session = store.session().await?;
            session.admit_capacity(&[(I::DevelopmentCycle, 1)]).await?;
            session
                .put(&ctx, "artifact", id, ctx.actor(), &cycle(id, id))
                .await?;
            session.commit().await
        }
    };
    let (a, b) = tokio::join!(create("winner-a"), create("winner-b"));
    assert_eq!(u8::from(a.is_ok()) + u8::from(b.is_ok()), 1);
    let refused = if a.is_err() { a } else { b };
    match refused {
        Err(Error::Capacity(detail)) => assert_eq!(
            (detail.item, detail.used, detail.limit, detail.delta),
            (I::DevelopmentCycle, 1000, 1000, 1)
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(used(&store, I::DevelopmentCycle).await, 1000);
}

#[tokio::test]
async fn capacity_classification_requires_exact_schema_kind_identity_and_owner() {
    let (_dir, store) = database().await;
    let ctx = admin("a");
    let good = stage(&store, &ctx, "actual", "source").await;
    for (id, kind, field, value) in [
        (
            "wrong-schema",
            "artifact",
            "schema_version",
            json!("rsia.e16.staged_asset.v99"),
        ),
        ("wrong-kind", "feedback", "id", json!("wrong-kind")),
        (
            "wrong-owner",
            "artifact",
            "owner_actor",
            json!("someone-else"),
        ),
        ("wrong-namespace", "artifact", "namespace", json!("b")),
    ] {
        let mut body = good.clone();
        body["id"] = json!(id);
        body[field] = value;
        put(&store, &ctx, kind, id, &body).await;
    }
    put(
        &store,
        &ctx,
        "artifact",
        "tag-only",
        &json!({"id":"tag-only","schema_version":"rsia.e16.staged_asset.v1"}),
    )
    .await;
    // Exact schema and object kind classify a row (the store itself binds the
    // body id to the row id). A classified row whose owner or namespace field
    // disagrees, or which is only a tag, still holds its slot: an unverifiable
    // record fails closed instead of freeing quota.
    assert_eq!(used(&store, I::StagingPackage).await, 4);
    // Occupancy identities do not grant a management claim or Current input.
    for (schema, id) in [
        (evo_storage::MANAGEMENT_JOB_V1_SCHEMA, "management-v1"),
        (evo_storage::MANAGEMENT_JOB_V2_SCHEMA, "management-v2"),
    ] {
        put(
            &store,
            &ctx,
            "job",
            id,
            &json!({"id":id,"schema_version":schema,"namespace":ctx.namespace(),"owner_actor":ctx.actor()}),
        )
        .await;
    }
    assert_eq!(used(&store, I::ManagementJobRecord).await, 2);
}

#[tokio::test]
async fn cleanup_evidence_alone_does_not_excuse_malformed_redacted_body() {
    let (_dir, store) = database().await;
    let ctx = admin("a");
    stage(&store, &ctx, "stage", "source").await;
    cleanup(&store, &ctx, "source").await;
    assert_eq!(used(&store, I::StagingPackage).await, 0);
    let mut session = store.session().await.unwrap();
    let original: Value = session.need(&ctx, "artifact", "stage").await.unwrap();
    session.commit().await.unwrap();
    let mut malformed = original.clone();
    malformed["unredacted_content"] = json!("must not excuse quota");
    put(&store, &ctx, "artifact", "stage", &malformed).await;
    assert_eq!(used(&store, I::StagingPackage).await, 1);
    put(&store, &ctx, "artifact", "stage", &original).await;
    assert_eq!(used(&store, I::StagingPackage).await, 0);
}

#[tokio::test]
async fn lawful_cleanup_releases_only_content_quota_and_keeps_history() {
    let (_dir, store) = database().await;
    let ctx = admin("a");
    stage(&store, &ctx, "stage", "source").await;
    store
        .authorize_root_budget(
            &ctx,
            &RootBudgetAuthorization {
                root_budget_id: "root".into(),
                billing_scope: "bill".into(),
                allowed_namespaces: vec!["a".into()],
                currency: "USD".into(),
                pricing_version: "price".into(),
                payment_subject: "payer".into(),
                authorization_receipt_digest: hash(b"authorized"),
                per_call_cap_micros: 20,
                total_limit_micros: 100,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let host = Context::new("a", "host", Role::Host).unwrap();
    let call = store
        .reserve_budget_call(
            &host,
            &BudgetCallReservation {
                billing_scope: "bill".into(),
                call_id: "paid".into(),
                dispatch_group_id: "paid".into(),
                stage: BudgetStage::Reflection,
                actual_input_digest: hash(b"input"),
                request_artifact: None,
                max_cost_micros: 20,
                lease_token: "lease".into(),
                lease_until: 100,
                now: 2,
            },
        )
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now: 3,
    };
    let dispatched = store
        .begin_budget_dispatch(&host, &fence)
        .await
        .unwrap()
        .call;
    let paid = store
        .finalize_budget_call(
            &host,
            &fence,
            &UsageCharge {
                amount_micros: 7,
                currency: "USD".into(),
                pricing_version: "price".into(),
                provider_request_id: "provider".into(),
                usage_record_id: "usage".into(),
                output_digest: hash(b"output"),
            },
        )
        .await
        .unwrap();
    let closed = store
        .close_budget_call_execution(
            &host,
            "bill",
            "paid",
            dispatched.dispatch_id.as_deref().unwrap(),
            "finished",
            4,
        )
        .await
        .unwrap();
    assert_eq!(paid.actual_cost_micros, Some(7));
    let root =
        serde_json::to_value(store.root_budget(&ctx, "bill").await.unwrap().unwrap()).unwrap();
    let audit = store.verify_audit(&ctx).await.unwrap();
    cleanup(&store, &ctx, "source").await;
    assert_eq!(used(&store, I::StagingPackage).await, 0);
    assert_eq!(used(&store, I::Run).await, 0);
    assert_eq!(
        store
            .budget_call(&host, "bill", "paid")
            .await
            .unwrap()
            .unwrap(),
        closed
    );
    assert_eq!(
        serde_json::to_value(store.root_budget(&ctx, "bill").await.unwrap().unwrap()).unwrap(),
        root
    );
    assert!(store.verify_audit(&ctx).await.unwrap() >= audit);
}

#[tokio::test]
async fn forged_redaction_or_deleted_sources_never_release_quota() {
    let (_dir, store) = database().await;
    let ctx = admin("a");
    let original = stage(&store, &ctx, "stage", "source").await;
    let mut session = store.session().await.unwrap();
    session.delete(&ctx, "run", "source").await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(used(&store, I::StagingPackage).await, 1);
    let fake = json!({"id":"stage","schema_version":"rsia.redacted.v1","state":"source_revoked","original_kind":"staged_asset","original_schema":"rsia.e16.staged_asset.v1","original_digest":hash(original.to_string().as_bytes()),"metadata":{"namespace":"a","owner_actor":"admin","payload":original["payload"]}});
    put(&store, &ctx, "artifact", "stage", &fake).await;
    assert_eq!(used(&store, I::StagingPackage).await, 1);
}

#[tokio::test]
async fn skill_birth_identity_excludes_improver_and_deduplicates_proven_projections() {
    let (_dir, store) = database().await;
    let ctx = admin("a");
    let proposal = |kind| evo_core::Proposal {
        request_key: "key".into(),
        run_id: "run".into(),
        parent_snapshot: "snapshot".into(),
        kind,
        hypothesis: "hypothesis".into(),
        applicability: "when".into(),
        counterexample: "unless".into(),
        content: "same content".into(),
        evidence_refs: vec![],
        required_capabilities: vec![],
        dependencies: vec![],
    };
    for (id, kind) in [
        ("skill-a", evo_core::ArtifactKind::Skill),
        ("skill-b", evo_core::ArtifactKind::Skill),
        ("improver", evo_core::ArtifactKind::Improver),
    ] {
        let body = serde_json::to_value(evo_core::Candidate {
            id: id.into(),
            owner: ctx.actor().into(),
            proposal: proposal(kind),
            digest: hash(b"same body"),
            state: evo_core::CandidateState::Proposed,
            improver_version: "i0".into(),
            evaluation_id: None,
            approved_by: None,
            created_at: 1,
        })
        .unwrap();
        put(&store, &ctx, "candidate", id, &body).await;
    }
    assert_eq!(used(&store, I::Skill).await, 2);
    let mut staged = stage(&store, &ctx, "projection", "source").await;
    staged["payload"]["candidate_ref"] = json!("skill-a");
    put(&store, &ctx, "artifact", "projection", &staged).await;
    // A name alone cannot prove a Current projection or erase the other kind.
    assert_eq!(used(&store, I::Skill).await, 3);
    let mut session = store.session().await.unwrap();
    session
        .put_edge(&ctx, "artifact", "skill-a", "artifact", "projection")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(used(&store, I::Skill).await, 3);
    let mut bundle = evo_core::contract::ResolvedBundleV3 {
        schema_version: evo_core::contract::BUNDLE_SCHEMA_V3.into(),
        compiler_version: evo_core::contract::BUNDLE_COMPILER_VERSION_V3.into(),
        profile_id: "profile".into(),
        parent_digest: hash(b"parent"),
        baseline_digest: hash(b"baseline"),
        skill: evo_core::contract::SkillSnapshot::empty(),
        improver: evo_core::Strategy::default(),
        origins: vec![],
        digest: String::new(),
    };
    bundle.digest = evo_core::contract::resolved_bundle_digest_v3(&bundle).unwrap();
    let current = json!({"schema_version":"rsia.release_candidate.v2","id":"current","bundle":bundle,"bundle_digest":bundle.digest,"environment_digest":hash(b"environment"),"profile_id":"profile","parent_digest":hash(b"parent"),"proposer_actor":ctx.actor(),"sources":[],"revoke_watermark":0});
    put(&store, &ctx, "artifact", "current", &current).await;
    assert_eq!(used(&store, I::Skill).await, 4);
    staged["payload"]["candidate_ref"] = json!("current");
    put(&store, &ctx, "artifact", "projection", &staged).await;
    assert_eq!(used(&store, I::Skill).await, 4);
    let mut session = store.session().await.unwrap();
    session
        .put_edge(&ctx, "artifact", "current", "artifact", "projection")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(used(&store, I::Skill).await, 3);
}

#[tokio::test]
async fn cycle_projection_and_embedded_failure_identity_are_counted_once() {
    let (_dir, store) = database().await;
    let ctx = admin("a");
    put(
        &store,
        &ctx,
        "artifact",
        "cycle",
        &cycle("cycle", "observed"),
    )
    .await;
    let envelope = |kind: &str, logical: &str, payload: Value| json!({"id":format!("e12-{}",fingerprint(&(kind,logical)).unwrap()),"schema_version":"rsia.curriculum_artifact_envelope.v1","record_kind":kind,"payload":payload});
    let receipt = envelope(
        "development_cycle_receipt_v1",
        "receipt",
        json!({"id":"receipt","development_observed_fact_id":"observed"}),
    );
    put(
        &store,
        &ctx,
        "artifact",
        receipt["id"].as_str().unwrap(),
        &receipt,
    )
    .await;
    for id in ["state-a", "state-b"] {
        let state = envelope(
            "learner_state_v2",
            id,
            json!({"id":id,"failure_records":[{"record_id":"same-failure","status":"resolved"},{"record_id":format!("diagnostic-{id}"),"status":"diagnostic"}]}),
        );
        put(
            &store,
            &ctx,
            "artifact",
            state["id"].as_str().unwrap(),
            &state,
        )
        .await;
    }
    assert_eq!(used(&store, I::DevelopmentCycle).await, 1);
    assert_eq!(used(&store, I::FailureRecord).await, 3);
}
