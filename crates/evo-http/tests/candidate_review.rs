//! The first two tests are the actual existing-API parent positive control and
//! route semantic red. Their original parent run is separately preserved.
use evo_core::contract::{
    CapabilityLevel, CompileParts, HostCapabilities, ImproverPatch, Profile, SkillPatch,
    SkillSnapshot, SystemSnapshot, compile_bundle,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{
    ReleaseCandidateRecord, ReleaseStore, StageBundleRequest, TypedSourceRef,
};
use evo_engine::service::{HostPrepareConfig, HostService};
use evo_http::{AuthIdentity, AuthRegistry, HttpState, router};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallRef, BudgetCallReservation, BudgetStage, RootBudgetAuthorization,
};
use evo_storage::lifecycle::{REVOKE_TOMBSTONE_SCHEMA, RevokeTombstone};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const NAMESPACE: &str = "review-fixture";
const ADMIN_TOKEN: &str = "review-admin-fixture-token-0123456789";
const CANDIDATE_ID: &str = "candidate-review";
const PROFILE_ID: &str = "profile-review";

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn context(actor: &str, role: Role) -> Context {
    Context::new(NAMESPACE, actor, role).unwrap()
}

fn source(id: &str, body: &str) -> StoredTraceAuthority {
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.as_bytes().to_vec(),
            parent_family: format!("family-{id}"),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: format!("family-{id}"),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.into(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    }
}

async fn fixture() -> (tempfile::TempDir, HttpState) {
    let guard = tempfile::tempdir().unwrap();
    let store = Store::open(&guard.path().join("review-fixture.sqlite3"))
        .await
        .unwrap();
    let host = context("gateway-host", Role::Host);
    let proposer = context("proposer", Role::Worker);
    let admin = context("admin", Role::Admin);
    let sources = [
        source("review-run-first", "first trusted review fixture"),
        source("review-run-second", "second trusted review fixture"),
    ];
    for source in &sources {
        store_trace_authority(&store, &host, source).await.unwrap();
    }
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .bump_watermark(&host, &d("review-watermark"))
            .await
            .unwrap(),
        1
    );
    session.commit().await.unwrap();

    let profile = Profile {
        id: PROFILE_ID.into(),
        evolution_enabled: true,
        parent_digest: d("parent"),
        baseline_digest: d("baseline"),
    };
    let parent = SkillSnapshot {
        content: "Complete stored review rule. 中文🙂".into(),
        applicability: "Relevant stored review fixture.".into(),
        counterexample: "Do not infer approval from the candidate.".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    let caps = HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    };
    // Fixture setup only. The review operation itself must never compile.
    let bundle = compile_bundle(CompileParts {
        profile: &profile,
        parent: &parent,
        baseline: &SkillSnapshot::empty(),
        parent_strategy: &Strategy::default(),
        baseline_strategy: &Strategy::default(),
        skill_patch: &SkillPatch::default(),
        improver_patch: &ImproverPatch::default(),
        caps: &caps,
        revoked: &BTreeSet::new(),
    })
    .unwrap();
    let system_snapshot = SystemSnapshot {
        schema_version: "rsia.system_snapshot.v2".into(),
        profile_id: PROFILE_ID.into(),
        host_id: "reference-host".into(),
        host_version: "1.0.0".into(),
        model_id: "disabled".into(),
        tools: vec![],
        mandatory_context_digest: d("mandatory"),
    };
    let staged = ReleaseStore::stage_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: CANDIDATE_ID.into(),
            bundle,
            environment_digest: system_snapshot.digest().unwrap(),
            proposer_actor: proposer.actor().into(),
            sources: sources
                .iter()
                .map(|source| TypedSourceRef {
                    kind: "run".into(),
                    id: source.record.id.clone(),
                    content_digest: source.trace.source_digest.clone(),
                })
                .collect(),
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    let persisted: ReleaseCandidateRecord = session
        .need(&admin, "artifact", CANDIDATE_ID)
        .await
        .unwrap();
    assert_eq!(
        evo_core::fingerprint(&staged).unwrap(),
        evo_core::fingerprint(&persisted).unwrap()
    );
    session.commit().await.unwrap();
    let auth = AuthRegistry::from_plaintext(vec![(
        ADMIN_TOKEN.into(),
        AuthIdentity::new(NAMESPACE, "admin", Role::Admin).unwrap(),
    )])
    .unwrap();
    let service = HostService::new(store, host).unwrap();
    let management = evo_engine::dispatch::ManagementDispatcher::new(
        service.store().clone(),
        auth.management_contexts().unwrap(),
    )
    .unwrap();
    (
        guard,
        HttpState {
            service,
            prepare_config: HostPrepareConfig {
                profile_id: PROFILE_ID.into(),
                system_snapshot,
                host_surface_id: "unused-by-read-only-review".into(),
                host_capabilities: caps,
                evolution_enabled: true,
                capability_level: CapabilityLevel::ToolOnly,
            },
            auth,
            management,
        },
    )
}

async fn spawn(state: HttpState) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn parent_existing_inspect_json_is_unchanged_positive_control() {
    let (_guard, state) = fixture().await;
    let (base, task) = spawn(state).await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/tools/inspect"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&json!({"kind":"run","id":"review-run-first"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let actual: Value = response.json().await.unwrap();
    assert_eq!(
        actual,
        json!({
            "kind":"run", "id":"review-run-first", "authority":"trusted_host",
            "usable_as_trusted_source":true, "parent_family":"family-review-run-first",
            "task_origin":"trusted_run", "execution_attestation":"trusted_host",
            "purpose":"development", "source_digest":d("first trusted review fixture"),
            "outcome":"success"
        })
    );
    task.abort();
}

#[tokio::test]
async fn parent_raw_http_stored_candidate_review_requires_new_route_semantic_red() {
    let (_guard, state) = fixture().await;
    let (base, task) = spawn(state).await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/review"))
        .bearer_auth(ADMIN_TOKEN)
        .header("content-type", "application/json")
        .body(r#"{"kind":"typed_candidate","id":"candidate-review","detail":"metadata"}"#)
        .send()
        .await
        .unwrap();
    // Reached only after successful fixture setup and real loopback HTTP.
    assert_eq!(response.status(), StatusCode::OK);
    let actual: Value = response.json().await.unwrap();
    assert_eq!(actual["schema_version"], "rsia.review.v1");
    assert_eq!(actual["kind"], "typed_candidate");
    assert_eq!(actual["diff_complete"], false);
    task.abort();
}

const REVIEW_BODY: &str =
    r#"{"kind":"typed_candidate","id":"candidate-review","detail":"metadata"}"#;

async fn send(base: &str, token: Option<&str>, body: &str) -> (StatusCode, Value) {
    let mut request = reqwest::Client::new()
        .post(format!("{base}/v1/review"))
        .header("content-type", "application/json")
        .body(body.to_owned());
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap())
}

fn error(code: &str) -> Value {
    json!({"error":{"code":code}})
}

#[tokio::test]
async fn authenticated_non_admin_and_cross_namespace_are_hidden_before_any_dto_parsing() {
    let (_guard, mut state) = fixture().await;
    let mut identities = vec![(
        ADMIN_TOKEN.into(),
        AuthIdentity::new(NAMESPACE, "admin", Role::Admin).unwrap(),
    )];
    let mut rejected_tokens = Vec::new();
    for role in [Role::Agent, Role::Worker, Role::Host, Role::Evaluator] {
        let token = format!("review-role-{role:?}-fixture-token");
        identities.push((
            token.clone(),
            AuthIdentity::new(NAMESPACE, "other-role", role).unwrap(),
        ));
        rejected_tokens.push(token);
    }
    let cross = "review-cross-namespace-admin-fixture-token";
    identities.push((
        cross.into(),
        AuthIdentity::new("another-namespace", "admin", Role::Admin).unwrap(),
    ));
    rejected_tokens.push(cross.into());
    state.auth = AuthRegistry::from_plaintext(identities).unwrap();
    let (base, task) = spawn(state).await;
    for token in &rejected_tokens {
        for body in [
            REVIEW_BODY,
            "{",
            "null",
            r#"{"unknown":"private","namespace":"another-namespace"}"#,
            r#"{"kind":"typed_candidate","id":"never-present"}"#,
            r#"{"kind":"typed_candidate","kind":"formal_report","id":"secret-id"}"#,
        ] {
            assert_eq!(
                send(&base, Some(token), body).await,
                (StatusCode::NOT_FOUND, error("review_unavailable"))
            );
        }
    }
    // The trusted Admin's malformed DTO is still rejected as an input error.
    assert_eq!(
        send(&base, Some(ADMIN_TOKEN), "{").await,
        (StatusCode::BAD_REQUEST, error("invalid_review_payload"))
    );
    task.abort();
}

#[tokio::test]
async fn existing_authentication_codes_and_identity_guards_are_preserved() {
    let (_guard, mut state) = fixture().await;
    let (base, task) = spawn(state.clone()).await;
    assert_eq!(
        send(&base, None, "{").await,
        (StatusCode::UNAUTHORIZED, error("bearer_token_required"))
    );
    assert_eq!(
        send(&base, Some("incorrect-fixture-token-00000000"), "{").await,
        (StatusCode::UNAUTHORIZED, error("invalid_bearer_token"))
    );
    for header in ["x-role", "x-namespace", "x-actor"] {
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/review"))
            .bearer_auth(ADMIN_TOKEN)
            .header(header, "forged")
            .body(REVIEW_BODY)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            error("identity_headers_forbidden")
        );
    }
    for key in ["role", "namespace", "actor"] {
        let body = json!({"kind":"typed_candidate","id":CANDIDATE_ID,(key):"forged"}).to_string();
        assert_eq!(
            send(&base, Some(ADMIN_TOKEN), &body).await,
            (StatusCode::BAD_REQUEST, error("invalid_review_payload"))
        );
    }
    task.abort();
    state.auth = AuthRegistry::default();
    let (base, task) = spawn(state).await;
    assert_eq!(
        send(&base, None, REVIEW_BODY).await,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            error("authentication_not_configured")
        )
    );
    task.abort();
}

#[tokio::test]
async fn strict_review_dto_rejects_unknown_duplicate_recursive_trailing_and_wrong_types() {
    let (_guard, state) = fixture().await;
    let (base, task) = spawn(state).await;
    for body in [
        r#"{"kind":"typed_candidate","id":"candidate-review","arbitrary":"secret"}"#,
        r#"{"kind":"typed_candidate","id":"candidate-review","detail":"exact","extra":{"key":1,"key":2}}"#,
        r#"{"kind":"typed_candidate","id":"candidate-review","id":"other"}"#,
        r#"{"kind":"run","id":"candidate-review"}"#,
        r#"{"kind":"typed_candidate","id":1}"#,
        r#"{"kind":"typed_candidate","id":"candidate-review","detail":"raw"}"#,
        r#"{"kind":"typed_candidate","id":"candidate-review"} {}"#,
        r#"{"kind":"typed_candidate","id":"candidate-review","detail":null}"#,
        r#"{"kind":"typed_candidate","id":"candidate-review","untrusted_tokens":42}"#,
        "[]",
        "null",
    ] {
        assert_eq!(
            send(&base, Some(ADMIN_TOKEN), body).await,
            (StatusCode::BAD_REQUEST, error("invalid_review_payload"))
        );
    }
    for (kind, id) in [
        ("typed_candidate", "missing-opaque-id"),
        ("stage_fact", CANDIDATE_ID),
        ("formal_report", CANDIDATE_ID),
        ("typed_candidate", "review-run-first"),
    ] {
        let body = json!({"kind":kind,"id":id}).to_string();
        assert_eq!(
            send(&base, Some(ADMIN_TOKEN), &body).await,
            (StatusCode::NOT_FOUND, error("review_unavailable"))
        );
    }
    task.abort();
}

#[tokio::test]
async fn raw_http_default_metadata_and_exact_full_candidate_are_explicitly_unbound() {
    let (_guard, state) = fixture().await;
    let (base, task) = spawn(state).await;
    let (status, metadata) = send(
        &base,
        Some(ADMIN_TOKEN),
        r#"{"kind":"typed_candidate","id":"candidate-review"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(metadata["detail"], "metadata");
    let metadata_content = &metadata["payload"]["materials"]["bundle"]["skill"]["content"];
    assert!(metadata_content["display"].is_null() && metadata_content["display_sha256"].is_null());
    assert_eq!(
        metadata_content["original_bytes"],
        "Complete stored review rule. 中文🙂".len()
    );
    let body = json!({"kind":"typed_candidate","id":CANDIDATE_ID,"detail":"exact"}).to_string();
    let (status, exact) = send(&base, Some(ADMIN_TOKEN), &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(exact["schema_version"], "rsia.review.v1");
    assert_eq!(exact["payload"]["type"], "typed_candidate");
    assert_eq!(
        exact["payload"]["materials"]["bundle"]["skill"]["content"]["display"],
        "Complete stored review rule. 中文🙂"
    );
    assert_eq!(exact["diff_complete"], false);
    assert!(exact["trusted_tokens"].is_null() && exact["total_cost"].is_null());
    for gap in [
        "typed_candidate_history_unbound",
        "typedcandidateformal_unbound",
        "parent_material_unbound",
        "baseline_material_unbound",
        "cost_unobserved",
        "host_application_unbound",
        "approval_unbound",
        "rollback_target_unbound",
    ] {
        assert!(exact["gaps"].as_array().unwrap().contains(&json!(gap)));
    }
    for hidden in [
        "raw_body",
        "output_utf8",
        "approval_id",
        "request_artifact",
        "snapshot_fingerprint",
        "body",
    ] {
        assert!(!exact.to_string().contains(&format!("\"{hidden}\"")));
    }
    task.abort();
}

#[tokio::test]
async fn http_budget_ref_identity_and_missing_revoked_source_edge_refuse_the_complete_payload() {
    for case in [
        "noncanonical",
        "missing_revoked_edge",
        "present_revoked_edge",
    ] {
        let (guard, state) = fixture().await;
        let store = state.service.store().clone();
        let admin = context("admin", Role::Admin);
        store
            .authorize_root_budget(
                &admin,
                &RootBudgetAuthorization {
                    root_budget_id: "http-review-root-budget".into(),
                    billing_scope: "http-review-billing".into(),
                    allowed_namespaces: vec![NAMESPACE.into()],
                    currency: "USD".into(),
                    pricing_version: "pricing-v1".into(),
                    payment_subject: "fixture-only".into(),
                    authorization_receipt_digest: d("http-budget-authorization"),
                    per_call_cap_micros: 100,
                    total_limit_micros: 1_000,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
        store
            .reserve_budget_call_with_sources(
                &admin,
                &BudgetCallReservation {
                    billing_scope: "http-review-billing".into(),
                    call_id: "http-review-call".into(),
                    dispatch_group_id: "http-review-group".into(),
                    stage: BudgetStage::TaskExecution,
                    actual_input_digest: d("http-budget-input"),
                    request_artifact: None,
                    max_cost_micros: 100,
                    lease_token: "fixture-lease".into(),
                    lease_until: 1_000,
                    now: 2,
                },
                &["review-run-first".into()],
            )
            .await
            .unwrap();
        let canonical_digest = fingerprint(&(
            "rsia.budget_call_ref.v1",
            NAMESPACE,
            "http-review-billing",
            "http-review-call",
        ))
        .unwrap();
        let canonical_id = format!("budget-ref-{}", &canonical_digest[..32]);
        let mut session = store.session().await.unwrap();
        let mut reference: BudgetCallRef = session
            .need(&admin, "artifact", &canonical_id)
            .await
            .unwrap();
        session
            .put_edge(&admin, "artifact", CANDIDATE_ID, "artifact", &canonical_id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        let (base, task) = spawn(state).await;
        let before = catalog(&guard.path().join("review-fixture.sqlite3"));
        let (status, control) = send(&base, Some(ADMIN_TOKEN), REVIEW_BODY).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!control["payload"].is_null());
        assert_eq!(
            catalog(&guard.path().join("review-fixture.sqlite3")),
            before
        );
        let mut session = store.session().await.unwrap();
        if case == "noncanonical" {
            reference.id = "budget-ref-uncanonical-fixture".into();
            session
                .put(&admin, "artifact", &reference.id, admin.actor(), &reference)
                .await
                .unwrap();
            session
                .put_edge(&admin, "artifact", CANDIDATE_ID, "artifact", &reference.id)
                .await
                .unwrap();
            session
                .put_edge(&admin, "artifact", &reference.id, "run", "review-run-first")
                .await
                .unwrap();
        } else {
            session.commit().await.unwrap();
            let hidden = source(
                "review-revoked-extra-source",
                "revoked source outside candidate sources",
            );
            store_trace_authority(&store, &context("gateway-host", Role::Host), &hidden)
                .await
                .unwrap();
            session = store.session().await.unwrap();
            reference.source_ids.push(hidden.record.id.clone());
            reference.source_ids.sort();
            session
                .put(&admin, "artifact", &reference.id, admin.actor(), &reference)
                .await
                .unwrap();
            session
                .put(
                    &admin,
                    "tombstone",
                    &hidden.record.id,
                    admin.actor(),
                    &RevokeTombstone {
                        id: hidden.record.id.clone(),
                        schema_version: REVOKE_TOMBSTONE_SCHEMA.into(),
                        source_kind: "run".into(),
                        reason: "fixture-only-revocation".into(),
                        watermark_seq: 1,
                        watermark_digest: d("review-watermark"),
                        created_at: 3,
                    },
                )
                .await
                .unwrap();
            if case == "present_revoked_edge" {
                session
                    .put_edge(&admin, "artifact", &reference.id, "run", &hidden.record.id)
                    .await
                    .unwrap();
            }
        }
        session.commit().await.unwrap();
        let before = catalog(&guard.path().join("review-fixture.sqlite3"));
        let (status, response) = send(&base, Some(ADMIN_TOKEN), REVIEW_BODY).await;
        assert_eq!(status, StatusCode::OK);
        assert!(response["payload"].is_null());
        assert_eq!(response["diff_complete"], false);
        let gap = if case == "present_revoked_edge" {
            "source_changed"
        } else {
            "reference_unavailable"
        };
        assert!(response["gaps"].as_array().unwrap().contains(&json!(gap)));
        assert!(!response.to_string().contains("review-revoked-extra-source"));
        assert_eq!(
            catalog(&guard.path().join("review-fixture.sqlite3")),
            before
        );
        task.abort();
    }
}

fn catalog(path: &std::path::Path) -> Value {
    let script = r#"
import base64,json,sqlite3,sys,urllib.parse
connection=sqlite3.connect('file:'+urllib.parse.quote(sys.argv[1])+'?mode=ro',uri=True)
def typed(value):
    if value is None: return ['null',None]
    if isinstance(value,int): return ['integer',str(value)]
    if isinstance(value,float): return ['real',value.hex()]
    if isinstance(value,str): return ['text',base64.b64encode(value.encode('utf-8')).decode('ascii')]
    return ['blob',base64.b64encode(value).decode('ascii')]
out=[]
for row in connection.execute('SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema ORDER BY type,name'):
    entry={'schema':[typed(value) for value in row]}
    if row[0]=='table':
        name='"'+row[1].replace('"','""')+'"'
        entry['columns']=[[typed(value) for value in info] for info in connection.execute('PRAGMA table_xinfo('+name+')')]
        rows=[[typed(value) for value in item] for item in connection.execute('SELECT * FROM '+name)]
        entry['rows']=sorted(rows,key=lambda item:json.dumps(item,sort_keys=True,separators=(',',':')))
    out.append(entry)
connection.close()
print(json.dumps(out,sort_keys=True,separators=(',',':')))
"#;
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "read-only catalog reader failed");
    serde_json::from_slice(&output.stdout).unwrap()
}
