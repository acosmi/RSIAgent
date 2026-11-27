//! AG-166c-reset management controls for `seed.reset_stage` (plan v4.5
//! §6.1.2 L828, §11.1 L1468, E16.3).
//!
//! Real Store, real Admin dispatcher, one Host-written live run and one
//! install labelled with that run's environment digest. The strict wire
//! carries only a request key, the install id and the run id; the current
//! environment is read from the live run, never reported by the caller.
//!
//! Two-tree probes. They use only API that predates the wiring and read every
//! new fact as JSON, so they compile on the parent tree, where the name is
//! registered but not wired and each probe fails with
//! `PARENT-EXPECTED-FAILURE`:
//! - CPP-A (persistent facts): one staged asset row with its edge and its one
//!   audit record, the install byte-for-byte unchanged, and a same-key replay
//!   that returns the original job and writes nothing;
//! - CPP-B (semantics): an install labelled for another environment is
//!   refused with zero business writes.
//!
//! The legacy control is a head-side test only: a legacy install record
//! carries no Current authority, so its reset fails with the fixed conflict
//! code and writes nothing.
//!
//! The head-side management controls pin the entry and the kernel: the Admin
//! gate precedes the payload, the wire is flat and strict, the kernel refuses
//! with zero business writes whatever is missing or foreign, a revoked source
//! blocks the request at submit, a request key freezes the run id, the
//! staging limit admits the management path too, and the staged reset is
//! closed by `asset.abort` and by nothing else.
#[path = "support/seed_reset.rs"]
mod seed_reset;

use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::dispatch::ManagementDispatcher;
use evo_engine::packages::E16Envelope;
use evo_engine::seeds::{
    E16_SEED_INSTALL_SCHEMA, PersistentSeedStore, SeedInstallPayloadV1, SeedStatus,
};
use seed_reset::{BASELINE, COMPILER_VERSION, Fixture, NS, RUN_ID, reset_payload, staged_id};
use serde_json::{Value, json};

#[tokio::test]
async fn cpp_a_seed_reset_stage_commits_one_staged_asset_and_replays_the_same_job() {
    let fixture = Fixture::new().await;
    let install_id = fixture.install_id.clone();
    let staged_id = staged_id("cpp-a");
    let install_body = fixture.raw_body("artifact", &install_id).await;
    let before = fixture.business_shape().await;
    let status = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("cpp-a", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(status["state"], json!("succeeded"), "{status}");
    assert_eq!(status["step"], json!("seed_reset_staged"));
    assert_eq!(status["error_code"], Value::Null);
    // The result names the install, the staged asset and B; nothing else.
    let result = &status["result"];
    assert_eq!(result["result"], json!("seed_reset_staged"));
    assert_eq!(result["install_id"], json!(install_id));
    assert_eq!(result["staged_asset_id"], json!(staged_id));
    assert_eq!(result["baseline_digest"], json!(fixture.baseline_digest));
    let mut keys: Vec<&str> = result
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["baseline_digest", "install_id", "result", "staged_asset_id"]
    );
    assert_eq!(
        status["committed_side_effects"][0]["kind"],
        json!("seed_reset_staged")
    );

    // The one new business object is the staged asset: state `staged`, the
    // reset package schema, B as its content, and only the digests of L, U
    // and the environment.
    let staged = fixture.get("artifact", &staged_id).await.unwrap();
    assert_eq!(staged["schema_version"], json!("rsia.e16.staged_asset.v1"));
    assert_eq!(staged["id"], json!(staged_id));
    assert_eq!(staged["namespace"], json!(NS));
    assert_eq!(staged["owner_actor"], json!("admin"));
    assert_eq!(staged["request_key"], json!("cpp-a"));
    assert_eq!(staged["revoke_watermark"], json!(1));
    assert_eq!(staged["payload"]["state"], json!("staged"));
    assert_eq!(
        staged["payload"]["package_schema_version"],
        json!("rsia.seed.reset.v1")
    );
    assert_eq!(
        staged["payload"]["content_blob_digest"],
        json!(fixture.baseline_blob_digest)
    );
    assert_eq!(staged["payload"]["content_bytes"], json!(BASELINE.len()));
    assert_eq!(
        staged["payload"]["baseline_digest"],
        json!(fixture.baseline_digest)
    );
    assert_eq!(
        staged["payload"]["local_digest"],
        json!(fixture.local_digest)
    );
    assert_eq!(
        staged["payload"]["upstream_digest"],
        json!(fixture.upstream_digest)
    );
    assert_eq!(
        staged["payload"]["environment_digest"],
        json!(fixture.run_environment_digest)
    );
    assert_eq!(
        staged["payload"]["compiler_version"],
        json!(COMPILER_VERSION)
    );
    assert_eq!(staged["payload"]["candidate_ref"], Value::Null);
    assert_eq!(staged["payload"]["candidate_material"], Value::Null);
    // Its source closure is the install's own sources plus the install.
    let install = fixture.get("artifact", &install_id).await.unwrap();
    let mut sources = install["source_refs"].as_array().unwrap().clone();
    sources.push(json!({
        "kind": "artifact",
        "id": install_id,
        "digest": fingerprint(&install).unwrap(),
    }));
    sources.sort_by(|left, right| {
        (left["kind"].as_str(), left["id"].as_str())
            .cmp(&(right["kind"].as_str(), right["id"].as_str()))
    });
    assert_eq!(staged["source_refs"], json!(sources));
    assert_eq!(
        fixture.dependents("artifact", &install_id).await,
        vec![("artifact".to_owned(), staged_id.clone())],
        "the staged asset depends on the install it resets"
    );
    // Exactly one audit record of the staging.
    let staging_audits: Vec<_> = fixture
        .business_audit()
        .into_iter()
        .filter(|(operation, entity)| operation == "e16.seed_reset.stage" && entity == &staged_id)
        .collect();
    assert_eq!(staging_audits.len(), 1);
    // The install record is byte-for-byte unchanged.
    assert_eq!(
        fixture.raw_body("artifact", &install_id).await,
        install_body
    );
    // No pointer, release, candidate, evaluation, budget or ticket row was
    // added: the staged asset is the only new object, and no file was
    // written: the reset publishes no blob.
    let after = fixture.business_shape().await;
    let old_objects = before["objects"].as_array().unwrap();
    let new_objects: Vec<&Value> = after["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| !old_objects.contains(row))
        .collect();
    assert_eq!(new_objects.len(), 1, "{new_objects:?}");
    assert_eq!(new_objects[0][0], json!("artifact"));
    assert_eq!(new_objects[0][1], json!(staged_id));
    assert_eq!(after["files"], before["files"]);
    assert_eq!(fixture.staged_packages().await, 1);

    // The same key and payload replay the original job: nothing changes.
    let replay = fixture
        .submit(
            "seed.reset_stage",
            reset_payload("cpp-a", &install_id, RUN_ID),
        )
        .await
        .unwrap();
    assert_eq!(replay.identity().id, status["id"].as_str().unwrap());
    assert_eq!(fixture.business_shape().await, after);
    assert_eq!(fixture.staged_packages().await, 1);
    fixture.store.close().await;
}

/// An install carries the environment label it was stored with. A reset must
/// refuse an install whose label is not the live run's environment, whatever
/// the request says: zero staged row, no staging audit, the install and every
/// blob file unchanged.
#[tokio::test]
async fn cpp_b_seed_reset_stage_refuses_an_install_labelled_for_another_environment() {
    let other = hash(b"another environment");
    let fixture = Fixture::installed(Some(&other)).await;
    assert_ne!(
        fixture.install_environment_digest,
        fixture.run_environment_digest
    );
    let install_body = fixture.raw_body("artifact", &fixture.install_id).await;
    let before = fixture.business_shape().await;
    let status = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("cpp-b", &fixture.install_id, RUN_ID),
        )
        .await;
    assert_eq!(status["state"], json!("failed"), "{status}");
    assert_eq!(status["error_code"], json!("conflict"));
    assert_eq!(status["result"], Value::Null);
    assert_eq!(status["committed_side_effects"], json!([]), "{status}");
    assert_eq!(fixture.business_shape().await, before);
    assert!(
        !fixture
            .business_audit()
            .iter()
            .any(|(operation, _)| operation == "e16.seed_reset.stage")
    );
    assert_eq!(
        fixture.raw_body("artifact", &fixture.install_id).await,
        install_body
    );
    assert_eq!(fixture.staged_packages().await, 0);
    fixture.store.close().await;
}

/// A legacy install record carries no Current authority. A well-formed reset
/// request for it is refused before the first write: the job fails with the
/// fixed conflict code, no staged row and no staging audit exist, and the
/// legacy record is byte-for-byte unchanged.
#[tokio::test]
async fn a_legacy_install_confers_no_reset_and_writes_nothing() {
    let fixture = Fixture::new().await;
    let legacy_id = "e16-seed-install-legacy";
    let baseline = b"legacy baseline";
    let baseline_blob = fixture
        .store
        .put_blob_bounded(&fixture.admin, baseline, 1024 * 1024)
        .await
        .unwrap();
    let current = fixture.get("artifact", &fixture.install_id).await.unwrap();
    let legacy = E16Envelope {
        schema_version: E16_SEED_INSTALL_SCHEMA.into(),
        id: legacy_id.into(),
        namespace: NS.into(),
        owner_actor: fixture.admin.actor().into(),
        request_key: "legacy-install".into(),
        input_digest: hash(b"legacy-input"),
        created_at: 1,
        updated_at: 1,
        source_refs: serde_json::from_value(current["source_refs"].clone()).unwrap(),
        revoke_watermark: 1,
        payload: SeedInstallPayloadV1 {
            publisher: "org.rsia".into(),
            asset_id: "legacy-seed".into(),
            kind: "skill".into(),
            baseline_blob_digest: baseline_blob.clone(),
            baseline_digest: hash(baseline),
            local_blob_digest: baseline_blob,
            local_digest: hash(baseline),
            upstream_blob_digest: None,
            upstream_digest: None,
            environment_digest: fixture.run_environment_digest.clone(),
            status: SeedStatus::Installed,
            quarantine_reason: None,
        },
    };
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            legacy_id,
            fixture.admin.actor(),
            &legacy,
        )
        .await
        .unwrap();
    for source in &legacy.source_refs {
        session
            .put_edge(
                &fixture.admin,
                "artifact",
                legacy_id,
                &source.kind,
                &source.id,
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let legacy_body = fixture.raw_body("artifact", legacy_id).await;
    let before = fixture.business_shape().await;
    let status = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("legacy-reset", legacy_id, RUN_ID),
        )
        .await;
    assert_eq!(status["state"], json!("failed"), "{status}");
    assert_eq!(status["error_code"], json!("conflict"));
    assert_eq!(status["result"], Value::Null);
    assert_eq!(status["committed_side_effects"], json!([]), "{status}");
    assert_eq!(fixture.business_shape().await, before);
    assert!(
        !fixture
            .business_audit()
            .iter()
            .any(|(operation, _)| operation == "e16.seed_reset.stage")
    );
    assert_eq!(fixture.raw_body("artifact", legacy_id).await, legacy_body);
    assert_eq!(
        fixture.get("artifact", &staged_id("legacy-reset")).await,
        None
    );
    assert_eq!(fixture.staged_packages().await, 0);
    fixture.store.close().await;
}

/// The role gate comes first: a non-Admin surface is refused before the
/// payload is parsed, whatever the payload is, and without acquiring the
/// connection the held Session occupies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seed_reset_stage_requires_admin_before_the_payload_is_observed() {
    let fixture = Fixture::new().await;
    let evaluator = Context::new(NS, "evaluator", Role::Evaluator).unwrap();
    let dispatcher = ManagementDispatcher::new(
        fixture.store.clone(),
        vec![fixture.admin.clone(), evaluator.clone()],
    )
    .unwrap();
    let blocker = fixture.store.session().await.unwrap();
    for payload in [Value::Null, json!(["not a business DTO"])] {
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            dispatcher.submit(&evaluator, "seed.reset_stage", payload),
        )
        .await
        .expect("the role gate must not acquire the held connection")
        .unwrap_err();
        assert!(matches!(error, Error::Forbidden), "{error:?}");
    }
    blocker.commit().await.unwrap();
    // A well-formed payload does not help a non-Admin surface, and a context
    // the dispatcher was not given is refused before its role is read.
    let well_formed = reset_payload("role-first", &fixture.install_id, RUN_ID);
    assert!(matches!(
        dispatcher
            .submit(&evaluator, "seed.reset_stage", well_formed)
            .await
            .unwrap_err(),
        Error::Forbidden
    ));
    let agent = Context::new(NS, "agent", Role::Agent).unwrap();
    assert!(matches!(
        dispatcher
            .submit(&agent, "seed.reset_stage", Value::Null)
            .await
            .unwrap_err(),
        Error::Forbidden
    ));
    assert_eq!(fixture.job_count().await, 0);
    assert_eq!(fixture.staged_packages().await, 0);
    fixture.store.close().await;
}

/// The wire is flat and strict: a request key, an install id and a run id.
/// An environment or compiler label, a path, a root, a role, an owner, a
/// namespace or content is refused before any session, and so is a missing
/// field, a foreign schema and an id or key that is not an identifier.
#[tokio::test]
async fn seed_reset_stage_wire_is_flat_and_strict() {
    let fixture = Fixture::new().await;
    let well_formed = reset_payload("strict", &fixture.install_id, RUN_ID);
    let before = fixture.business_shape().await;
    const NOT_A_DTO: &str = "invalid management payload";
    const NOT_AN_IDENTIFIER: &str = "invalid identifier";
    const EMPTY_IDENTIFIER: &str = "identifier: expected nonempty text <= 128 bytes";
    let mut refused = Vec::new();
    for (field, value) in [
        ("environment_digest", json!(fixture.run_environment_digest)),
        ("compiler_version", json!(COMPILER_VERSION)),
        ("path", json!("seeds/baseline.txt")),
        ("root", json!("seed-root")),
        ("role", json!("admin")),
        ("owner", json!("admin")),
        ("namespace", json!(NS)),
        ("actor", json!("admin")),
        ("content", json!("baseline bytes")),
        ("baseline_digest", json!(fixture.baseline_digest)),
        ("created_at", json!(1)),
    ] {
        let mut payload = well_formed.clone();
        payload[field] = value;
        refused.push((format!("extra {field}"), payload, NOT_A_DTO));
    }
    for field in ["request_key", "install_id", "run_id"] {
        let mut payload = well_formed.clone();
        payload.as_object_mut().unwrap().remove(field);
        refused.push((format!("missing {field}"), payload, NOT_A_DTO));
    }
    for (field, value, code) in [
        ("request_key", json!(""), EMPTY_IDENTIFIER),
        ("install_id", json!("../install"), NOT_AN_IDENTIFIER),
        ("run_id", json!("run/other"), NOT_AN_IDENTIFIER),
    ] {
        let mut payload = well_formed.clone();
        payload[field] = value;
        refused.push((format!("{field}: {value}"), payload, code));
    }
    for (what, payload, code) in refused {
        let error = fixture
            .submit("seed.reset_stage", payload)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Invalid(message) if message == code),
            "{what}: {error:?}"
        );
        assert_eq!(fixture.job_count().await, 0, "{what}");
    }
    // The schema is named by the operation and cannot be absent or borrowed
    // from another operation.
    for schema in [Value::Null, json!("rsia.management.seed_install.v1")] {
        let mut payload = well_formed.clone();
        payload["schema_version"] = schema;
        let error = fixture
            .submit("seed.reset_stage", payload)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Invalid(message)
                if message == "unsupported management payload schema"),
            "{error:?}"
        );
    }
    let mut schemaless = well_formed.clone();
    schemaless.as_object_mut().unwrap().remove("schema_version");
    assert!(matches!(
        fixture.submit("seed.reset_stage", schemaless).await.unwrap_err(),
        Error::Invalid(ref message) if message == "unsupported management payload schema"
    ));
    assert_eq!(fixture.job_count().await, 0);
    assert_eq!(
        fixture.business_shape().await,
        before,
        "no refused request wrote anything"
    );
    assert!(!fixture.has_staging_audit());
    fixture.store.close().await;
}

/// One refused reset: the job fails with the fixed code, no staged row and
/// no staging audit exist, and the business shape is unchanged.
async fn assert_refused_reset(
    fixture: &Fixture,
    key: &str,
    install_id: &str,
    run_id: &str,
    code: &str,
) {
    let before = fixture.business_shape().await;
    let status = fixture
        .run_job("seed.reset_stage", reset_payload(key, install_id, run_id))
        .await;
    assert_eq!(status["state"], json!("failed"), "{status}");
    assert_eq!(status["error_code"], json!(code), "{status}");
    assert_eq!(status["result"], Value::Null);
    assert_eq!(status["committed_side_effects"], json!([]), "{status}");
    assert_eq!(
        fixture.business_shape().await,
        before,
        "zero business writes"
    );
    assert!(!fixture.has_staging_audit());
    assert_eq!(fixture.get("artifact", &staged_id(key)).await, None);
}

/// The request names a run and an install; neither is taken on faith. A run
/// that does not exist supplies no current environment, and an install that
/// does not exist supplies no B: each fails the job with `not_found` before
/// the first write.
#[tokio::test]
async fn seed_reset_stage_refuses_a_missing_run_or_install() {
    let fixture = Fixture::new().await;
    let install_id = fixture.install_id.clone();
    assert_refused_reset(
        &fixture,
        "missing-run",
        &install_id,
        "no-such-run",
        "not_found",
    )
    .await;
    assert_refused_reset(
        &fixture,
        "missing-install",
        "e16-seed-install-missing",
        RUN_ID,
        "not_found",
    )
    .await;
    assert_eq!(fixture.staged_packages().await, 0);
    fixture.store.close().await;
}

/// The install must be the caller's own record: another Admin of the
/// namespace is refused with the conflict code, and the install is
/// byte-for-byte unchanged.
#[tokio::test]
async fn seed_reset_stage_refuses_an_install_of_another_owner() {
    let fixture = Fixture::new().await;
    let other = Context::new(NS, "admin-two", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(fixture.store.clone(), vec![other.clone()]).unwrap();
    let install_body = fixture.raw_body("artifact", &fixture.install_id).await;
    let before = fixture.business_shape().await;
    let queued = dispatcher
        .submit(
            &other,
            "seed.reset_stage",
            reset_payload("other-owner", &fixture.install_id, RUN_ID),
        )
        .await
        .unwrap();
    let status = fixture
        .terminal_for(&dispatcher, &other, &queued.identity().id)
        .await;
    assert_eq!(status["state"], json!("failed"), "{status}");
    assert_eq!(status["error_code"], json!("conflict"));
    assert_eq!(status["owner_actor"], json!("admin-two"));
    assert_eq!(status["result"], Value::Null);
    assert_eq!(
        fixture.business_shape().await,
        before,
        "zero business writes"
    );
    assert!(!fixture.has_staging_audit());
    assert_eq!(
        fixture.raw_body("artifact", &fixture.install_id).await,
        install_body
    );
    assert_eq!(
        fixture.get("artifact", &staged_id("other-owner")).await,
        None
    );
    fixture.store.close().await;
}

/// Only an Installed record is a resettable image. An install record that is
/// still Prepared, as an interrupted installation leaves it, is refused with
/// the conflict code and nothing is staged.
#[tokio::test]
async fn seed_reset_stage_refuses_an_install_that_is_not_installed() {
    let fixture = Fixture::new().await;
    let mut install = fixture.get("artifact", &fixture.install_id).await.unwrap();
    assert_eq!(install["payload"]["status"], json!("installed"));
    install["payload"]["status"] = json!("prepared");
    fixture.put("artifact", &fixture.install_id, &install).await;
    assert_refused_reset(
        &fixture,
        "not-installed",
        &fixture.install_id,
        RUN_ID,
        "conflict",
    )
    .await;
    assert_eq!(fixture.staged_packages().await, 0);
    fixture.store.close().await;
}

/// A revoked source blocks the reset at submit (V075, V078): the request
/// depends on the install and the install on its source run, so the submit
/// check refuses the request before its first write, with no job at all. The
/// same revocation blocks the public API.
#[tokio::test]
async fn seed_reset_stage_is_refused_after_the_install_source_is_revoked() {
    let fixture = Fixture::new().await;
    fixture.revoke_install_source().await;
    let before = fixture.business_shape().await;
    let jobs_before = fixture.job_count().await;
    let error = fixture
        .submit(
            "seed.reset_stage",
            reset_payload("after-revoke", &fixture.install_id, RUN_ID),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Conflict(message)
            if message.starts_with("management request") && message.contains("run seed-source")),
        "{error:?}"
    );
    assert_eq!(fixture.job_count().await, jobs_before, "no job was created");
    assert!(
        PersistentSeedStore::stage_reset_to_baseline(
            &fixture.admin,
            &fixture.store,
            &fixture.install_id,
            "after-revoke-public",
            &fixture.run_environment_digest,
            COMPILER_VERSION,
        )
        .await
        .is_err(),
        "the public API is blocked by the same revocation"
    );
    assert_eq!(fixture.business_shape().await, before);
    assert!(!fixture.has_staging_audit());
    assert_eq!(fixture.staged_packages().await, 0);
    fixture.store.close().await;
}

/// One request key freezes one intent (§6.1.2): the same key with the same
/// wire replays the original job and writes nothing, the same key with
/// another run id is refused at submit, and a new key stages a second asset
/// of the same install.
#[tokio::test]
async fn seed_reset_stage_replays_a_key_and_refuses_another_run_under_it() {
    let fixture = Fixture::new().await;
    let install_id = fixture.install_id.clone();
    let first = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("one-key", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(first["state"], json!("succeeded"), "{first}");
    let first_id = first["id"].as_str().unwrap().to_owned();
    let before = fixture.business_shape().await;

    let replay = fixture
        .submit(
            "seed.reset_stage",
            reset_payload("one-key", &install_id, RUN_ID),
        )
        .await
        .unwrap();
    assert_eq!(replay.identity().id, first_id);
    assert_eq!(fixture.business_shape().await, before);

    let error = fixture
        .submit(
            "seed.reset_stage",
            reset_payload("one-key", &install_id, "another-run"),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "idempotency key reused with different content"),
        "{error:?}"
    );
    assert_eq!(
        fixture.job_count().await,
        1,
        "the refused request created no job"
    );
    assert_eq!(fixture.business_shape().await, before);

    let second = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("two-key", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(second["state"], json!("succeeded"), "{second}");
    assert_ne!(second["id"], first["id"]);
    assert_eq!(
        second["result"]["staged_asset_id"],
        json!(staged_id("two-key"))
    );
    assert_ne!(
        second["result"]["staged_asset_id"],
        first["result"]["staged_asset_id"]
    );
    let asset = fixture
        .get("artifact", &staged_id("two-key"))
        .await
        .unwrap();
    assert_eq!(asset["payload"]["state"], json!("staged"));
    assert_eq!(asset["request_key"], json!("two-key"));
    assert_eq!(fixture.staged_packages().await, 2);
    fixture.store.close().await;
}

/// The management path passes the same staging limit as the public one
/// (§3.3.1). Twenty reset jobs fill it; a new key is then refused with the
/// conflict code and writes nothing, and at the limit the replay of an
/// existing key is served from its original job and writes nothing either.
#[tokio::test]
async fn seed_reset_stage_obeys_the_staging_limit() {
    let fixture = Fixture::new().await;
    let install_id = fixture.install_id.clone();
    let mut fill_zero_job = String::new();
    for index in 0..20 {
        let status = fixture
            .run_job(
                "seed.reset_stage",
                reset_payload(&format!("fill-{index}"), &install_id, RUN_ID),
            )
            .await;
        assert_eq!(
            status["state"],
            json!("succeeded"),
            "fill-{index}: {status}"
        );
        if index == 0 {
            fill_zero_job = status["id"].as_str().unwrap().to_owned();
        }
    }
    assert_eq!(fixture.staged_packages().await, 20);
    let before = fixture.business_shape().await;

    let refused = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("fill-20", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(refused["state"], json!("failed"), "{refused}");
    assert_eq!(refused["error_code"], json!("conflict"));
    assert_eq!(refused["result"], Value::Null);
    assert_eq!(fixture.get("artifact", &staged_id("fill-20")).await, None);
    assert_eq!(fixture.staged_packages().await, 20);
    assert_eq!(
        fixture.business_shape().await,
        before,
        "the refused 21st staging wrote nothing"
    );

    // At the limit the same key is still the original job, not a new request.
    let replay = fixture
        .submit(
            "seed.reset_stage",
            reset_payload("fill-0", &install_id, RUN_ID),
        )
        .await
        .unwrap();
    assert_eq!(replay.identity().id, fill_zero_job);
    assert_eq!(fixture.business_shape().await, before);
    // The replay branch of the kernel precedes the admission for a management
    // product too: the public API, with the same key, returns the asset the
    // job wrote instead of the capacity conflict (§3.3.2).
    let replayed = PersistentSeedStore::stage_reset_to_baseline(
        &fixture.admin,
        &fixture.store,
        &install_id,
        "fill-0",
        &fixture.run_environment_digest,
        COMPILER_VERSION,
    )
    .await
    .unwrap();
    assert_eq!(replayed.id, staged_id("fill-0"));
    assert_eq!(fixture.business_shape().await, before);
    assert_eq!(fixture.staged_packages().await, 20);
    fixture.store.close().await;
}

/// The staged reset is an unactivated staging (E16.3): rollback only closes
/// it, through `asset.abort`, and never touches the install. It is not a
/// validated package projection, so `asset.handoff` and `export.prepare`
/// refuse it and write nothing.
#[tokio::test]
async fn a_staged_reset_is_closed_by_asset_abort_and_by_nothing_else() {
    let fixture = Fixture::new().await;
    let install_id = fixture.install_id.clone();
    let staged = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("abort-me", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(staged["state"], json!("succeeded"), "{staged}");
    let staged_id = staged_id("abort-me");
    let before = fixture.business_shape().await;
    for (operation, payload) in [
        (
            "asset.handoff",
            json!({
                "schema_version": "rsia.management.asset_handoff.v1",
                "request_key": "handoff",
                "staged_asset_id": staged_id,
                "candidate_id": "no-such-candidate",
            }),
        ),
        (
            "export.prepare",
            json!({
                "schema_version": "rsia.management.export_prepare.v1",
                "request_key": "export",
                "staged_asset_id": staged_id,
            }),
        ),
    ] {
        let refused = fixture.run_job(operation, payload).await;
        assert_eq!(refused["state"], json!("failed"), "{operation}: {refused}");
        assert_eq!(refused["error_code"], json!("invalid_input"));
        assert_eq!(refused["result"], Value::Null);
    }
    assert_eq!(
        fixture.business_shape().await,
        before,
        "neither the handoff nor the export wrote anything"
    );

    let aborted = fixture
        .run_job(
            "asset.abort",
            json!({
                "schema_version": "rsia.management.asset_abort.v1",
                "request_key": "abort",
                "staged_asset_id": staged_id,
            }),
        )
        .await;
    assert_eq!(aborted["state"], json!("succeeded"), "{aborted}");
    assert_eq!(aborted["step"], json!("asset_aborted"));
    assert_eq!(aborted["result"]["result"], json!("asset_aborted"));
    assert_eq!(aborted["result"]["state"], json!("aborted"));
    let asset = fixture.get("artifact", &staged_id).await.unwrap();
    assert_eq!(asset["payload"]["state"], json!("aborted"));
    assert_eq!(fixture.audit_count("e16.staged_asset.abort", &staged_id), 1);
    // Against the shape before the abort: the same number of objects with the
    // staged asset as the only changed row, one more business audit record,
    // and no file touched. The install keeps its edge to the closed asset:
    // closing an unactivated update does not overwrite the local copy.
    assert_eq!(
        fixture.dependents("artifact", &install_id).await,
        vec![("artifact".to_owned(), staged_id)]
    );
    let after = fixture.business_shape().await;
    let old_objects = before["objects"].as_array().unwrap();
    let objects = after["objects"].as_array().unwrap();
    assert_eq!(objects.len(), old_objects.len());
    let changed: Vec<&Value> = objects
        .iter()
        .filter(|row| !old_objects.contains(row))
        .collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0][1], json!(staged_id));
    let mut expected_audit = before["audit"].as_array().unwrap().clone();
    expected_audit.push(json!(["e16.staged_asset.abort", staged_id]));
    assert_eq!(after["audit"], json!(expected_audit));
    assert_eq!(after["files"], before["files"]);
    fixture.store.close().await;
}

fn sorted_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    keys
}

/// The result and the committed effect carry identifiers and digests only:
/// the install id, the staged asset id and B's digest. The run, the
/// environment label, the compiler version, the content, a path and any fee
/// field are not part of what a caller gets back.
#[tokio::test]
async fn seed_reset_stage_result_and_effect_expose_identifiers_only() {
    let fixture = Fixture::new().await;
    let install_id = fixture.install_id.clone();
    let status = fixture
        .run_job(
            "seed.reset_stage",
            reset_payload("exposed", &install_id, RUN_ID),
        )
        .await;
    assert_eq!(status["state"], json!("succeeded"), "{status}");
    let result = &status["result"];
    assert_eq!(
        sorted_keys(result),
        ["baseline_digest", "install_id", "result", "staged_asset_id"]
    );
    let effects = status["committed_side_effects"].as_array().unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(
        sorted_keys(&effects[0]),
        ["baseline_digest", "install_id", "kind", "staged_asset_id"]
    );
    let exposed = json!([result, effects]).to_string();
    assert!(!exposed.contains(RUN_ID), "{exposed}");
    assert!(
        !exposed.contains(&fixture.run_environment_digest),
        "the environment label is not returned"
    );
    assert!(!exposed.contains(COMPILER_VERSION), "{exposed}");
    assert!(
        !exposed.contains(std::str::from_utf8(BASELINE).unwrap()),
        "no baseline content is returned"
    );
    fixture.store.close().await;
}
