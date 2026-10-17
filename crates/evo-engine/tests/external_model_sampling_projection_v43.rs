//! Local software counterexamples: only the Host test child is OS-observed.
//! The other three principals and authorization records are explicit fixtures.
//! No external provider, paid dispatch, four-UID deployment, or benefit credit.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use evo_core::contract::{HostCapabilities, SkillSnapshot};
use evo_core::evidence::Purpose;
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
};
use evo_core::skill_edit::EvidenceRef;
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::broker::ModelTransport;
use evo_engine::formal_authority::{
    PRINCIPAL_ACTION_SCHEMA, PRINCIPALS_SCHEMA, RegisteredExternalTransport, RegisteredPrincipal,
    RegisteredPrincipalSet, content_digest,
};
use evo_storage::{Session, Store};
use serde_json::{Value, json};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;
use std::time::{Duration, Instant};

async fn witness(s: &mut Session, ctx: &Context, id: &str, body: &Value) {
    let base = match ctx.role() {
        Role::Admin => "formal.admin.content.v1",
        Role::Evaluator => "formal.evaluator.content.v1",
        _ => unreachable!(),
    };
    s.audit(
        ctx,
        &format!(
            "{base}.{}",
            fingerprint(&(ctx.namespace(), "artifact", id)).unwrap()
        ),
        &fingerprint(&(
            ctx.namespace(),
            "artifact",
            id,
            content_digest(body).unwrap(),
        ))
        .unwrap(),
    )
    .await
    .unwrap();
}

async fn synthetic_action(
    s: &mut Session,
    set: &RegisteredPrincipalSet,
    host: &evo_engine::formal_authority::RegisteredFormalHost,
    id: &str,
    body: &Value,
) {
    let action_id = format!(
        "principal-action-{}",
        fingerprint(&("artifact", id, Role::Admin)).unwrap()
    );
    let p = &set.administrator;
    let value = json!({"id":action_id,"schema_version":PRINCIPAL_ACTION_SCHEMA,"namespace":"n",
        "host_registration_id":host.id,"host_digest":fingerprint(host).unwrap(),"principal_set_digest":set.binding_digest().unwrap(),
        "role":"admin","actor":p.actor,"uid":p.uid,"principal":p.principal,"credential_digest":p.credential_digest,
        "subject_kind":"artifact","subject_id":id,"subject_content_digest":content_digest(body).unwrap(),
        "operation":"bootstrap","authenticated_request_digest":hash(b"explicit-synthetic-admin-command"),
        "peer_uid":p.uid,"peer_gid":Value::Null,"peer_pid":Value::Null,"authority_pid":1,"authority_uid":set.grader.uid,
        "authority_executable_digest":host.grader_code_digest});
    let grader = Context::new("n", "grader", Role::Evaluator).unwrap();
    s.put(&grader, "artifact", &action_id, grader.actor(), &value)
        .await
        .unwrap();
    witness(s, &grader, &action_id, &value).await;
    s.put_edge(&grader, "artifact", id, "artifact", &action_id)
        .await
        .unwrap();
    s.put_edge(&grader, "artifact", &action_id, "artifact", &set.id)
        .await
        .unwrap();
}

async fn probe(case: &str) {
    let workspace = std::env::current_dir().unwrap();
    let uid = std::fs::metadata(&workspace).unwrap().uid();
    let executable_digest = hash(&std::fs::read(std::env::current_exe().unwrap()).unwrap());
    let credential = "local-only-fixture-host-credential-32-bytes";
    assert_eq!(
        std::env::var("RSIA_AUDIT_F06_LOCAL_HOST_KEY").unwrap(),
        credential
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let store = Store::open(&workspace.join("db")).await.unwrap();
    let mut f = trusted_compiler_inputs::inputs(
        "n",
        "p",
        SkillSnapshot::empty(),
        Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        &hash(b"environment"),
    );
    let principal = |uid, actor: &str, secret: &str| RegisteredPrincipal {
        uid,
        actor: actor.into(),
        principal: hash(secret.as_bytes()),
        credential_digest: hash(secret.as_bytes()),
    };
    let set = RegisteredPrincipalSet {
        id: format!("formal-principals-{}", f.host.id),
        schema_version: PRINCIPALS_SCHEMA.into(),
        namespace: "n".into(),
        host_registration_id: f.host.id.clone(),
        version: 1,
        host: principal(uid, "host", credential),
        grader: principal(uid + 1, "grader", "fixture-grader-credential"),
        administrator: principal(uid + 2, "fixture-admin", "fixture-admin-credential"),
        worker: principal(uid + 3, "worker", "fixture-worker-credential"),
    };
    f.host.host_uid = uid;
    f.host.grader_uid = uid + 1;
    f.host.administrator_uid = uid + 2;
    f.host.code_digest = executable_digest.clone();
    f.host.host_workspace = workspace.to_str().unwrap().into();
    f.host.host_principal = set.host.principal.clone();
    f.host.grader_principal = set.grader.principal.clone();
    f.host.administrator_principal = set.administrator.principal.clone();
    f.host.worker_principal = set.worker.principal.clone();
    f.host.credential_binding_digest = set.binding_digest().unwrap();
    f.provider.endpoint = format!("https://{address}/v1/chat/completions");
    f.provider.credential_environment_key = "RSIA_AUDIT_F06_LOCAL_HOST_KEY".into();
    f.provider.credential_principal = set.host.credential_digest.clone();
    f.provider.transport_code_digest = executable_digest;
    trusted_compiler_inputs::install(&store, &f).await.unwrap();
    let admin = Context::new("n", "fixture-admin", Role::Admin).unwrap();
    let grader = Context::new("n", "grader", Role::Evaluator).unwrap();
    let observation = json!({"id":set.id,"schema_version":"rsia.formal_principal_set_observation.v1","namespace":"n",
        "host_digest":fingerprint(&f.host).unwrap(),"principals":set,"grader_pid":1,"grader_uid":f.host.grader_uid,
        "grader_executable_digest":f.host.grader_code_digest,"grader_workspace":f.host.grader_workspace});
    let mut s = store.session().await.unwrap();
    s.put(&admin, "artifact", &set.id, admin.actor(), &observation)
        .await
        .unwrap();
    witness(&mut s, &admin, &set.id, &observation).await;
    witness(&mut s, &grader, &set.id, &observation).await;
    synthetic_action(
        &mut s,
        &set,
        &f.host,
        &f.host.id,
        &serde_json::to_value(&f.host).unwrap(),
    )
    .await;
    synthetic_action(
        &mut s,
        &set,
        &f.host,
        &f.provider.id,
        &serde_json::to_value(&f.provider).unwrap(),
    )
    .await;
    s.commit().await.unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    // This must succeed first: an earlier setup/credential failure is not a
    // sampling counterexample. connect observes the actual child PID/UID/code.
    let transport = RegisteredExternalTransport::connect(&host, &store, &f.provider.id)
        .await
        .unwrap();
    let network = std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    drop(stream);
                    return 1;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return 0,
                Err(e) => panic!("local listener failed: {e}"),
            }
        }
    });
    let sampling = match case {
        "arbitrary" => hash(b"arbitrary-valid-sha256"),
        "current_digest" => hash(br#"{"algorithm_version":"rsia.chat_sampling.sha256.cjson.v1","protocol_id":"chat_completions_nonstream_v1","sampling":{"temperature":0},"schema_version":"rsia.chat_sampling_projection.v1"}"#),
        _ => hash(br#"{"temperature":1}"#),
    };
    let request = ModelRequest::build(
        ModelRequestContext {
            request_id: "sampling-probe".into(),
            namespace: "n".into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "episode".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: hash(b"P"),
            bundle_digest: hash(b"bundle"),
            source_closure: vec![EvidenceRef {
                id: "source".into(),
                digest: hash(b"source"),
            }],
            model_digest: f.provider.model_digest.clone(),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: sampling.clone(),
            revoke_watermark: 0,
            max_suggestions: 1,
        },
        vec![ModelInputPart {
            role: ModelInputRole::User,
            label: "input".into(),
            content: "local sampling probe".into(),
        }],
    )
    .unwrap();
    let result = transport.execute(&request, "local-only-dispatch").await;
    let connections = network.join().unwrap();
    println!(
        "AUDIT_F06 {}",
        json!({"case":case,"healthy_connect":true,"pid":std::process::id(),"uid":uid,
        "sampling_digest":sampling,"observed_local_connections":connections,"actual_result":format!("{result:?}"),"real_provider":false})
    );
    assert!(matches!(result,Err(Error::Conflict(s)) if s=="LegacyUnboundSampling"));
    assert_eq!(connections, 0);
    store.close().await;
}

fn run(case: &str, name: &str) {
    if std::env::var("RSIA_AUDIT_F06_CASE").as_deref() == Ok(case) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(Box::pin(probe(case)));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .current_dir(dir.path())
        .env_clear()
        .env("RSIA_AUDIT_F06_CASE", case)
        .env(
            "RSIA_AUDIT_F06_LOCAL_HOST_KEY",
            "local-only-fixture-host-credential-32-bytes",
        )
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&result.stdout);
    for line in stdout.lines().filter(|l| l.contains("AUDIT_F06")) {
        println!("{line}");
    }
    assert!(
        result.status.success(),
        "child semantic check failed: {stdout}\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn audit_f06_arbitrary_sampling_cannot_reach_local_transport() {
    run(
        "arbitrary",
        "audit_f06_arbitrary_sampling_cannot_reach_local_transport",
    );
}
#[test]
fn audit_f06_temperature_one_cannot_reach_local_transport() {
    run(
        "temperature_one",
        "audit_f06_temperature_one_cannot_reach_local_transport",
    );
}

#[test]
fn audit_f06_matching_new_digest_does_not_promote_legacy_registration() {
    run(
        "current_digest",
        "audit_f06_matching_new_digest_does_not_promote_legacy_registration",
    );
}
