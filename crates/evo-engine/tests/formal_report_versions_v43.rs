//! AG163 software-history tests: original SQLite bytes, not paid/provider/OS proof.
//! Prepared source; compilation and runtime evidence have not been produced.
#![cfg(any(target_os = "macos", target_os = "linux"))]

#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use evo_core::evaluation::{
    ControlCondition, ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec,
    FormalExperimentPlanV41, FormalStatisticalUnit, Money, OptimizationBudgetPlan,
    OptimizerComparisonContract, ProfileKind,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::holdout::{
    AnchorCoverageMatrix, CriticalCapabilityCoverage, FrozenCandidatePair, HoldoutManifest,
    SequentialMode,
};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::sequential::{
    AlphaAllocation, EarlyStopPlan, FormalClaimKind, ResearchFamilyAlphaPlan,
};
use evo_core::{Context, Error, Role, canonical_fingerprint, fingerprint, hash};
use evo_engine::dispatch::{ManagementDispatcher, ManagementJobState};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::release_store::{CurrentStageBundleRequest, ReleaseStore, TypedSourceRef};
use evo_engine::streaming_evaluator::{
    EvaluationEvidenceScope, FixedGraderMethod, FixedGraderSpec, FrozenOracleEntry,
    IssueTicketRequest, ProtectedHoldoutRecord, ProtectedInputRef, RegisteredEvaluationControl,
};
use evo_engine::streaming_evaluator::{
    IndependentEvaluationControl, read_verified_formal_report_in_session,
};
use evo_storage::budget::RootBudgetAuthorization;
use evo_storage::lifecycle::{
    FORMAL_JUDGMENT_VERSIONS, FormalJudgmentDomain, FormalJudgmentIdStrategy, LifecycleStore,
    TypedObjectRef, formal_judgment_version,
};
use evo_storage::{PersistentContractState, Store};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const HISTORY: &[u8] = include_bytes!("fixtures/ag163_formal_v2_history.json");
const HISTORY_SHA: &str = "2bc4ecd85ad308cd888067c797391473f43814a0a39988b6035de93dc84a2e79";
const FORMAL_KIND: &str = "formal_evaluation_v2";
const COMPLETE: &str = "ag163-old-complete-v2";
const EARLY: &str = "ag163-old-early-v2";

fn sid(kind: &str, id: &str) -> String {
    format!("e05-{}", fingerprint(&(kind, id)).unwrap())
}

fn hex_bytes(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

struct History {
    store: Store,
    path: PathBuf,
    original: Value,
    evaluator: Context,
    admin: Context,
    ticket_id: String,
    root: tempfile::TempDir,
}

impl History {
    async fn load(name: &str) -> Self {
        assert_eq!(hash(HISTORY), HISTORY_SHA);
        let all: Value = serde_json::from_slice(HISTORY).unwrap();
        assert_eq!(
            all["producer_commit"],
            "800eafc4ccaf39d5e4f3fb1b7891f9c142e6b482"
        );
        assert_eq!(all["software_fixture_only"], true);
        assert_eq!(all["real_provider_or_paid_or_OS_authority_proof"], false);
        let original = all["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap()
            .clone();
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        fs::DirBuilder::new().mode(0o700).create(&private).unwrap();
        let path = private.join("history.sqlite3");
        let bytes = hex_bytes(original["database_hex"].as_str().unwrap());
        assert_eq!(
            bytes.len() as u64,
            original["database_bytes"].as_u64().unwrap()
        );
        assert_eq!(hash(&bytes), original["database_sha256"].as_str().unwrap());
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let namespace = original["namespace"].as_str().unwrap();
        let evaluator = Context::new(
            namespace,
            original["evaluator_actor"].as_str().unwrap(),
            Role::Evaluator,
        )
        .unwrap();
        let admin = Context::new(namespace, "admin", Role::Admin).unwrap();
        let ticket_id = original["ticket_id"].as_str().unwrap().to_owned();
        let store = Store::open(&path).await.unwrap();
        assert_eq!(store.integrity().await.unwrap(), "ok");
        let result = Self {
            store,
            path,
            original,
            evaluator,
            admin,
            ticket_id,
            root,
        };
        result.assert_legacy().await;
        result
    }

    async fn invalid() -> Self {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        fs::DirBuilder::new().mode(0o700).create(&private).unwrap();
        let path = private.join("history.sqlite3");
        drop(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap(),
        );
        let initial = Store::open(&path).await.unwrap();
        initial.close().await;
        let frozen =
            trusted_compiler_inputs::restore_frozen_issuer_rows(&path, "late-final-inflight")
                .await
                .unwrap();
        assert_eq!(
            frozen["original_snapshot_sha256"],
            "1e6805a1a02b89b4e75be8d04691924321098614429fc0037bc85afa872d0326"
        );
        let evaluator =
            Context::new("late-grader-final-fixture", "evaluator", Role::Evaluator).unwrap();
        let admin = Context::new("late-grader-final-fixture", "admin", Role::Admin).unwrap();
        let store = Store::open(&path).await.unwrap();
        let original = json!({"expected_variant":"invalid","original_call_rows":3});
        let result = Self {
            store,
            path,
            original,
            evaluator,
            admin,
            ticket_id: "late-grader-final-ticket".into(),
            root,
        };
        result.assert_legacy().await;
        result
    }

    async fn assert_legacy(&self) {
        let before = image(&self.path);
        let status =
            IndependentEvaluationControl::status(&self.evaluator, &self.store, &self.ticket_id)
                .await
                .unwrap();
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(
            value["formal"]["variant"],
            self.original["expected_variant"]
        );
        assert_eq!(value["formal"]["evidence_scope"], "program_fixture");
        let mut session = self.store.session().await.unwrap();
        let read =
            read_verified_formal_report_in_session(&self.evaluator, &mut session, &self.ticket_id)
                .await
                .unwrap();
        assert_eq!(read.state(), PersistentContractState::Legacy);
        assert!(!read.historical_view().promotion_eligible);
        assert!(matches!(read.current_view(), Err(Error::Conflict(reason))
            if reason == "source_legacy: formal report is not Current input"));
        session.commit().await.unwrap();
        assert!(
            image(&self.path) == before,
            "healthy Legacy reads changed original full database"
        );
    }

    async fn body(&self, logical_id: &str) -> String {
        let mut session = self.store.session().await.unwrap();
        let row = session
            .object_row(&self.evaluator, "artifact", &sid(FORMAL_KIND, logical_id))
            .await
            .unwrap()
            .unwrap();
        session.commit().await.unwrap();
        row.body
    }

    async fn recompute(&self, id: &str) -> String {
        IndependentEvaluationControl::recompute_judgment(&self.evaluator, &self.store, id)
            .await
            .unwrap()
    }

    async fn replace_body(&mut self, logical_id: &str, body: &Value) {
        self.store.close().await;
        let program = "import sqlite3,sys,json\nb=json.load(sys.stdin)\nc=sqlite3.connect(sys.argv[1]);c.execute('UPDATE objects SET body=? WHERE namespace=? AND kind=\"artifact\" AND id=?',(json.dumps(b,separators=(\",\",\":\")),sys.argv[2],sys.argv[3]));assert c.total_changes==1;c.commit();c.close()";
        let mut child = Command::new("python3")
            .args(["-I", "-B", "-c", program])
            .arg(&self.path)
            .arg(self.evaluator.namespace())
            .arg(sid(FORMAL_KIND, logical_id))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(body).unwrap())
            .unwrap();
        assert!(
            child.wait_with_output().unwrap().status.success(),
            "owned negative-body mutation failed"
        );
        self.store = Store::open(&self.path).await.unwrap();
    }
}

/// Full typed logical image. Equality failures never dump the full database.
fn image(path: &Path) -> Value {
    let program = r#"
import json,sqlite3,struct,sys,urllib.parse
def qi(s):return '"'+s.replace('"','""')+'"'
def cell(k,v):
 if k==b'null':return ['null']
 if k==b'integer':return ['integer',str(v)]
 if k==b'real':return ['real',struct.pack('>d',v).hex()]
 if k in (b'text',b'blob'):return [k.decode(),v.hex()]
 raise AssertionError(k)
c=sqlite3.connect('file:'+urllib.parse.quote(sys.argv[1],safe='/')+'?mode=ro',uri=True,isolation_level=None);c.text_factory=bytes
c.execute('PRAGMA query_only=ON');c.execute('BEGIN');assert c.execute('PRAGMA integrity_check').fetchall()==[(b'ok',)]
schema=c.execute('SELECT type,name,tbl_name,rootpage,sql FROM sqlite_schema ORDER BY type COLLATE BINARY,name COLLATE BINARY').fetchall();tables=[]
for typ,n,_,_,ddl in schema:
 if typ!=b'table':continue
 name=n.decode();info=c.execute('PRAGMA table_xinfo('+qi(name)+')').fetchall();cols=[x[1].decode() for x in info if x[6]!=1]
 rowid=b'WITHOUT ROWID' not in (ddl or b'').upper();fields=['_rowid_'] if rowid else []
 for col in cols:fields.extend(['typeof('+qi(col)+')',qi(col)])
 rows=[]
 for v in c.execute('SELECT '+','.join(fields)+' FROM '+qi(name)):
  off=1 if rowid else 0;rows.append([str(v[0]) if rowid else None,[cell(v[off+2*i],v[off+2*i+1]) for i in range(len(cols))]])
 rows.sort(key=lambda v:json.dumps(v,separators=(',',':')));tables.append({'name':name,'columns':cols,'rowid':rowid,'rows':rows})
calls=c.execute('SELECT state,actual_cost_micros,execution_closed FROM root_budget_calls ORDER BY billing_scope,call_id').fetchall()
c.execute('ROLLBACK');c.close();print(json.dumps({'schema':[[a.hex(),b.hex(),d.hex(),p,None if sql is None else sql.hex()] for a,b,d,p,sql in schema],'tables':tables,'calls':[[s.decode(),a,x] for s,a,x in calls]},sort_keys=True,separators=(',',':')))
"#;
    let output = Command::new("python3")
        .args(["-I", "-B", "-c", program])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "owned readonly full database image failed"
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn table<'a>(image: &'a Value, name: &str) -> &'a Value {
    image["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|table| table["name"] == name)
        .unwrap()
}

fn preserves_old_facts(before: &Value, after: &Value) {
    for old in before["tables"].as_array().unwrap() {
        let name = old["name"].as_str().unwrap();
        let new = table(after, name);
        if name == "sqlite_sequence" {
            assert!(
                old["columns"] == new["columns"],
                "sqlite_sequence schema changed"
            );
            for row in old["rows"].as_array().unwrap() {
                let successor = new["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| v[1][0] == row[1][0])
                    .unwrap();
                assert!(
                    successor[0] == row[0] && successor[1][0] == row[1][0],
                    "original sqlite_sequence identity changed"
                );
                assert_eq!(row[1][1][0], "integer");
                assert_eq!(successor[1][1][0], "integer");
                let before_seq: i64 = row[1][1][1].as_str().unwrap().parse().unwrap();
                let after_seq: i64 = successor[1][1][1].as_str().unwrap().parse().unwrap();
                assert!(
                    after_seq >= before_seq,
                    "actual append reset an original sqlite_sequence counter"
                );
            }
            continue;
        }
        assert!(old["columns"] == new["columns"], "columns changed");
        let old_rows = old["rows"].as_array().unwrap();
        let new_rows = new["rows"].as_array().unwrap();
        assert!(
            old_rows.iter().all(|row| new_rows.contains(row)),
            "old rowid/typed bytes changed"
        );
        if name.starts_with("root_budget") || name.starts_with("revoke_") {
            assert!(
                old_rows == new_rows,
                "old paid/uncertain/closed/revocation facts changed"
            );
        }
    }
    assert!(
        before["calls"] == after["calls"],
        "recompute created or changed actual budget calls"
    );
}

/// VACUUM INTO may relocate ordinary rowids. Restore equality still compares
/// every stored column's type/raw bytes and sqlite_sequence, excluding only the
/// synthetic rowid and sqlite_schema root-page addresses used by same-file reads.
fn restore_logical_tables(image: &Value) -> BTreeMap<String, Value> {
    image["tables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|table| {
            let mut rows: Vec<Value> = table["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row[1].clone())
                .collect();
            rows.sort_by_key(|row| serde_json::to_string(row).unwrap());
            (
                table["name"].as_str().unwrap().to_owned(),
                json!({"columns":table["columns"],"rows":rows}),
            )
        })
        .collect()
}

async fn status_metadata(fx: &History, key: &str) -> Value {
    let dispatcher =
        ManagementDispatcher::new(fx.store.clone(), vec![fx.evaluator.clone()]).unwrap();
    let job = dispatcher.submit(&fx.evaluator, "evaluation.status", json!({
        "schema_version":"rsia.management.evaluation_status.v1","request_key":key,"ticket_id":fx.ticket_id
    })).await.unwrap();
    for _ in 0..500 {
        let current = dispatcher.status(&fx.evaluator, &job.id).await.unwrap();
        if current.state == ManagementJobState::Succeeded {
            let value = serde_json::to_value(current).unwrap();
            assert_eq!(value["result"]["result"], "evaluation_status_read_v2");
            let meta = value["result"]["report_read"].clone();
            assert_eq!(
                meta["schema_version"],
                "rsia.verified_formal_report_read.v1"
            );
            return meta;
        }
        assert!(
            matches!(
                current.state,
                ManagementJobState::Queued | ManagementJobState::Running
            ),
            "real status producer failed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("real status producer did not finish");
}

fn old_complete_domain(control: &str) -> FormalJudgmentDomain {
    serde_json::from_value(json!([
        "formal_evaluation_v2",
        "rsia.typed_artifact_envelope.v1",
        null,
        "complete_batch",
        "program_fixture",
        control,
        "rsia.evaluation_ticket.v2",
        "rsia.experiment_plan.v1",
        "rsia.formal_experiment_plan.v4_1",
        "rsia.research_family_alpha_plan.v1",
        "rsia.optimizer_comparison_contract.v1",
        "rsia.optimization_budget_plan.v1",
        "rsia.protected_holdout_record.v1",
        "rsia.empirical_bernstein.v2",
        null,
        null,
        null,
        if control.ends_with(".v1") {
            "complete_control_v1_frozen"
        } else {
            "complete_control_v2_preserve_ucb0"
        }
    ]))
    .unwrap()
}

// The following private producer is extracted from the existing candidate_review
// software diagnostic: the same public source recording/compiler/staging/issuance
// calls run here. No new permission, PrincipalAction or issuer factory is added.
const CURRENT_NS: &str = "ag163-current-diagnostic";
fn current_digest(label: &str) -> String {
    hash(label.as_bytes())
}
struct CurrentDiagnostic {
    _dir: tempfile::TempDir,
    db_path: PathBuf,
    store: Store,
    evaluator: Context,
    ticket_id: String,
}
fn current_source(id: &str, body: &str) -> StoredTraceAuthority {
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
fn current_money(amount: &str) -> Money {
    Money {
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
        amount: amount.into(),
    }
}

fn current_optimizer(arm: FixedOptimizerArm, implementation: &str) -> FixedOptimizerSpec {
    FixedOptimizerSpec {
        arm,
        implementation_digest: current_digest(implementation),
        initial_s0_digest: current_digest("s0"),
        base_model_digest: current_digest("model"),
        tools_digest: current_digest("tools"),
        authorized_materials_digest: current_digest("materials"),
        task_partition_digest: current_digest("partition"),
        context_limit_tokens: 4096,
        root_budget_scope_id: "billing-1".into(),
        budget_limit: current_money("0.001"),
    }
}

async fn current_diagnostic_fixture(n: u32, sequential: bool) -> CurrentDiagnostic {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("e05.sqlite3");
    drop(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&db_path)
            .unwrap(),
    );
    let store = Store::open(&db_path).await.unwrap();
    let admin = Context::new(CURRENT_NS, "admin", Role::Admin).unwrap();
    let evaluator = Context::new(CURRENT_NS, "evaluator", Role::Evaluator).unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "billing-1".into(),
                allowed_namespaces: vec![CURRENT_NS.into(), "other".into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "fixture-only".into(),
                authorization_receipt_digest: current_digest("admin-budget-receipt"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();

    let mut v1 = ExperimentPlan::first_low_risk("formal-e05", 60).unwrap();
    v1.n_planned = n as usize;
    v1.monetary_budget = current_money("0.001");
    v1.frozen = true;
    v1.frozen_at = Some(1);
    v1.validate().unwrap();

    let alpha = ResearchFamilyAlphaPlan::new(
        "family-e05",
        "0.05",
        vec![
            AlphaAllocation {
                claim_id: "fixed-gain".into(),
                attempt_id: "attempt-1".into(),
                kind: FormalClaimKind::FixedSampleGain,
                alpha: "0.04".into(),
            },
            AlphaAllocation {
                claim_id: "early-reject".into(),
                attempt_id: "attempt-1".into(),
                kind: FormalClaimKind::SequentialReject,
                alpha: "0.01".into(),
            },
        ],
    )
    .unwrap();
    let anchors = AnchorCoverageMatrix::new(
        "quality-gain",
        "anchors-v1",
        vec![CriticalCapabilityCoverage {
            capability_id: "safety".into(),
            anchor_ids: vec!["anchor-1".into()],
        }],
    )
    .unwrap();
    let mut budget = OptimizationBudgetPlan::unfunded("billing-1", "payer", "USD").unwrap();
    budget.root_total = current_money("0.001");
    budget.admin_authorization_receipt_digest = Some(current_digest("admin-budget-receipt"));
    for stage in &mut budget.stages {
        stage.per_call_limit.pricing_version = "pricing-v1".into();
        stage.experiment_total.pricing_version = "pricing-v1".into();
    }
    budget.validate().unwrap();
    let comparison = OptimizerComparisonContract::new(
        current_optimizer(FixedOptimizerArm::CSimple, "simple"),
        current_optimizer(FixedOptimizerArm::CSkillopt, "skillopt"),
        FixedOptimizerArm::CSkillopt,
    )
    .unwrap();
    assert!(v1.conditions.contains(&ControlCondition::FixedImproverC));
    let formal = FormalExperimentPlanV41::new(
        &v1,
        current_digest("preregistration"),
        &alpha,
        &comparison,
        &budget,
        anchors.digest().unwrap(),
        current_digest("sampling"),
        current_digest("common-mean"),
        FormalStatisticalUnit::IndependentTaskCluster,
        n,
    )
    .unwrap();
    let ordered_clusters: Vec<_> = (0..n).map(|index| format!("cluster-{index}")).collect();
    let early = sequential.then(|| {
        EarlyStopPlan::new(
            v1.digest().unwrap(),
            &alpha,
            "early-reject",
            "attempt-1",
            ProfileKind::QualityGain,
            n,
            ordered_clusters.clone(),
            current_digest("common-mean"),
            current_digest("sampling"),
            anchors.digest().unwrap(),
            20_000,
            10_000,
            10_000,
        )
        .unwrap()
    });
    let grader = FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    };
    let control = RegisteredEvaluationControl {
        schema_version: RegisteredEvaluationControl::SCHEMA.into(),
        id: "control-1".into(),
        v1_plan_snapshot: v1.clone(),
        formal_plan: formal,
        alpha_plan: alpha.clone(),
        optimizer_comparison: comparison,
        optimization_budget: budget,
        early_stop_plan: early.clone(),
        anchor_matrix: anchors.clone(),
        attempt_id: "attempt-1".into(),
        fixed_sample_claim_id: "fixed-gain".into(),
        billing_scope: "billing-1".into(),
        executor_actor: "executor".into(),
        evaluator_actor: "evaluator".into(),
        proposer_actor: "proposer".into(),
        approver_actor: "approver".into(),
        oracle_digest: current_digest("oracle-version"),
        grader_digest: evo_core::fingerprint(&grader).unwrap(),
        fixed_grader: grader,
        evidence_scope: EvaluationEvidenceScope::ProgramFixture,
        created_at_unix_seconds: 2,
    };
    IndependentEvaluationControl::register_control(&evaluator, &store, control.clone())
        .await
        .unwrap();

    // ProgramFixture's real Current candidate remains a software diagnostic:
    // it confers no external-host, provider, paid or promotion authority.
    let host = Context::new(CURRENT_NS, "host", Role::Host).unwrap();
    let authority = current_source("formal-fixture-source", "formal fixture development source");
    store_trace_authority(&store, &host, &authority)
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let watermark = session
        .bump_watermark(&admin, &current_digest("formal-fixture-watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let proposer = Context::new(CURRENT_NS, "proposer", Role::Worker).unwrap();
    let candidate = trusted_compiler_inputs::software_candidate_current(
        &store,
        &proposer,
        &current_digest("environment"),
        "candidate-bundle",
        watermark as u64,
    )
    .await
    .unwrap();
    let pair = FrozenCandidatePair::new(
        v1.digest().unwrap(),
        current_digest("candidate"),
        candidate.parent_digest.clone(),
        3,
    )
    .unwrap();
    let sequential_mode = match &early {
        Some(plan) => SequentialMode::RejectOnly {
            early_stop_plan_digest: plan.digest().unwrap(),
        },
        None => SequentialMode::Disabled,
    };
    let manifest = HoldoutManifest::issue_after_candidates_frozen(
        &pair,
        "family-e05",
        alpha.digest().unwrap(),
        sequential_mode,
        "epoch-1",
        "slice-1",
        ordered_clusters.clone(),
        ordered_clusters
            .iter()
            .map(|id| current_digest(&format!("source-{id}")))
            .collect(),
        "sampler-v1",
        current_digest("oracle-version"),
        anchors.digest().unwrap(),
        evo_core::fingerprint(&FixedGraderSpec {
            schema_version: FixedGraderSpec::SCHEMA.into(),
            version: "exact-json-v1".into(),
            method: FixedGraderMethod::ExactJsonAnswerV1,
        })
        .unwrap(),
        current_digest("environment"),
        current_digest("private-seed"),
        4,
    )
    .unwrap();
    let rotating_inputs: Vec<_> = ordered_clusters
        .iter()
        .enumerate()
        .map(|(index, cluster)| ProtectedInputRef {
            target_id: format!("r{index}"),
            cluster_or_anchor_id: cluster.clone(),
            input_artifact_id: format!("input-r{index}"),
            input_digest: current_digest(&format!("input-r{index}")),
        })
        .collect();
    let anchor_inputs = vec![ProtectedInputRef {
        target_id: "a0".into(),
        cluster_or_anchor_id: "anchor-1".into(),
        input_artifact_id: "input-a0".into(),
        input_digest: current_digest("input-a0"),
    }];
    let oracle_entries: Vec<_> = rotating_inputs
        .iter()
        .chain(anchor_inputs.iter())
        .map(|input| FrozenOracleEntry {
            target_id: input.target_id.clone(),
            expected_answer_json: json!("ok").to_string(),
        })
        .collect();
    let holdout = ProtectedHoldoutRecord {
        schema_version: ProtectedHoldoutRecord::SCHEMA.into(),
        id: "holdout-1".into(),
        registration_id: "control-1".into(),
        pair,
        manifest,
        candidate_bundle_digest: candidate.digest.clone(),
        baseline_bundle_digest: current_digest("baseline-bundle"),
        rotating_inputs,
        anchor_inputs,
        oracle_payload_digest: evo_core::fingerprint(&oracle_entries).unwrap(),
        oracle_entries,
    };
    IndependentEvaluationControl::register_holdout(&evaluator, &store, holdout.clone())
        .await
        .unwrap();
    // True-source order (L288): the candidate is staged, then the ticket is
    // issued and binds it (AG-100 issues no ticket without a candidate record).
    trusted_compiler_inputs::persist_bundle_current(
        &store,
        &proposer,
        &candidate,
        &current_digest("environment"),
        watermark as u64,
    )
    .await
    .unwrap();
    ReleaseStore::stage_current_bundle(
        &proposer,
        &store,
        CurrentStageBundleRequest {
            candidate_id: holdout.pair.candidate_digest.clone(),
            bundle: candidate,
            environment_digest: holdout.manifest.environment_digest.clone(),
            proposer_actor: proposer.actor().into(),
            sources: vec![TypedSourceRef {
                kind: "run".into(),
                id: authority.record.id,
                content_digest: authority.trace.source_digest,
            }],
            revoke_watermark: watermark as u64,
        },
    )
    .await
    .unwrap();
    let ticket = IndependentEvaluationControl::issue_ticket(
        &evaluator,
        &store,
        IssueTicketRequest {
            ticket_id: "ticket-1".into(),
            registration_id: "control-1".into(),
            holdout_id: "holdout-1".into(),
            issued_at_unix_seconds: 5,
        },
    )
    .await
    .unwrap();
    assert!(ticket.candidate_dependency_binding.is_some());
    CurrentDiagnostic {
        _dir: dir,
        db_path,
        store,
        evaluator,
        ticket_id: ticket.id,
    }
}

#[test]
fn closed_judgment_catalog_preserves_exact_existing_domains() {
    let mut keys = BTreeSet::new();
    let mut old = 0;
    let mut explicit = 0;
    for entry in FORMAL_JUDGMENT_VERSIONS {
        assert!(!entry.introduced_change.is_empty());
        assert!(
            keys.insert(entry.domain.version_key().unwrap()),
            "duplicate full semantic domain"
        );
        assert_eq!(
            formal_judgment_version(&entry.domain).unwrap().domain,
            entry.domain
        );
        assert_eq!(
            serde_json::to_value(&entry.domain)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            18
        );
        match entry.id_strategy {
            FormalJudgmentIdStrategy::SourceTicket => {
                old += 1;
                assert!(entry.domain.2.is_none());
                assert!(entry.verifier_retained);
            }
            FormalJudgmentIdStrategy::VersionedInput => {
                explicit += 1;
                assert_eq!(entry.domain.2.as_deref(), Some("rsia.formal_judgment.v1"));
                assert_ne!(entry.domain.3.as_ref(), "invalid");
            }
        }
    }
    assert_eq!((old, explicit), (56, 40));
}

#[test]
fn formal_control_versions_are_distinct_semantic_keys() {
    let one = old_complete_domain("rsia.registered_evaluation_control.v1");
    let two = old_complete_domain("rsia.registered_evaluation_control.v2");
    assert_ne!(one.version_key().unwrap(), two.version_key().unwrap());
    assert!(!one.same_input_family(&two));
    assert_eq!(
        formal_judgment_version(&one).unwrap().id_strategy,
        FormalJudgmentIdStrategy::SourceTicket
    );
    assert_eq!(
        formal_judgment_version(&two).unwrap().id_strategy,
        FormalJudgmentIdStrategy::SourceTicket
    );
    assert_eq!(one.17.as_ref(), "complete_control_v1_frozen");
    assert_eq!(two.17.as_ref(), "complete_control_v2_preserve_ucb0");
}

#[test]
fn certificate_version_requires_the_registered_plan_domain() {
    let entry = FORMAL_JUDGMENT_VERSIONS
        .iter()
        .find(|row| {
            row.domain.3.as_ref() == "early_stopped"
                && row.domain.16.as_deref() == Some("sequential_reject_only")
        })
        .unwrap();
    assert!(entry.domain.13.is_none() && entry.domain.15.is_some());
    let mut missing_plan = entry.domain.clone();
    missing_plan.15 = None;
    assert!(
        matches!(formal_judgment_version(&missing_plan), Err(Error::Invalid(reason))
        if reason == "formal_judgment_version_outside_table")
    );
    let mut future_certificate = entry.domain.clone();
    future_certificate.14 = Some("rsia.early_stop_certificate.v99".into());
    assert!(
        matches!(formal_judgment_version(&future_certificate), Err(Error::Invalid(reason))
        if reason == "formal_judgment_version_outside_table")
    );
    let critical = FORMAL_JUDGMENT_VERSIONS
        .iter()
        .find(|row| {
            row.domain.3.as_ref() == "early_stopped"
                && row.domain.16.as_deref() == Some("critical_k0")
                && row.domain.15.is_none()
        })
        .unwrap();
    assert!(formal_judgment_version(&critical.domain).is_ok());
}

#[test]
fn versioned_identity_and_lineage_keep_old_physical_ids() {
    let domain = old_complete_domain("rsia.registered_evaluation_control.v2");
    let expected = r#"["formal_evaluation_v2","rsia.typed_artifact_envelope.v1",null,"complete_batch","program_fixture","rsia.registered_evaluation_control.v2","rsia.evaluation_ticket.v2","rsia.experiment_plan.v1","rsia.formal_experiment_plan.v4_1","rsia.research_family_alpha_plan.v1","rsia.optimizer_comparison_contract.v1","rsia.optimization_budget_plan.v1","rsia.protected_holdout_record.v1","rsia.empirical_bernstein.v2",null,null,null,"complete_control_v2_preserve_ucb0"]"#;
    assert_eq!(serde_json::to_vec(&domain).unwrap(), expected.as_bytes());
    let value: Value = serde_json::from_str(expected).unwrap();
    let decoded: FormalJudgmentDomain = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(decoded, domain);
    assert_eq!(
        canonical_fingerprint(&domain).unwrap(),
        canonical_fingerprint(&value).unwrap()
    );
    assert_eq!(
        domain.version_key().unwrap(),
        format!("e05dv-{}", canonical_fingerprint(&value).unwrap())
    );
    // Literal copied from the real old producer's frozen objects row, not
    // generated by a Python reimplementation of the ID formula.
    let old_ticket = "late-grader-final-ticket";
    assert_eq!(
        sid(FORMAL_KIND, old_ticket),
        "e05-d7d4baf82010d2637b36e41685d89c69bd0c0021ec862315897ba3fd8ded420a"
    );
    assert_eq!(
        formal_judgment_version(&domain).unwrap().id_strategy,
        FormalJudgmentIdStrategy::SourceTicket
    );
    eprintln!("AG163_RUST_DOMAIN_GOLDEN={}", domain.version_key().unwrap());
}

#[tokio::test]
async fn new_wire_cannot_retag_legacy_payload_or_accept_future_versions() {
    let mut fx = History::load(COMPLETE).await;
    let old_body = fx.body(&fx.ticket_id).await;
    let id = fx.recompute(&fx.ticket_id).await;
    let mut wire: Value = serde_json::from_str(&fx.body(&id).await).unwrap();
    wire["payload"]["schema_version"] = json!("rsia.formal_evaluation.v99");
    fx.replace_body(&id, &wire).await;
    let before = image(&fx.path);
    assert!(
        matches!(IndependentEvaluationControl::recompute_judgment(&fx.evaluator, &fx.store, &id).await,
        Err(Error::Invalid(reason)) if reason == "formal_judgment_version_outside_table")
    );
    assert!(
        image(&fx.path) == before,
        "future wire changed persistent facts"
    );
    assert!(
        fx.body(&fx.ticket_id).await == old_body,
        "new wire rewrote original report"
    );
    fx.store.close().await;
    // Tag-only negatives reach the exact decoder of the injected name: the
    // ordinary native name keeps the original strict report decoder, and only
    // the derived name reaches the derived decoder. Neither retags old history.
    for (tag, expected) in [
        ("rsia.formal_evaluation.v3", "typed envelope is unreadable"),
        (
            "rsia.formal_judgment.v1",
            "formal_judgment_derived_unreadable",
        ),
    ] {
        let mut fx = History::load(COMPLETE).await;
        let mut old: Value = serde_json::from_str(&fx.body(&fx.ticket_id).await).unwrap();
        old["payload"]["schema_version"] = json!(tag);
        let ticket = fx.ticket_id.clone();
        fx.replace_body(&ticket, &old).await;
        let before = image(&fx.path);
        assert!(
            matches!(IndependentEvaluationControl::recompute_judgment(&fx.evaluator, &fx.store, &ticket).await,
            Err(Error::Conflict(reason)) if reason == expected)
        );
        assert!(
            image(&fx.path) == before,
            "tag-only negative was migrated or refreshed"
        );
        fx.store.close().await;
    }
}

#[tokio::test]
async fn actual_v2_complete_history_reads_legacy_without_refresh() {
    let fx = History::load(COMPLETE).await;
    let before = image(&fx.path);
    assert_eq!(before["calls"].as_array().unwrap().len(), 122);
    assert!(
        before["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| call[0] == "finalized" && call[2] == 1)
    );
    assert_eq!(
        hash(fx.body(&fx.ticket_id).await.as_bytes()),
        fx.original["original_formal_body_sha256"].as_str().unwrap()
    );
    fx.assert_legacy().await;
    assert!(image(&fx.path) == before);
    fx.store.close().await;
    let reopened = Store::open(&fx.path).await.unwrap();
    let mut session = reopened.session().await.unwrap();
    let read = read_verified_formal_report_in_session(&fx.evaluator, &mut session, &fx.ticket_id)
        .await
        .unwrap();
    assert_eq!(read.state(), PersistentContractState::Legacy);
    assert!(!read.historical_view().promotion_eligible);
    session.commit().await.unwrap();
    assert!(image(&fx.path) == before);
    reopened.close().await;
}

#[tokio::test]
async fn actual_v1_certificate_history_keeps_original_stop_snapshot() {
    let fx = History::load(EARLY).await;
    let before = image(&fx.path);
    let body = fx.body(&fx.ticket_id).await;
    let value: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        value["payload"]["certificate"]["schema_version"],
        "rsia.early_stop_certificate.v1"
    );
    assert!(value["payload"]["certificate"]["k"].as_u64().unwrap() > 0);
    assert_eq!(before["calls"].as_array().unwrap().len(), 30);
    assert!(
        matches!(IndependentEvaluationControl::recompute_judgment(&fx.evaluator, &fx.store, &fx.ticket_id).await,
        Err(Error::Conflict(reason)) if reason == "formal_recompute_input_unavailable")
    );
    assert!(
        image(&fx.path) == before,
        "unavailable first-stop input caused writes"
    );
    assert!(
        fx.body(&fx.ticket_id).await == body,
        "first-stop certificate/body was refreshed"
    );
    let meta = status_metadata(&fx, "ag163-early-lineage").await;
    assert_eq!(meta["state"], "legacy");
    assert!(meta["input_digest"].is_null());
    assert_eq!(
        meta["lineage"],
        json!({"state":"unavailable","details":"original_stop_snapshot_unavailable"})
    );
    assert!(fx.body(&fx.ticket_id).await == body);
    preserves_old_facts(&before, &image(&fx.path));
    fx.store.close().await;
}

#[tokio::test]
async fn actual_v2_invalid_history_keeps_reason_order_and_paid_facts() {
    let fx = History::invalid().await;
    let before = image(&fx.path);
    let body = fx.body(&fx.ticket_id).await;
    let value: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        value["payload"]["reasons"],
        json!(["explicit_environment_invalid"])
    );
    assert_eq!(
        before["calls"],
        json!([
            ["finalized", 5, 1],
            ["finalized", 10, 1],
            ["finalized", 17, 1]
        ])
    );
    assert!(
        matches!(IndependentEvaluationControl::recompute_judgment(&fx.evaluator, &fx.store, &fx.ticket_id).await,
        Err(Error::Conflict(reason)) if reason == "formal_recompute_input_unavailable")
    );
    assert!(image(&fx.path) == before);
    assert!(fx.body(&fx.ticket_id).await == body);
    let meta = status_metadata(&fx, "ag163-invalid-lineage").await;
    assert_eq!(
        meta["lineage"],
        json!({"state":"unavailable","details":"original_invalid_snapshot_unavailable"})
    );
    preserves_old_facts(&before, &image(&fx.path));
    assert!(fx.body(&fx.ticket_id).await == body);
    fx.store.close().await;
}

#[tokio::test]
async fn unknown_judgment_version_is_quarantined_before_recompute() {
    let mut fx = History::load(COMPLETE).await;
    let id = fx.recompute(&fx.ticket_id).await;
    let mut wire: Value = serde_json::from_str(&fx.body(&id).await).unwrap();
    let future = format!("e05dv-{}", "0".repeat(64));
    assert!(
        FORMAL_JUDGMENT_VERSIONS
            .iter()
            .all(|row| row.domain.version_key().unwrap() != future)
    );
    wire["payload"]["judgment_version"] = json!(future);
    fx.replace_body(&id, &wire).await;
    let before = image(&fx.path);
    assert!(
        matches!(IndependentEvaluationControl::recompute_judgment(&fx.evaluator, &fx.store, &id).await,
        Err(Error::Invalid(reason)) if reason == "formal_judgment_version_outside_table")
    );
    assert!(
        image(&fx.path) == before,
        "table-outside judgment version was rewritten"
    );
    fx.store.close().await;
}

#[tokio::test]
async fn unreadable_and_identity_invalid_are_distinct_from_legacy() {
    for missing in [true, false] {
        let mut fx = History::load(COMPLETE).await;
        let id = fx.recompute(&fx.ticket_id).await;
        let mut wire: Value = serde_json::from_str(&fx.body(&id).await).unwrap();
        if missing {
            wire["payload"]
                .as_object_mut()
                .unwrap()
                .remove("input_snapshot");
        } else {
            wire["payload"]["report_id"] = json!("wrong-real-report-identity");
        }
        fx.replace_body(&id, &wire).await;
        let before = image(&fx.path);
        let expected = if missing {
            "formal_judgment_derived_unreadable"
        } else {
            "formal_judgment_derived_identity_changed"
        };
        assert!(
            matches!(IndependentEvaluationControl::recompute_judgment(&fx.evaluator, &fx.store, &id).await,
            Err(Error::Conflict(reason)) if reason == expected)
        );
        assert!(
            image(&fx.path) == before,
            "invalid/unreadable record was treated as refreshable legacy"
        );
        fx.store.close().await;
    }
}

#[tokio::test]
async fn missing_original_image_never_becomes_current_verified() {
    let mut fx = History::load(COMPLETE).await;
    let source_body = fx.body(&fx.ticket_id).await;
    let paid = image(&fx.path)["calls"].clone();
    fx.store.close().await;
    // Deliberate missing-original negative only. No healthy business row is made.
    let program = "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1]);c.execute('PRAGMA foreign_keys=OFF');c.execute('DELETE FROM objects WHERE namespace=? AND kind=\"artifact\" AND id=?',(sys.argv[2],sys.argv[3]));assert c.total_changes==1;c.commit();c.close()";
    let output = Command::new("python3")
        .args(["-I", "-B", "-c", program])
        .arg(&fx.path)
        .arg(fx.evaluator.namespace())
        .arg(sid("evaluation_resource_evidence_v1", &fx.ticket_id))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "owned missing-resource negative failed"
    );
    fx.store = Store::open(&fx.path).await.unwrap();
    let before = image(&fx.path);
    assert!(before["calls"] == paid);
    // The current registered Complete source requires its frozen resource.
    // A physically lost object is NotFound, never invented historical absence.
    assert!(matches!(
        IndependentEvaluationControl::recompute_judgment(&fx.evaluator, &fx.store, &fx.ticket_id)
            .await,
        Err(Error::NotFound)
    ));
    let mut session = fx.store.session().await.unwrap();
    assert!(matches!(
        read_verified_formal_report_in_session(&fx.evaluator, &mut session, &fx.ticket_id).await,
        Err(Error::NotFound)
    ));
    session.commit().await.unwrap();
    assert!(
        image(&fx.path) == before,
        "missing original image was filled from current state"
    );
    assert!(fx.body(&fx.ticket_id).await == source_body);
    fx.store.close().await;
}

#[tokio::test]
async fn current_diagnostic_semantics_do_not_grant_promotion_authority() {
    let fx = current_diagnostic_fixture(2, false).await;
    let issued = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap();
    assert!(issued.ticket.candidate_dependency_binding.is_some());
    assert!(issued.ticket.source_plan_binding.is_some());
    IndependentEvaluationControl::invalidate(
        &fx.evaluator,
        &fx.store,
        &fx.ticket_id,
        "missing_execution_rows",
        20,
    )
    .await
    .unwrap();
    let before = image(&fx.db_path);
    let mut session = fx.store.session().await.unwrap();
    let read = read_verified_formal_report_in_session(&fx.evaluator, &mut session, &fx.ticket_id)
        .await
        .unwrap();
    assert_eq!(read.state(), PersistentContractState::CurrentVerified);
    let view = read.current_view().unwrap();
    assert_eq!(view.evidence_scope, EvaluationEvidenceScope::ProgramFixture);
    assert!(!view.promotion_eligible);
    assert!(
        view.ineligibility_reasons
            .iter()
            .any(|reason| reason == "program_fixture_not_production_authority")
    );
    session.commit().await.unwrap();
    assert!(
        image(&fx.db_path) == before,
        "current diagnostic read changed records or granted a paid action"
    );
    assert!(
        before["calls"].as_array().unwrap().is_empty(),
        "this diagnostic never executes a provider/model"
    );
    fx.store.close().await;
}

#[tokio::test]
async fn explicit_recompute_requires_original_evaluator_and_real_sources() {
    let fx = History::load(COMPLETE).await;
    let before = image(&fx.path);
    for wrong in [
        Context::new(fx.evaluator.namespace(), "admin", Role::Admin).unwrap(),
        Context::new(fx.evaluator.namespace(), "wrong-evaluator", Role::Evaluator).unwrap(),
    ] {
        assert!(matches!(
            IndependentEvaluationControl::recompute_judgment(&wrong, &fx.store, &fx.ticket_id)
                .await,
            Err(Error::Forbidden)
        ));
        assert!(image(&fx.path) == before);
    }
    let other = Context::new("wrong-namespace", fx.evaluator.actor(), Role::Evaluator).unwrap();
    assert!(matches!(
        IndependentEvaluationControl::recompute_judgment(&other, &fx.store, &fx.ticket_id).await,
        Err(Error::NotFound)
    ));
    assert!(image(&fx.path) == before);
    let status = IndependentEvaluationControl::status(&fx.evaluator, &fx.store, &fx.ticket_id)
        .await
        .unwrap();
    let binding = status.ticket.candidate_dependency_binding.as_ref().unwrap();
    assert!(
        binding
            .sources
            .iter()
            .any(|source| source.kind == "run" && source.id == "S2")
    );
    let mut session = fx.store.session().await.unwrap();
    let original_source = session
        .object_row(&fx.admin, "run", "S2")
        .await
        .unwrap()
        .unwrap();
    let source_body: Value = serde_json::from_str(&original_source.body).unwrap();
    assert_eq!(source_body["schema_version"], "rsia.optimization.source.v1");
    assert_eq!(source_body["record"]["id"], "S2");
    assert!(
        session
            .dependents(&fx.admin, "run", "S2")
            .await
            .unwrap()
            .contains(&("artifact".into(), binding.candidate_id.clone()))
    );
    assert!(
        session
            .dependents(&fx.admin, "artifact", &binding.candidate_id)
            .await
            .unwrap()
            .contains(&(
                "artifact".into(),
                sid("evaluation_ticket_v2", &fx.ticket_id)
            ))
    );
    session.commit().await.unwrap();
    assert!(
        image(&fx.path) == before,
        "real upstream identity/edge health checks wrote original facts"
    );
    // A real derived judgment by the original writer over the same healthy source.
    let derived = fx.recompute(&fx.ticket_id).await;
    let derived_body = fx.body(&derived).await;
    let paid_before = before["calls"].clone();
    let original_formal = fx.body(&fx.ticket_id).await;
    let mut status = LifecycleStore::begin_revoke(
        &fx.admin,
        &fx.store,
        TypedObjectRef {
            kind: "run".into(),
            id: "S2".into(),
        },
        "owned real upstream source-revocation negative",
        evo_core::now().unwrap(),
    )
    .await
    .unwrap();
    let revoked = image(&fx.path);
    assert!(revoked["calls"] == paid_before);
    for old in before["tables"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|table| table["name"].as_str().unwrap().starts_with("root_budget"))
    {
        let unchanged = table(&revoked, old["name"].as_str().unwrap());
        assert!(
            old == unchanged,
            "logical revocation changed complete original budget facts"
        );
    }
    assert!(
        fx.body(&fx.ticket_id).await == original_formal,
        "logical revocation rewrote original formal report"
    );
    assert!(matches!(
        IndependentEvaluationControl::recompute_judgment(&fx.evaluator, &fx.store, &fx.ticket_id)
            .await,
        Err(Error::Forbidden)
    ));
    assert!(
        image(&fx.path) == revoked,
        "revoked original caused a new derivation"
    );
    // Real cleanup of that closure: only the exact derived name reaches the
    // derived redactor; the ordinary original report and paid facts are kept.
    for _ in 0..100 {
        if matches!(
            status.state,
            evo_storage::lifecycle::CleanupState::Complete
                | evo_storage::lifecycle::CleanupState::Failed
        ) {
            break;
        }
        status = LifecycleStore::cleanup_step(
            &fx.admin,
            &fx.store,
            &status.job_id,
            16,
            evo_core::now().unwrap(),
        )
        .await
        .unwrap();
    }
    assert_eq!(status.state, evo_storage::lifecycle::CleanupState::Complete);
    assert!(status.last_error.is_none());
    assert!(
        fx.body(&fx.ticket_id).await == original_formal,
        "cleanup changed the ordinary original report"
    );
    let redacted: Value = serde_json::from_str(&fx.body(&derived).await).unwrap();
    assert_eq!(redacted["schema_version"], "rsia.redacted.v1");
    assert_eq!(redacted["original_digest"], hash(derived_body.as_bytes()));
    assert_eq!(redacted["metadata"]["report_id"], derived);
    assert_eq!(redacted["metadata"]["source_ticket_id"], fx.ticket_id);
    assert!(
        image(&fx.path)["calls"] == paid_before,
        "cleanup changed actual paid/None/closed facts"
    );
    fx.store.close().await;
}

#[tokio::test]
async fn explicit_recompute_gets_new_id_and_queryable_lineage() {
    let fx = History::load(COMPLETE).await;
    let old_body = fx.body(&fx.ticket_id).await;
    let before = image(&fx.path);
    let new_id = fx.recompute(&fx.ticket_id).await;
    assert_ne!(new_id, fx.ticket_id);
    assert!(new_id.starts_with("e05j-"));
    let raw: Value = serde_json::from_str(&fx.body(&new_id).await).unwrap();
    let p = &raw["payload"];
    assert_eq!(raw["id"], sid(FORMAL_KIND, &new_id));
    assert_eq!(raw["record_kind"], FORMAL_KIND);
    assert_eq!(p.as_object().unwrap().len(), 8);
    assert_eq!(p["schema_version"], "rsia.formal_judgment.v1");
    assert_eq!(p["source_ticket_id"], fx.ticket_id);
    assert_eq!(p["input_snapshot"]["common"][0], fx.evaluator.namespace());
    let input = &p["input_snapshot"];
    let expected_digest = canonical_fingerprint(&json!([
        "rsia.formal_judgment_input.v1",
        input["common"],
        "complete_batch",
        input["variant"]["input"]
    ]))
    .unwrap();
    assert_eq!(p["input_digest"], expected_digest);
    let expected_id = format!(
        "e05j-{}",
        &canonical_fingerprint(&json!([
            fx.evaluator.namespace(),
            fx.ticket_id,
            p["input_digest"],
            p["judgment_version"]
        ]))
        .unwrap()[..32]
    );
    assert_eq!(new_id, expected_id);
    assert!(fx.body(&fx.ticket_id).await == old_body);
    preserves_old_facts(&before, &image(&fx.path));
    let stable = image(&fx.path);
    assert_eq!(fx.recompute(&fx.ticket_id).await, new_id);
    assert!(image(&fx.path) == stable);
    assert_eq!(fx.recompute(&new_id).await, new_id);
    assert!(
        image(&fx.path) == stable,
        "v3 normalized itself into another source"
    );
    let meta = status_metadata(&fx, "ag163-known-lineage").await;
    assert_eq!(meta["lineage"]["state"], "known");
    assert_eq!(meta["lineage"]["details"][0], p["input_digest"]);
    let entries = meta["lineage"]["details"][1].as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|e| e[1] == fx.ticket_id && e[2] == true)
            .count(),
        1
    );
    assert_eq!(
        entries
            .iter()
            .filter(|e| e[1] == new_id && e[2] == true)
            .count(),
        1
    );
    assert_eq!(entries.iter().filter(|e| e[2] == true).count(), 2);
    assert!(fx.body(&fx.ticket_id).await == old_body);
    preserves_old_facts(&before, &image(&fx.path));
    eprintln!("AG163_RUST_ID_GOLDEN={} {}", p["judgment_version"], new_id);
    fx.store.close().await;
}

#[tokio::test]
async fn same_id_changed_report_conflicts_without_mutating_history() {
    let mut fx = History::load(COMPLETE).await;
    let id = fx.recompute(&fx.ticket_id).await;
    let old = fx.body(&fx.ticket_id).await;
    let mut forged: Value = serde_json::from_str(&fx.body(&id).await).unwrap();
    forged["payload"]["judgment_version"] = json!(format!("e05dv-{}", "0".repeat(64)));
    fx.replace_body(&id, &forged).await;
    let before = image(&fx.path);
    assert!(
        matches!(IndependentEvaluationControl::recompute_judgment(&fx.evaluator, &fx.store, &fx.ticket_id).await, Err(Error::Conflict(reason)) if reason == "formal_judgment_same_id_different_content")
    );
    assert!(
        image(&fx.path) == before,
        "same-ID conflicting body was overwritten or new facts added"
    );
    assert!(fx.body(&fx.ticket_id).await == old);
    fx.store.close().await;
}

#[tokio::test]
async fn restore_retains_judgment_versions_and_spent_facts_without_reopening_execution() {
    let fx = History::load(COMPLETE).await;
    let id = fx.recompute(&fx.ticket_id).await;
    let body = fx.body(&id).await;
    let old_body = fx.body(&fx.ticket_id).await;
    let before = image(&fx.path);
    let backup = fx.root.path().join("backup");
    fx.store.backup(&backup).await.unwrap();
    fx.store.close().await;
    let manifest_raw = fs::read(backup.join("backup-manifest.json")).unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_raw).unwrap();
    let namespaces: Vec<Value> = manifest["watermarks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            json!({
                "namespace":w["namespace"],"base_seq":w["seq"],"base_digest":w["digest"],
                "events":[],"latest_seq":w["seq"],"latest_digest":w["digest"]
            })
        })
        .collect();
    let delta = fx.root.path().join("restore-delta.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&delta)
        .unwrap();
    file.write_all(
        &serde_json::to_vec(&json!({"schema_version":"rsia.revoke_delta.v1",
        "base_manifest_sha256":hash(&manifest_raw),"namespaces":namespaces}))
        .unwrap(),
    )
    .unwrap();
    drop(file);
    let restored = fx.root.path().join("restored");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/restore_backup.py");
    let output = Command::new("python3")
        .arg("-B")
        .arg(script)
        .arg("--backup")
        .arg(&backup)
        .arg("--dest")
        .arg(&restored)
        .arg("--revoke-delta")
        .arg(&delta)
        .arg("--trusted-revocations-db")
        .arg(&fx.path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "actual restore_backup.py rejected the healthy backup"
    );
    let path = restored.join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    assert_eq!(store.integrity().await.unwrap(), "ok");
    let mut session = store.session().await.unwrap();
    let old = session
        .object_row(&fx.evaluator, "artifact", &sid(FORMAL_KIND, &fx.ticket_id))
        .await
        .unwrap()
        .unwrap();
    let new = session
        .object_row(&fx.evaluator, "artifact", &sid(FORMAL_KIND, &id))
        .await
        .unwrap()
        .unwrap();
    assert!(
        old.body == old_body && new.body == body,
        "restore changed original/new judgment wire bytes"
    );
    let read = read_verified_formal_report_in_session(&fx.evaluator, &mut session, &fx.ticket_id)
        .await
        .unwrap();
    assert_eq!(read.state(), PersistentContractState::Legacy);
    assert!(!read.historical_view().promotion_eligible);
    session.commit().await.unwrap();
    let restored_image = image(&path);
    assert!(restored_image["calls"] == before["calls"]);
    assert!(
        restore_logical_tables(&restored_image) == restore_logical_tables(&before),
        "actual restore changed stored column types/raw bytes/accounting/audit/sequence"
    );
    assert_eq!(
        IndependentEvaluationControl::recompute_judgment(&fx.evaluator, &store, &id)
            .await
            .unwrap(),
        id
    );
    assert!(
        image(&path) == restored_image,
        "restored same-version recompute reopened execution or wrote again"
    );
    store.close().await;
}
