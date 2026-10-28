//! AG-158: real source/watermark-backed B/L/U comparison, without activation.
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::packages::{E16Envelope, E16SourceRef, PersistentPackageStore, StagedAssetState};
use evo_engine::seeds::{
    BUILTIN_I0_BYTES, DiffCategory, E16_CURRENT_SEED_INSTALL_SCHEMA, E16_SEED_INSTALL_SCHEMA,
    InstallCurrentSeedRequest, InstallSeedRequest, PersistentSeedStore, SeedClass, SeedIdentity,
    SeedInstallPayloadV1, SeedUpstream,
};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::path::Path;

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    admin: Context,
    sources: Vec<E16SourceRef>,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("seed.sqlite3")).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let source = json!({"schema_version":"fixture.seed_source.v1","id":"source"});
        let mut session = store.session().await.unwrap();
        session
            .put(&admin, "artifact", "source", admin.actor(), &source)
            .await
            .unwrap();
        session.bump_watermark(&admin, "initial").await.unwrap();
        session.commit().await.unwrap();
        Self {
            dir,
            store,
            admin,
            sources: vec![E16SourceRef {
                kind: "artifact".into(),
                id: "source".into(),
                digest: fingerprint(&source).unwrap(),
            }],
        }
    }

    fn request(&self, key: &str, b: &str, l: &str, u: &str) -> InstallCurrentSeedRequest {
        InstallCurrentSeedRequest {
            request_key: key.into(),
            baseline_identity: Some(SeedIdentity {
                publisher: "org.rsia".into(),
                asset_id: "seed".into(),
                kind: "skill".into(),
            }),
            local_identity: Some(SeedIdentity {
                publisher: "org.rsia".into(),
                asset_id: "seed".into(),
                kind: "skill".into(),
            }),
            local_marked: true,
            baseline_bytes: Some(b.as_bytes().to_vec()),
            local_bytes: Some(l.as_bytes().to_vec()),
            upstream: Some(SeedUpstream {
                path: "upstream/seed.txt".into(),
                content_digest: hash(u.as_bytes()),
                bytes: u.as_bytes().to_vec(),
            }),
            environment_digest: hash(b"environment"),
            source_refs: self.sources.clone(),
            compiler_version: "compiler-v1".into(),
            confirmed_bytes: None,
        }
    }

    async fn shape(&self) -> Value {
        let mut session = self.store.session().await.unwrap();
        let mut objects = Vec::new();
        let mut edges = Vec::new();
        for kind in [
            "run",
            "feedback",
            "event",
            "candidate",
            "release",
            "pointer",
            "receipt",
            "dataset",
            "evaluation",
            "budget",
            "reservation",
            "job",
            "improvement",
            "artifact",
            "tombstone",
        ] {
            let bodies: Vec<Value> = session.list(&self.admin, kind).await.unwrap();
            for body in &bodies {
                if let Some(id) = body["id"].as_str() {
                    let mut dependent = session.dependents(&self.admin, kind, id).await.unwrap();
                    dependent.sort();
                    edges.push(json!([kind, id, dependent]));
                }
            }
            objects.push(json!([kind, bodies]));
        }
        let watermark = session.watermark(&self.admin).await.unwrap();
        session.commit().await.unwrap();
        let audit = self.store.verify_audit(&self.admin).await.unwrap();
        let mut files = Vec::new();
        for name in ["blobs", "exports"] {
            files_under(self.dir.path(), &self.dir.path().join(name), &mut files);
        }
        files.sort();
        json!({"objects":objects,"edges":edges,"watermark":watermark,"audit":audit,"files":files})
    }

    async fn no_active(&self) {
        let mut session = self.store.session().await.unwrap();
        for kind in [
            "candidate",
            "release",
            "pointer",
            "receipt",
            "evaluation",
            "budget",
            "reservation",
        ] {
            assert!(
                session
                    .list::<Value>(&self.admin, kind)
                    .await
                    .unwrap()
                    .is_empty(),
                "{kind}"
            );
        }
        session.commit().await.unwrap();
    }
}

fn files_under(root: &Path, path: &Path, files: &mut Vec<(String, String)>) {
    if !path.exists() {
        return;
    }
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_under(root, &path, files);
        } else {
            files.push((
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                hash(&std::fs::read(path).unwrap()),
            ));
        }
    }
}

#[tokio::test]
async fn install_unknown_side_or_identity_refuses_before_writes() {
    let fx = Fixture::new().await;
    let good = fx.request("unknown", "baseline", "baseline", "baseline");
    let before = fx.shape().await;
    for index in 0..14 {
        let mut request = good.clone();
        match index {
            0 => request.baseline_bytes = None,
            1 => request.local_bytes = None,
            2 => request.upstream = None,
            3 => request.baseline_identity = None,
            4 => request.local_identity = None,
            5 => request.baseline_bytes = Some(vec![]),
            6 => request.local_bytes = Some(vec![255]),
            7 => request.upstream.as_mut().unwrap().bytes.clear(),
            8 => request.upstream.as_mut().unwrap().content_digest = hash(b"wrong"),
            9 => request.upstream.as_mut().unwrap().path.clear(),
            10 => request.upstream.as_mut().unwrap().path = "../seed".into(),
            11 => request
                .baseline_identity
                .as_mut()
                .unwrap()
                .publisher
                .clear(),
            12 => request.local_identity.as_mut().unwrap().asset_id.clear(),
            _ => request.local_identity.as_mut().unwrap().kind = "unknown".into(),
        }
        assert!(
            matches!(PersistentSeedStore::install_current(&fx.admin, &fx.store, request).await, Err(Error::Invalid(message)) if message == "seed_comparison_unknown"),
            "mutation {index}"
        );
        assert_eq!(fx.shape().await, before, "mutation {index}");
    }
    let mut missing_marker = good.clone();
    missing_marker.local_marked = false;
    assert!(
        matches!(PersistentSeedStore::install_current(&fx.admin, &fx.store, missing_marker).await, Err(Error::Conflict(message)) if message == "seed_comparison_missing_marker")
    );
    assert_eq!(fx.shape().await, before);
    let current = PersistentSeedStore::install_current(&fx.admin, &fx.store, good)
        .await
        .unwrap();
    assert_eq!(current.class, SeedClass::Unmodified);
    assert_eq!(
        current.install.schema_version,
        E16_CURRENT_SEED_INSTALL_SCHEMA
    );
}

#[tokio::test]
async fn install_uses_all_blu_relationships_without_silent_local_or_active_updates() {
    for (l, u, class, category) in [
        (
            "baseline",
            "baseline",
            SeedClass::Unmodified,
            DiffCategory::NoChange,
        ),
        (
            "local",
            "baseline",
            SeedClass::LocallyEdited,
            DiffCategory::LocalOnlyModified,
        ),
        (
            "baseline",
            "upstream",
            SeedClass::UpstreamNewer,
            DiffCategory::UpstreamOnlyModified,
        ),
        (
            "local",
            "upstream",
            SeedClass::BothChanged,
            DiffCategory::Conflict,
        ),
        (
            "upstream",
            "upstream",
            SeedClass::IdenticalToUpstream,
            DiffCategory::BothModifiedIdentical,
        ),
    ] {
        let fx = Fixture::new().await;
        let outcome = PersistentSeedStore::install_current(
            &fx.admin,
            &fx.store,
            fx.request("install", "baseline", l, u),
        )
        .await
        .unwrap();
        assert_eq!(outcome.class, class);
        assert_eq!(outcome.diff.category, category);
        assert_eq!(outcome.install.payload.baseline_digest, hash(b"baseline"));
        assert_eq!(outcome.install.payload.local_digest, hash(l.as_bytes()));
        assert_eq!(outcome.staged.is_some(), class == SeedClass::UpstreamNewer);
        assert!(!outcome.diff.auto_activated);
        fx.no_active().await;
    }
    let fx = Fixture::new().await;
    let original = PersistentSeedStore::install_current(
        &fx.admin,
        &fx.store,
        fx.request("original", "baseline", "baseline", "baseline"),
    )
    .await
    .unwrap();
    let before = fx.shape().await;
    for index in 0..3 {
        let mut request = fx.request("different-identity", "baseline", "baseline", "baseline");
        let local = request.local_identity.as_mut().unwrap();
        match index {
            0 => local.publisher = "other.publisher".into(),
            1 => local.asset_id = "other-seed".into(),
            _ => local.kind = "harness".into(),
        }
        assert!(
            matches!(PersistentSeedStore::install_current(&fx.admin, &fx.store, request).await, Err(Error::Conflict(message)) if message == "seed_comparison_identity_differs")
        );
        assert_eq!(fx.shape().await, before);
    }
    let mut independent = fx.request(
        "independent",
        "different baseline",
        "different baseline",
        "different baseline",
    );
    independent.baseline_identity.as_mut().unwrap().publisher = "other.publisher".into();
    independent.local_identity.as_mut().unwrap().publisher = "other.publisher".into();
    let independent = PersistentSeedStore::install_current(&fx.admin, &fx.store, independent)
        .await
        .unwrap();
    assert_ne!(original.install.id, independent.install.id);
    let builtin = PersistentSeedStore::install(
        &fx.admin,
        &fx.store,
        InstallSeedRequest {
            request_key: "builtin".into(),
            publisher: "rsia.builtin".into(),
            asset_id: "improver-i0".into(),
            kind: "improver".into(),
            baseline_bytes: BUILTIN_I0_BYTES.to_vec(),
            local_bytes: BUILTIN_I0_BYTES.to_vec(),
            upstream_bytes: Some(BUILTIN_I0_BYTES.to_vec()),
            environment_digest: hash(b"environment"),
            source_refs: fx.sources.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(builtin.payload.catalog_version, Some(2));
    fx.no_active().await;
}

#[tokio::test]
async fn upstream_newer_only_stages_and_replay_is_idempotent() {
    let fx = Fixture::new().await;
    let initial = PersistentSeedStore::install_current(
        &fx.admin,
        &fx.store,
        fx.request("baseline", "baseline", "baseline", "baseline"),
    )
    .await
    .unwrap()
    .install;
    let request = fx.request("new-upstream", "baseline", "baseline", "upstream");
    let outcome = PersistentSeedStore::install_current(&fx.admin, &fx.store, request.clone())
        .await
        .unwrap();
    assert_eq!(outcome.class, SeedClass::UpstreamNewer);
    assert_eq!(
        fingerprint(&initial).unwrap(),
        fingerprint(&outcome.install).unwrap()
    );
    let staged = outcome.staged.unwrap();
    assert_eq!(staged.payload.package_schema_version, "rsia.seed.update.v1");
    assert_eq!(staged.payload.state, StagedAssetState::Staged);
    assert!(staged.payload.candidate_ref.is_none());
    assert!(staged.payload.candidate_material.is_none());
    assert_eq!(
        fx.store
            .read_blob(&fx.admin, &staged.payload.content_blob_digest, 1024)
            .await
            .unwrap(),
        b"upstream"
    );
    PersistentPackageStore::read_staged(&fx.admin, &fx.store, &staged.id)
        .await
        .unwrap();
    let mut session = fx.store.session().await.unwrap();
    let mut original_bodies = Vec::new();
    for id in [&initial.id, &staged.id] {
        let row = session
            .object_row(&fx.admin, "artifact", id)
            .await
            .unwrap()
            .unwrap();
        original_bodies.push((id.clone(), row.body));
    }
    session.commit().await.unwrap();
    let before = fx.shape().await;
    let replay = PersistentSeedStore::install_current(&fx.admin, &fx.store, request)
        .await
        .unwrap();
    assert_eq!(replay.staged.unwrap().id, staged.id);
    assert_eq!(fx.shape().await, before);
    let mut same_key = fx.request("new-upstream", "baseline", "baseline", "different upstream");
    same_key.upstream.as_mut().unwrap().path = "upstream/version3.txt".into();
    assert!(
        matches!(PersistentSeedStore::install_current(&fx.admin, &fx.store, same_key).await, Err(Error::Conflict(message)) if message == "seed update request key changed")
    );
    assert_eq!(fx.shape().await, before);
    for (local, upstream) in [("baseline", "baseline"), ("upstream", "upstream")] {
        let before = fx.shape().await;
        let same_key = fx.request("new-upstream", "baseline", local, upstream);
        assert!(
            matches!(PersistentSeedStore::install_current(&fx.admin, &fx.store, same_key).await, Err(Error::Conflict(message)) if message == "seed update request key changed")
        );
        assert_eq!(fx.shape().await, before);
        let mut session = fx.store.session().await.unwrap();
        for (id, body) in &original_bodies {
            assert_eq!(
                &session
                    .object_row(&fx.admin, "artifact", id)
                    .await
                    .unwrap()
                    .unwrap()
                    .body,
                body
            );
        }
        session.commit().await.unwrap();
    }
    fx.no_active().await;
}

#[tokio::test]
async fn both_changed_returns_diff_until_explicit_candidate_confirmation() {
    let fx = Fixture::new().await;
    let initial = PersistentSeedStore::install_current(
        &fx.admin,
        &fx.store,
        fx.request("baseline", "baseline", "baseline", "baseline"),
    )
    .await
    .unwrap()
    .install;
    let request = fx.request("compare", "baseline", "local edit", "upstream edit");
    let before = fx.shape().await;
    let comparison = PersistentSeedStore::install_current(&fx.admin, &fx.store, request.clone())
        .await
        .unwrap();
    assert_eq!(comparison.class, SeedClass::BothChanged);
    assert_eq!(comparison.diff.category, DiffCategory::Conflict);
    assert!(comparison.diff.has_conflicts);
    assert!(comparison.diff.requires_new_evaluation);
    let preview = comparison.diff.merged_preview.unwrap();
    for part in [
        "<<<<<<< LOCAL (user modified)",
        "local edit",
        "=======",
        "upstream edit",
        ">>>>>>> UPSTREAM (new upstream)",
    ] {
        assert!(preview.contains(part));
    }
    assert!(comparison.staged.is_none());
    assert_eq!(fx.shape().await, before);
    let mut confirmed = request;
    confirmed.request_key = "manual-resolution".into();
    confirmed.confirmed_bytes = Some(b"human resolved material".to_vec());
    let confirmed = PersistentSeedStore::install_current(&fx.admin, &fx.store, confirmed)
        .await
        .unwrap();
    assert_eq!(
        fingerprint(&confirmed.install).unwrap(),
        fingerprint(&initial).unwrap()
    );
    let staged = confirmed.staged.unwrap();
    assert_eq!(
        fx.store
            .read_blob(&fx.admin, &staged.payload.content_blob_digest, 1024)
            .await
            .unwrap(),
        b"human resolved material"
    );
    assert!(staged.payload.candidate_ref.is_none());
    assert_eq!(staged.payload.state, StagedAssetState::Staged);
    fx.no_active().await;
}

#[tokio::test]
async fn seed_update_staging_capacity_refuses_without_partial_install_or_active_write() {
    let fx = Fixture::new().await;
    let installed = PersistentSeedStore::install_current(
        &fx.admin,
        &fx.store,
        fx.request("first", "baseline", "baseline", "upstream"),
    )
    .await
    .unwrap();
    let first = installed.staged.unwrap();
    for index in 0..19 {
        PersistentSeedStore::stage_reset_to_baseline(
            &fx.admin,
            &fx.store,
            &installed.install.id,
            &format!("fill-{index}"),
            &hash(b"environment"),
            "compiler-v1",
        )
        .await
        .unwrap();
    }
    let before = fx.shape().await;
    let mut request = fx.request("overflow", "baseline", "baseline", "new upstream");
    request.baseline_identity.as_mut().unwrap().asset_id = "new-seed".into();
    request.local_identity.as_mut().unwrap().asset_id = "new-seed".into();
    assert!(
        matches!(PersistentSeedStore::install_current(&fx.admin, &fx.store, request).await, Err(Error::Conflict(message)) if message.contains("staged packages") && message.contains("20"))
    );
    assert_eq!(fx.shape().await, before);
    let replay = PersistentSeedStore::install_current(
        &fx.admin,
        &fx.store,
        fx.request("first", "baseline", "baseline", "upstream"),
    )
    .await
    .unwrap();
    assert_eq!(replay.staged.unwrap().id, first.id);
    assert_eq!(fx.shape().await, before);
    fx.no_active().await;
}

#[tokio::test]
async fn legacy_v1_install_stays_readonly_and_never_becomes_current_comparison_input() {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/ag158_seed_install_v1.json")).unwrap();
    for label in [
        "typed_observation",
        "raw_observation",
        "revocable_observation",
    ] {
        let fixture = &fixtures[label];
        assert_eq!(
            fixture["producer_sha"],
            "7d22edb5dfe6ec84f7a3167aba02ae683e9941be"
        );
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("legacy.sqlite3"))
            .await
            .unwrap();
        let admin =
            Context::new(fixture["namespace"].as_str().unwrap(), "admin", Role::Admin).unwrap();
        let observed: E16Envelope<SeedInstallPayloadV1> =
            if let Some(raw) = fixture["stored_envelope_body"].as_str() {
                assert_eq!(hash(raw.as_bytes()), fixture["stored_envelope_body_sha256"]);
                let observed = serde_json::from_str(raw).unwrap();
                assert_eq!(serde_json::to_string(&observed).unwrap(), raw);
                observed
            } else {
                serde_json::from_value(fixture["envelope"].clone()).unwrap()
            };
        let source: Value = if let Some(body) = fixture["source_body"].as_str() {
            assert_eq!(hash(body.as_bytes()), fixture["source_body_sha256"]);
            serde_json::from_str(body).unwrap()
        } else {
            fixture["source_body"].clone()
        };
        for field in ["baseline_bytes", "local_bytes", "upstream_bytes"] {
            let bytes: Vec<u8> = serde_json::from_value(fixture["request"][field].clone()).unwrap();
            store.put_blob(&admin, &bytes).await.unwrap();
        }
        let mut session = store.session().await.unwrap();
        let source_ref = &observed.source_refs[0];
        let source_owner = if label == "revocable_observation" {
            assert_eq!(fixture["source_is_execution_or_formal_proof"], false);
            assert_eq!(fixture["source_kind"], "run");
            assert_eq!(fixture["source_edges"], json!([]));
            assert_eq!(
                fixture["dependency_edges"],
                json!([[source_ref.kind, source_ref.id]])
            );
            fixture["source_owner_actor"].as_str().unwrap()
        } else {
            admin.actor()
        };
        let source_context = Context::new(
            admin.namespace(),
            source_owner,
            if label == "revocable_observation" {
                Role::Host
            } else {
                Role::Admin
            },
        )
        .unwrap();
        session
            .put(
                &source_context,
                &source_ref.kind,
                &source_ref.id,
                source_owner,
                &source,
            )
            .await
            .unwrap();
        session
            .put(&admin, "artifact", &observed.id, admin.actor(), &observed)
            .await
            .unwrap();
        session
            .put_edge(
                &admin,
                "artifact",
                &observed.id,
                &source_ref.kind,
                &source_ref.id,
            )
            .await
            .unwrap();
        assert_eq!(
            session
                .dependents(&admin, &source_ref.kind, &source_ref.id)
                .await
                .unwrap(),
            vec![("artifact".to_string(), observed.id.clone())]
        );
        session
            .bump_watermark(&admin, "legacy-watermark")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let legacy = PersistentSeedStore::read_legacy_install(&admin, &store, &observed.id)
            .await
            .unwrap();
        assert_eq!(legacy.schema_version, E16_SEED_INSTALL_SCHEMA);
        assert_eq!(
            fingerprint(&legacy).unwrap(),
            fingerprint(&observed).unwrap()
        );
        assert!(
            matches!(PersistentSeedStore::read_install(&admin, &store, &observed.id).await, Err(Error::Conflict(message)) if message == "seed_install_legacy")
        );
        let staged = PersistentSeedStore::stage_reset_to_baseline(
            &admin,
            &store,
            &observed.id,
            "explicit-safe-b",
            &observed.payload.environment_digest,
            "compiler-v1",
        )
        .await
        .unwrap();
        assert_eq!(staged.payload.state, StagedAssetState::Staged);
        assert_eq!(
            staged.payload.content_blob_digest,
            observed.payload.baseline_digest
        );
        assert!(staged.payload.candidate_ref.is_none());
        PersistentPackageStore::read_staged(&admin, &store, &staged.id)
            .await
            .unwrap();
        let mut session = store.session().await.unwrap();
        let stored = session
            .object_row(&admin, "artifact", &observed.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.body, serde_json::to_string(&observed).unwrap());
        if let Some(raw) = fixture["stored_envelope_body"].as_str() {
            assert_eq!(stored.body, raw);
        }
        if let Some(raw) = fixture["source_body"].as_str() {
            assert_eq!(
                session
                    .object_row(&admin, &source_ref.kind, &source_ref.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .body,
                raw
            );
        }
        assert!(
            session
                .list::<Value>(&admin, "pointer")
                .await
                .unwrap()
                .is_empty()
        );
        session.commit().await.unwrap();
        // Only this independently captured third instance has a real revocable
        // run source. The two earlier observed artifacts keep their bodies and
        // source relationships; no ancestry is invented for them.
        if label == "revocable_observation" {
            assert_eq!(
                fixture["parent_cleanup_health"]["status"]["state"],
                "complete"
            );
            let mut cleanup = LifecycleStore::begin_revoke(
                &admin,
                &store,
                TypedObjectRef {
                    kind: source_ref.kind.clone(),
                    id: source_ref.id.clone(),
                },
                "legacy source revoked",
                2,
            )
            .await
            .unwrap();
            for tick in 3..30 {
                cleanup = LifecycleStore::cleanup_step(&admin, &store, &cleanup.job_id, 16, tick)
                    .await
                    .unwrap();
                if cleanup.pending_nodes == 0 {
                    break;
                }
            }
            assert_eq!(cleanup.state, CleanupState::Complete);
            let mut session = store.session().await.unwrap();
            let redacted: Value = session
                .need(&admin, "artifact", &observed.id)
                .await
                .unwrap();
            assert_eq!(redacted["schema_version"], "rsia.redacted.v1");
            assert_eq!(
                redacted["original_digest"],
                fixture["stored_envelope_body_sha256"]
            );
            let reset: Value = session.need(&admin, "artifact", &staged.id).await.unwrap();
            assert_eq!(reset["schema_version"], "rsia.redacted.v1");
            session.commit().await.unwrap();
            assert!(matches!(
                store
                    .read_blob(&admin, &observed.payload.baseline_digest, 1024)
                    .await,
                Err(Error::NotFound)
            ));
        }
    }
}
