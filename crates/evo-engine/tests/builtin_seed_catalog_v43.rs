//! AG-158 first-party I0 catalog; audit inputs cannot become runtime authority.
use evo_core::improver::ImproverContentV2;
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::packages::{E16Envelope, E16SourceRef, PersistentPackageStore, StagedAssetState};
use evo_engine::seeds::{
    BUILTIN_I0_BYTES, BUILTIN_SEED_CATALOG_BYTES, InstallCurrentSeedRequest, InstallSeedRequest,
    PersistentSeedStore, SeedClass, SeedIdentity, SeedInstallPayloadV1, SeedStatus, SeedUpstream,
    builtin_seed_catalog, validate_builtin_seed_catalog,
};
use evo_storage::Store;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

async fn fixture() -> (tempfile::TempDir, Store, Context, Vec<E16SourceRef>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("catalog.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let body = json!({"schema_version":"fixture.first_party_source.v1","id":"source","license":"Source-Available"});
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", "source", admin.actor(), &body)
        .await
        .unwrap();
    session
        .bump_watermark(&admin, "catalog-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
    (
        dir,
        store,
        admin,
        vec![E16SourceRef {
            kind: "artifact".into(),
            id: "source".into(),
            digest: fingerprint(&body).unwrap(),
        }],
    )
}

fn request(key: &str, sources: Vec<E16SourceRef>) -> InstallSeedRequest {
    InstallSeedRequest {
        request_key: key.into(),
        publisher: "rsia.builtin".into(),
        asset_id: "improver-i0".into(),
        kind: "improver".into(),
        baseline_bytes: BUILTIN_I0_BYTES.to_vec(),
        local_bytes: BUILTIN_I0_BYTES.to_vec(),
        upstream_bytes: Some(BUILTIN_I0_BYTES.to_vec()),
        environment_digest: hash(b"environment"),
        source_refs: sources,
    }
}

async fn shape(store: &Store, admin: &Context, dir: &Path) -> Value {
    let mut session = store.session().await.unwrap();
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
        let bodies: Vec<Value> = session.list(admin, kind).await.unwrap();
        for body in &bodies {
            if let Some(id) = body["id"].as_str() {
                let mut dependents = session.dependents(admin, kind, id).await.unwrap();
                dependents.sort();
                edges.push(json!([kind, id, dependents]));
            }
        }
        objects.push(json!([kind, bodies]));
    }
    let watermark = session.watermark(admin).await.unwrap();
    session.commit().await.unwrap();
    let mut files = Vec::new();
    let blobs = dir.join("blobs");
    if blobs.exists() {
        for namespace in std::fs::read_dir(blobs).unwrap() {
            for entry in std::fs::read_dir(namespace.unwrap().path()).unwrap() {
                let path = entry.unwrap().path();
                files.push((
                    path.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    hash(&std::fs::read(path).unwrap()),
                ));
            }
        }
    }
    files.sort();
    json!([
        objects,
        edges,
        watermark,
        store.verify_audit(admin).await.unwrap(),
        files
    ])
}

#[test]
fn embedded_catalog_metadata_license_and_git_blob_match() {
    let catalog = builtin_seed_catalog().unwrap();
    assert_eq!(catalog.schema_version, "rsia.builtin_seed_catalog.v1");
    assert_eq!(catalog.entries.len(), 1);
    let entry = &catalog.entries[0];
    assert_eq!(
        (
            &*entry.publisher,
            &*entry.asset_id,
            &*entry.kind,
            entry.version
        ),
        ("rsia.builtin", "improver-i0", "improver", 2)
    );
    assert_eq!(entry.license.name, "Source-Available");
    assert_eq!(BUILTIN_I0_BYTES.len(), 278);
    assert!(!BUILTIN_I0_BYTES.ends_with(b"\n"));
    let actual = ImproverContentV2::builtin_default();
    assert_eq!(serde_json::to_vec(&actual).unwrap(), BUILTIN_I0_BYTES);
    assert_eq!(actual.content_digest().unwrap(), hash(BUILTIN_I0_BYTES));
    assert_eq!(entry.content_sha256, actual.content_digest().unwrap());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (path, blob, digest) in [
        (&entry.path, &entry.git_blob, &entry.content_sha256),
        (
            &entry.license.path,
            &entry.license.git_blob,
            &entry.license.sha256,
        ),
    ] {
        assert_eq!(hash(&std::fs::read(root.join(path)).unwrap()), *digest);
        let output = Command::new("git")
            .current_dir(&root)
            .args(["rev-parse", &format!("HEAD:{path}")])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), blob);
    }
}

#[tokio::test]
async fn unregistered_builtin_install_and_reset_refuse_before_writes() {
    let (dir, store, admin, sources) = fixture().await;
    let installed =
        PersistentSeedStore::install(&admin, &store, request("healthy", sources.clone()))
            .await
            .unwrap();
    assert_eq!(installed.payload.status, SeedStatus::Installed);
    let reset = PersistentSeedStore::stage_reset_to_baseline(
        &admin,
        &store,
        &installed.id,
        "healthy-reset",
        &hash(b"environment"),
        "compiler-v1",
    )
    .await
    .unwrap();
    assert_eq!(reset.payload.state, StagedAssetState::Staged);
    let before = shape(&store, &admin, dir.path()).await;
    assert_eq!(
        PersistentSeedStore::install(&admin, &store, request("healthy", sources.clone()))
            .await
            .unwrap()
            .id,
        installed.id
    );
    assert_eq!(shape(&store, &admin, dir.path()).await, before);
    assert!(
        matches!(PersistentSeedStore::install(&admin, &store, request("another-initial-copy", sources.clone())).await, Err(Error::Conflict(message)) if message == "seed_initial_install_already_exists")
    );
    assert_eq!(shape(&store, &admin, dir.path()).await, before);
    let mut unregistered = request("unknown", sources.clone());
    unregistered.asset_id = "unregistered-i0".into();
    assert!(
        matches!(PersistentSeedStore::install(&admin, &store, unregistered).await, Err(Error::Invalid(message)) if message == "seed_catalog_unregistered")
    );
    assert_eq!(shape(&store, &admin, dir.path()).await, before);
    let mut missing = request("missing-u", sources.clone());
    missing.upstream_bytes = None;
    assert!(
        matches!(PersistentSeedStore::install(&admin, &store, missing).await, Err(Error::Invalid(message)) if message == "seed_comparison_unknown")
    );
    assert_eq!(shape(&store, &admin, dir.path()).await, before);
    // Synthetic unregistered material, distinct from the observed v1 fixture.
    let legacy = E16Envelope {
        schema_version: "rsia.e16.seed_install.v1".into(),
        id: "unregistered-material".into(),
        namespace: "n".into(),
        owner_actor: "admin".into(),
        request_key: "unregistered-material".into(),
        input_digest: hash(b"synthetic legacy input"),
        created_at: 1,
        updated_at: 1,
        source_refs: sources,
        revoke_watermark: 1,
        payload: SeedInstallPayloadV1 {
            publisher: "rsia.builtin".into(),
            asset_id: "unregistered-i0".into(),
            kind: "improver".into(),
            baseline_blob_digest: hash(BUILTIN_I0_BYTES),
            baseline_digest: hash(BUILTIN_I0_BYTES),
            local_blob_digest: hash(BUILTIN_I0_BYTES),
            local_digest: hash(BUILTIN_I0_BYTES),
            upstream_blob_digest: None,
            upstream_digest: None,
            environment_digest: hash(b"environment"),
            status: SeedStatus::Installed,
            quarantine_reason: None,
        },
    };
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, "artifact", &legacy.id, admin.actor(), &legacy)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let before = shape(&store, &admin, dir.path()).await;
    assert!(
        matches!(PersistentSeedStore::stage_reset_to_baseline(&admin, &store, &legacy.id, "refused-reset", &hash(b"environment"), "compiler-v1").await, Err(Error::Invalid(message)) if message == "seed_catalog_unregistered")
    );
    assert_eq!(shape(&store, &admin, dir.path()).await, before);
}

#[test]
fn catalog_rejects_changed_content_path_blob_and_license() {
    let good: Value = serde_json::from_slice(BUILTIN_SEED_CATALOG_BYTES).unwrap();
    let license = include_bytes!("../../../LICENSE");
    let mutations = [
        ("/schema_version", json!("unknown")),
        ("/entries/0/publisher", json!("third.party")),
        ("/entries/0/asset_id", json!("unknown")),
        ("/entries/0/kind", json!("unknown")),
        ("/entries/0/version", json!(0)),
        ("/entries/0/content_sha256", json!(hash(b"wrong"))),
        ("/entries/0/path", json!("../escape.json")),
        ("/entries/0/path", json!("/absolute.json")),
        ("/entries/0/path", json!("builtin/other.json")),
        ("/entries/0/git_blob", json!("0".repeat(40))),
        ("/entries/0/license/name", json!("MIT")),
        ("/entries/0/license/path", json!("other/LICENSE")),
        ("/entries/0/license/git_blob", json!("0".repeat(40))),
        ("/entries/0/license/sha256", json!(hash(b"wrong"))),
    ];
    let mut all = Vec::new();
    for (pointer, value) in mutations {
        let mut changed = good.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        all.push(serde_json::to_vec(&changed).unwrap());
    }
    let mut duplicate = good.clone();
    duplicate["entries"]
        .as_array_mut()
        .unwrap()
        .push(good["entries"][0].clone());
    all.push(serde_json::to_vec(&duplicate).unwrap());
    let mut missing = good.clone();
    missing["entries"] = json!([]);
    all.push(serde_json::to_vec(&missing).unwrap());
    let mut unknown = good.clone();
    unknown["entries"][0]["unknown"] = json!(true);
    all.push(serde_json::to_vec(&unknown).unwrap());
    let mut missing_field = good.clone();
    missing_field["entries"][0]
        .as_object_mut()
        .unwrap()
        .remove("license");
    all.push(serde_json::to_vec(&missing_field).unwrap());
    all.push(br#"{"schema_version":"rsia.builtin_seed_catalog.v1","schema_version":"rsia.builtin_seed_catalog.v1","entries":[]}"#.to_vec());
    for bytes in all {
        assert!(
            matches!(validate_builtin_seed_catalog(&bytes, BUILTIN_I0_BYTES, license), Err(Error::Invalid(message)) if message == "seed_catalog_mismatch")
        );
    }
    for (content, license) in [
        (b"changed".as_slice(), license.as_slice()),
        (BUILTIN_I0_BYTES, b"changed".as_slice()),
    ] {
        assert!(
            matches!(validate_builtin_seed_catalog(BUILTIN_SEED_CATALOG_BYTES, content, license), Err(Error::Invalid(message)) if message == "seed_catalog_mismatch")
        );
    }
}

#[tokio::test]
async fn catalog_version_change_is_new_u_and_requires_normal_staging() {
    let (_dir, store, admin, sources) = fixture().await;
    let catalog = builtin_seed_catalog().unwrap();
    let entry = &catalog.entries[0];
    let mut old = ImproverContentV2::builtin_default();
    let evo_core::improver::ImproverMechanismV2::ExplorationPolicy { policy } = &mut old.mechanism;
    policy.max_focus_actions = 1;
    let baseline = serde_json::to_vec(&old).unwrap();
    // A user's separately identified managed copy follows the real catalog's
    // current U. This does not invent a historical builtin catalog or authority.
    let initial = InstallCurrentSeedRequest {
        request_key: "version1-copy".into(),
        baseline_identity: Some(SeedIdentity {
            publisher: "org.rsia".into(),
            asset_id: "i0-copy".into(),
            kind: "improver".into(),
        }),
        local_identity: Some(SeedIdentity {
            publisher: "org.rsia".into(),
            asset_id: "i0-copy".into(),
            kind: "improver".into(),
        }),
        local_marked: true,
        baseline_bytes: Some(baseline.clone()),
        local_bytes: Some(baseline.clone()),
        upstream: Some(SeedUpstream {
            path: "managed/version1.json".into(),
            content_digest: hash(&baseline),
            bytes: baseline,
        }),
        environment_digest: hash(b"environment"),
        source_refs: sources,
        compiler_version: "compiler-v1".into(),
        confirmed_bytes: None,
    };
    let installed = PersistentSeedStore::install_current(&admin, &store, initial.clone())
        .await
        .unwrap()
        .install;
    let mut update = initial;
    update.request_key = format!("catalog-version-{}", entry.version);
    update.upstream = Some(SeedUpstream {
        path: entry.path.clone(),
        content_digest: entry.content_sha256.clone(),
        bytes: BUILTIN_I0_BYTES.to_vec(),
    });
    let update = PersistentSeedStore::install_current(&admin, &store, update)
        .await
        .unwrap();
    assert_eq!(update.class, SeedClass::UpstreamNewer);
    assert_eq!(
        fingerprint(&update.install).unwrap(),
        fingerprint(&installed).unwrap()
    );
    let staged = update.staged.unwrap();
    assert_eq!(staged.payload.state, StagedAssetState::Staged);
    assert_eq!(staged.payload.content_blob_digest, entry.content_sha256);
    assert!(staged.payload.candidate_ref.is_none());
    PersistentPackageStore::read_staged(&admin, &store, &staged.id)
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    for kind in ["candidate", "release", "pointer", "receipt", "evaluation"] {
        assert!(
            session
                .list::<Value>(&admin, kind)
                .await
                .unwrap()
                .is_empty()
        );
    }
    session.commit().await.unwrap();
}
