//! Head-only stored-review acceptance; the old-API HTTP parent baseline is
//! separately preserved by ROOT. All writes below are isolated fixture setup.
use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, Profile, ResolvedBundle, SkillPatch,
    SkillSnapshot, compile_bundle,
};
use evo_core::evaluation::{
    ControlCondition, ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec,
    FormalExperimentPlanV41, FormalStatisticalUnit, Money, OptimizationBudgetPlan,
    OptimizerComparisonContract, ProfileKind,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::holdout::{
    AnchorCoverageMatrix, CriticalCapabilityCoverage, FrozenCandidatePair, HoldoutManifest,
    SequentialMode,
};
use evo_core::optimization::{
    MergeRecord, ModelRequestContext, ModelStage, OptimizationTrace, TraceOutcome,
};
use evo_core::sequential::{
    AlphaAllocation, EarlyStopPlan, FormalClaimKind, ResearchFamilyAlphaPlan,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, compile_skill_edit_batch,
    skill_snapshot_digest,
};
use evo_core::{Context, Error, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, ingest_trusted_run_records, store_trace_authority,
};
use evo_engine::optimization::{
    DevelopmentManifest, DevelopmentRunRequest, DevelopmentSelection, DevelopmentSelectionDecision,
    DevelopmentTask, OptimizationJournal, OptimizationJournalStage, StageDependency, StageFact,
    StageFactKind, StepTerminalClass, StoreOptimizationJournal,
};
use evo_engine::release_store::{
    ReleaseCandidateRecord, ReleaseStore, StageBundleRequest, TypedSourceRef,
};
use evo_engine::review::{
    ReviewDetail, ReviewGap, ReviewKind, ReviewPayload, ReviewRequest, ReviewResponse,
    review_stored,
};
use evo_engine::service::HostService;
use evo_engine::streaming_evaluator::{
    EvaluationEvidenceScope, ExecutionReceiptRequest, ExecutionSide, FixedGraderMethod,
    FixedGraderSpec, FrozenOracleEntry, GraderReceiptRequest, IndependentEvaluationControl,
    IssueTicketRequest, ProtectedHoldoutRecord, ProtectedInputRef, RegisteredEvaluationControl,
    StartExecutionRequest,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallFence, BudgetCallRef, BudgetCallReservation, BudgetStage, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{REVOKE_TOMBSTONE_SCHEMA, RevokeTombstone};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    future::{Future, poll_fn},
    path::PathBuf,
    process::Command,
    task::Poll,
};

const NS: &str = "review-fixture";
fn d(label: &str) -> String {
    hash(label.as_bytes())
}
fn ctx(actor: &str, role: Role) -> Context {
    Context::new(NS, actor, role).unwrap()
}
fn parent() -> SkillSnapshot {
    SkillSnapshot {
        content: "ab中文🙂cd".into(),
        applicability: "ABCDE".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    }
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
struct MaterialFixture {
    _dir: tempfile::TempDir,
    db_path: PathBuf,
    store: Store,
    admin: Context,
    host: Context,
    profile: Profile,
    parent: SkillSnapshot,
    baseline: SkillSnapshot,
    parent_strategy: Strategy,
    baseline_strategy: Strategy,
    caps: HostCapabilities,
    sources: Vec<StoredTraceAuthority>,
    candidate: ReleaseCandidateRecord,
}
async fn material_fixture(parent: SkillSnapshot) -> MaterialFixture {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("stored-review.sqlite3");
    let store = Store::open(&db_path).await.unwrap();
    let admin = ctx("admin", Role::Admin);
    let host = ctx("host", Role::Host);
    let sources = vec![
        source("source-one", "first trusted source"),
        source("source-two", "unselected trusted support"),
    ];
    for authority in &sources {
        store_trace_authority(&store, &host, authority)
            .await
            .unwrap();
    }
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .bump_watermark(&admin, &d("watermark-one"))
            .await
            .unwrap(),
        1
    );
    session.commit().await.unwrap();
    let profile = Profile {
        id: "review-profile".into(),
        evolution_enabled: true,
        parent_digest: d("declared-parent"),
        baseline_digest: d("declared-baseline"),
    };
    let baseline = SkillSnapshot {
        content: "Safe baseline rule".into(),
        applicability: "Safe baseline applies".into(),
        counterexample: "Safe baseline counterexample".into(),
        required_capabilities: parent.required_capabilities.clone(),
        dependencies: parent.dependencies.clone(),
    };
    let caps = HostCapabilities {
        available: parent.required_capabilities.iter().cloned().collect(),
        granted: parent.required_capabilities.iter().cloned().collect(),
    };
    let bundle = compile_bundle(CompileParts {
        profile: &profile,
        parent: &parent,
        baseline: &baseline,
        parent_strategy: &Strategy::default(),
        baseline_strategy: &Strategy::default(),
        skill_patch: &SkillPatch::default(),
        improver_patch: &ImproverPatch::default(),
        caps: &caps,
        revoked: &BTreeSet::new(),
    })
    .unwrap();
    let proposer = ctx("proposer", Role::Worker);
    let candidate = ReleaseStore::stage_bundle(
        &proposer,
        &store,
        StageBundleRequest {
            candidate_id: "stored-candidate".into(),
            environment_digest: d("environment"),
            bundle,
            proposer_actor: proposer.actor().into(),
            sources: sources
                .iter()
                .map(|s| TypedSourceRef {
                    kind: "run".into(),
                    id: s.record.id.clone(),
                    content_digest: s.trace.source_digest.clone(),
                })
                .collect(),
            revoke_watermark: 1,
        },
    )
    .await
    .unwrap();
    MaterialFixture {
        _dir: dir,
        db_path,
        store,
        admin,
        host,
        profile,
        parent,
        baseline,
        parent_strategy: Strategy::default(),
        baseline_strategy: Strategy::default(),
        caps,
        sources,
        candidate,
    }
}
async fn review(
    fixture: &MaterialFixture,
    kind: ReviewKind,
    id: &str,
    detail: ReviewDetail,
) -> ReviewResponse {
    review_stored(
        &fixture.admin,
        &fixture.store,
        ReviewRequest {
            kind,
            id: id.into(),
            detail,
        },
    )
    .await
    .unwrap()
}
fn as_json(response: &ReviewResponse) -> Value {
    serde_json::to_value(response).unwrap()
}
fn assert_metadata(value: &Value) {
    match value {
        Value::Object(object) => {
            if object.contains_key("original_field_sha256") {
                assert!(object["display"].is_null());
                assert!(object["display_sha256"].is_null());
            }
            for child in object.values() {
                assert_metadata(child);
            }
        }
        Value::Array(array) => {
            for child in array {
                assert_metadata(child);
            }
        }
        _ => {}
    }
}
fn assert_four_null(value: &Value) {
    for key in [
        "original_field_sha256",
        "original_bytes",
        "display",
        "display_sha256",
    ] {
        assert!(value[key].is_null(), "blocked field leaked {key}");
    }
}

// Full logical catalog, including schema and duplicate rows, preserving each
// SQLite storage class and the exact UTF-8/BLOB bytes. This helper is read-only.
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
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "read-only catalog reader failed");
    serde_json::from_slice(&output.stdout).unwrap()
}

struct EditFixture {
    prepared: StageFact,
    compiled: StageFact,
    template: SkillEditBatch,
    context: ModelRequestContext,
    development: DevelopmentRunRequest,
    bundle: ResolvedBundle,
}
fn dependencies(
    fixture: &MaterialFixture,
    grant: &str,
    prepared: Option<&str>,
) -> Vec<StageDependency> {
    let mut values: Vec<_> = fixture
        .sources
        .iter()
        .map(|s| StageDependency {
            kind: "run".into(),
            id: s.record.id.clone(),
        })
        .collect();
    values.push(StageDependency {
        kind: "revoke_watermark".into(),
        id: "1".into(),
    });
    values.push(StageDependency {
        kind: "artifact".into(),
        id: grant.into(),
    });
    if let Some(id) = prepared {
        values.push(StageDependency {
            kind: "artifact".into(),
            id: id.into(),
        });
    }
    values.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    values
}
fn fact(
    kind: StageFactKind,
    stage: OptimizationJournalStage,
    input_digest: String,
    deps: Vec<StageDependency>,
    payload: Value,
) -> StageFact {
    StageFact {
        schema_version: "rsia.optimization.stage_fact.v1".into(),
        artifact_id: String::new(),
        namespace: NS.into(),
        episode_id: "review-episode".into(),
        step: 1,
        attempt: 1,
        stage,
        kind,
        request_id: "review-request".into(),
        input_digest,
        output_digest: None,
        dependencies: deps,
        payload,
    }
    .seal()
    .unwrap()
}
async fn seed_edit(fixture: &MaterialFixture, edits: Vec<SkillTextEdit>) -> EditFixture {
    let source_refs: Vec<_> = fixture
        .sources
        .iter()
        .map(|s| EvidenceRef {
            id: s.record.id.clone(),
            digest: s.trace.source_digest.clone(),
        })
        .collect();
    let selection = SourceSelection {
        roots: vec![],
        run_ids: fixture
            .sources
            .iter()
            .map(|s| s.record.id.clone())
            .collect(),
        purpose: Purpose::Development,
        allow_model_excerpts: true,
    };
    let evidence = ingest_trusted_run_records(
        &selection,
        &fixture
            .sources
            .iter()
            .map(|s| s.record.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let grant_id = format!("optgrant-{}", fingerprint(&selection).unwrap());
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &grant_id,
            fixture.admin.actor(),
            &selection,
        )
        .await
        .unwrap();
    for source in &fixture.sources {
        session
            .put_edge(
                &fixture.admin,
                "artifact",
                &grant_id,
                "run",
                &source.record.id,
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let trusted = TrustedEditContext::new(
        NS,
        &fixture.profile.id,
        "skill-review",
        "v1",
        &fixture.profile.parent_digest,
        &fixture.profile.baseline_digest,
        &fixture.parent,
        source_refs.clone(),
    )
    .unwrap();
    let template = SkillEditBatch {
        schema_version: SKILL_EDIT_SCHEMA.into(),
        compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
        namespace: NS.into(),
        profile_id: fixture.profile.id.clone(),
        skill_id: "skill-review".into(),
        skill_version: "v1".into(),
        input_digest: skill_snapshot_digest(&fixture.parent).unwrap(),
        approved_parent_digest: fixture.profile.parent_digest.clone(),
        safe_baseline_digest: fixture.profile.baseline_digest.clone(),
        evidence: EvidenceClosure {
            support: vec![],
            counterexamples: vec![],
            dependencies: vec![],
        },
        edits: vec![],
    };
    let context = ModelRequestContext {
        request_id: "review-request".into(),
        namespace: NS.into(),
        purpose: Purpose::Development,
        stage: ModelStage::ReflectFailure,
        episode_id: "review-episode".into(),
        step: 1,
        attempt: 1,
        parent_skill_digest: template.input_digest.clone(),
        bundle_digest: d("incumbent-bundle"),
        source_closure: source_refs.clone(),
        model_digest: d("model"),
        tools_digest: d("tools"),
        rules_digest: d("rules"),
        sampling_digest: d("sampling"),
        revoke_watermark: 1,
        max_suggestions: 4,
    };
    let development = DevelopmentRunRequest {
        request_id: "development-original".into(),
        namespace: NS.into(),
        purpose: Purpose::Development,
        episode_id: context.episode_id.clone(),
        step: 1,
        attempt: 1,
        manifest: DevelopmentManifest::build(
            "dev-manifest",
            vec![DevelopmentTask {
                id: "dev-task".into(),
                parent_family: "dev-family".into(),
                input_digest: d("dev-input"),
            }],
        )
        .unwrap(),
        parent_bundle_digest: context.bundle_digest.clone(),
        candidate_bundle_digest: d("candidate-placeholder"),
        environment_digest: d("environment"),
        grader_digest: d("grader"),
        rules_digest: d("rules"),
        tools_digest: d("tools"),
        revoke_watermark: 1,
        idempotency_key: "dev-idempotency".into(),
    };
    let prepared_payload = json!({"evidence":evidence,"selection":selection,
        "bindings":fixture.sources.iter().map(|s| (&s.record.id,&s.trace.source_digest,&s.record.parent_family)).collect::<Vec<_>>(),
        "traces":fixture.sources.iter().map(|s| s.trace.clone()).collect::<Vec<_>>(),"context":context,"parent":fixture.parent,
        "trusted_edit_context":format!("{trusted:?}"),"edit_template":template,"protected_ranges":"[]","profile":fixture.profile,
        "baseline":fixture.baseline,"parent_strategy":fixture.parent_strategy,"baseline_strategy":fixture.baseline_strategy,
        "improver":ImproverPatch::default(),"caps":fixture.caps,"revoked":BTreeSet::<String>::new(),"development":development,"allow_rank":false});
    let prepared = fact(
        StageFactKind::StepPrepared,
        OptimizationJournalStage::Merge,
        fingerprint(&prepared_payload).unwrap(),
        dependencies(fixture, &grant_id, None),
        prepared_payload,
    );
    let journal = StoreOptimizationJournal::new(
        fixture.store.clone(),
        fixture.admin.clone(),
        fixture.admin.actor(),
    )
    .unwrap();
    journal.commit(prepared.clone()).await.unwrap();
    let selected: Vec<_> = (0..edits.len())
        .map(|i| format!("suggestion-{i}"))
        .collect();
    let mut input = selected.clone();
    input.push("unselected-suggestion".into());
    let merged = MergeRecord {
        input_suggestion_ids: input,
        selected_suggestion_ids: selected,
        read_dependencies: source_refs.clone(),
        evidence: EvidenceClosure {
            support: vec![source_refs[0].clone()],
            counterexamples: vec![source_refs[1].clone()],
            dependencies: source_refs,
        },
        edits,
    };
    let mut batch = template.clone();
    batch.evidence = merged.evidence.clone();
    batch.edits = merged.edits.clone();
    // The existing compiler runs in fixture setup, never inside review.
    let compiled = compile_skill_edit_batch(&fixture.parent, &trusted, &batch, &[]).unwrap();
    let bundle = compile_bundle(CompileParts {
        profile: &fixture.profile,
        parent: &fixture.parent,
        baseline: &fixture.baseline,
        parent_strategy: &fixture.parent_strategy,
        baseline_strategy: &fixture.baseline_strategy,
        skill_patch: &compiled.patch,
        improver_patch: &ImproverPatch::default(),
        caps: &fixture.caps,
        revoked: &BTreeSet::new(),
    })
    .unwrap();
    let payload = json!({"output":compiled.output,"patch":compiled.patch,"report":compiled.report,"bundle":bundle,"merge":merged});
    let compiled = fact(
        StageFactKind::EditCompiled,
        OptimizationJournalStage::EditCompile,
        fingerprint(&batch).unwrap(),
        dependencies(fixture, &grant_id, Some(&prepared.artifact_id)),
        payload,
    );
    journal.commit(compiled.clone()).await.unwrap();
    EditFixture {
        prepared,
        compiled,
        template,
        context,
        development,
        bundle,
    }
}
fn edit(
    parent: &SkillSnapshot,
    field: SkillTextField,
    start: usize,
    end: usize,
    operation: TextEditOperation,
) -> SkillTextEdit {
    let text = match field {
        SkillTextField::Content => &parent.content,
        SkillTextField::Applicability => &parent.applicability,
        SkillTextField::Counterexample => &parent.counterexample,
    };
    SkillTextEdit {
        field,
        start,
        end,
        expected_text_digest: hash(text.get(start..end).unwrap().as_bytes()),
        exact_anchor: None,
        operation,
    }
}
async fn replace_fact(fixture: &MaterialFixture, fact: &StageFact) {
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &fact.artifact_id,
            fixture.admin.actor(),
            fact,
        )
        .await
        .unwrap();
    for reference in &fact.dependencies {
        session
            .put_edge(
                &fixture.admin,
                "artifact",
                &fact.artifact_id,
                &reference.kind,
                &reference.id,
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
}

#[tokio::test]
async fn typed_candidate_is_exact_or_metadata_but_never_infers_history_formal_or_release() {
    let mut p = parent();
    p.content = "Cafe\u{301} 中文🙂".into();
    let fixture = material_fixture(p).await;
    let before = catalog(&fixture.db_path);
    let metadata = review(
        &fixture,
        ReviewKind::TypedCandidate,
        &fixture.candidate.id,
        ReviewDetail::Metadata,
    )
    .await;
    assert_metadata(&as_json(&metadata));
    assert!(!metadata.diff_complete);
    for gap in [
        ReviewGap::TypedCandidateHistoryUnbound,
        ReviewGap::TypedcandidateformalUnbound,
        ReviewGap::ParentMaterialUnbound,
        ReviewGap::BaselineMaterialUnbound,
        ReviewGap::HostApplicationUnbound,
        ReviewGap::ApprovalUnbound,
        ReviewGap::RollbackTargetUnbound,
    ] {
        assert!(metadata.gaps.contains(&gap));
    }
    let exact = review(
        &fixture,
        ReviewKind::TypedCandidate,
        &fixture.candidate.id,
        ReviewDetail::Exact,
    )
    .await;
    let value = as_json(&exact);
    assert_eq!(value["schema_version"], "rsia.review.v1");
    let content = &value["payload"]["materials"]["bundle"]["skill"]["content"];
    assert_eq!(content["display"], fixture.parent.content);
    assert_eq!(content["original_bytes"], fixture.parent.content.len());
    assert_eq!(
        content["original_field_sha256"],
        hash(fixture.parent.content.as_bytes())
    );
    assert!(value["trusted_tokens"].is_null() && value["total_cost"].is_null());
    assert!(!exact.diff_complete);
    assert!(
        catalog(&fixture.db_path) == before,
        "review changed the complete business catalog"
    );
}

#[tokio::test]
async fn actual_ascii_unicode_insert_replace_delete_diff_is_complete_and_read_only() {
    let fixture = material_fixture(parent()).await;
    let edits = vec![
        edit(
            &fixture.parent,
            SkillTextField::Applicability,
            2,
            2,
            TextEditOperation::Insert { text: "XY".into() },
        ),
        edit(
            &fixture.parent,
            SkillTextField::Content,
            2,
            8,
            TextEditOperation::Replace {
                text: "新🙂".into(),
            },
        ),
        edit(
            &fixture.parent,
            SkillTextField::Content,
            12,
            14,
            TextEditOperation::Delete,
        ),
    ];
    let stored = seed_edit(&fixture, edits).await;
    let before = catalog(&fixture.db_path);
    let metadata = review(
        &fixture,
        ReviewKind::StageFact,
        &stored.compiled.artifact_id,
        ReviewDetail::Metadata,
    )
    .await;
    assert_metadata(&as_json(&metadata));
    assert!(!metadata.diff_complete);
    let exact = review(
        &fixture,
        ReviewKind::StageFact,
        &stored.compiled.artifact_id,
        ReviewDetail::Exact,
    )
    .await;
    assert!(
        exact.diff_complete,
        "expected a fully bound exact stored diff: {:?}",
        exact.gaps
    );
    assert!(exact.gaps.contains(&ReviewGap::MissingRejectionReason));
    let value = as_json(&exact);
    let materials = &value["payload"]["materials"];
    assert_eq!(
        materials["bundle"]["skill"]["content"]["display"],
        "ab新🙂🙂"
    );
    assert_eq!(
        materials["bundle"]["skill"]["applicability"]["display"],
        "ABXYCDE"
    );
    assert_eq!(materials["diff"]["edits"][1]["start"], 2);
    assert_eq!(materials["diff"]["edits"][1]["end"], 8);
    assert_eq!(materials["diff"]["edits"][1]["removed"]["display"], "中文");
    assert_eq!(materials["diff"]["edits"][1]["inserted"]["display"], "新🙂");
    assert_eq!(materials["diff"]["edits"][2]["removed"]["display"], "cd");
    assert_eq!(materials["sources"].as_array().unwrap().len(), 2);
    assert!(
        catalog(&fixture.db_path) == before,
        "review changed full rows/BLOB catalog"
    );
}

#[tokio::test]
async fn seven_real_variants_keep_terminal_classes_and_unverified_selection_unbound() {
    let fixture = material_fixture(parent()).await;
    let stored = seed_edit(
        &fixture,
        vec![edit(
            &fixture.parent,
            SkillTextField::Content,
            0,
            2,
            TextEditOperation::Replace { text: "new".into() },
        )],
    )
    .await;
    let mut ids = vec![
        stored.prepared.artifact_id.clone(),
        stored.compiled.artifact_id.clone(),
    ];
    for (kind, class) in [
        (
            StageFactKind::TerminalRejected,
            StepTerminalClass::GrantUnavailable,
        ),
        (
            StageFactKind::TerminalNoChange,
            StepTerminalClass::NoEditSuggestions,
        ),
        (
            StageFactKind::TerminalUncertain,
            StepTerminalClass::ModelUsageUnknown,
        ),
    ] {
        let payload = json!({"reason":class.code(),"class":class});
        let terminal = fact(
            kind,
            OptimizationJournalStage::Development,
            fingerprint(&payload).unwrap(),
            stored.compiled.dependencies.clone(),
            payload,
        );
        replace_fact(&fixture, &terminal).await;
        ids.push(terminal.artifact_id);
    }
    let selection = DevelopmentSelection {
        request_id: "unbound-development".into(),
        manifest_digest: stored.development.manifest.digest.clone(),
        parent_bundle_digest: stored.context.bundle_digest.clone(),
        candidate_bundle_digest: stored.bundle.digest.clone(),
        decision: DevelopmentSelectionDecision::AcceptCandidate,
        parent_total_micros: 100_000,
        candidate_total_micros: 200_000,
        reason: "Fixture selection is not verified authority".into(),
    };
    let payload = json!({"status":"candidate","output":stored.compiled.payload["output"],"patch":stored.compiled.payload["patch"],
        "report":stored.compiled.payload["report"],"bundle":stored.bundle,"selection":selection});
    let mut deps = stored.compiled.dependencies.clone();
    deps.push(StageDependency {
        kind: "artifact".into(),
        id: stored.compiled.artifact_id.clone(),
    });
    let terminal = fact(
        StageFactKind::TerminalCandidate,
        OptimizationJournalStage::Development,
        fingerprint(&stored.context).unwrap(),
        deps.clone(),
        payload.clone(),
    );
    replace_fact(&fixture, &terminal).await;
    ids.push(terminal.artifact_id.clone());
    deps.push(StageDependency {
        kind: "artifact".into(),
        id: terminal.artifact_id,
    });
    let completed = fact(
        StageFactKind::StepCompleted,
        OptimizationJournalStage::Merge,
        stored.prepared.input_digest.clone(),
        deps,
        payload,
    );
    replace_fact(&fixture, &completed).await;
    ids.push(completed.artifact_id);
    let before = catalog(&fixture.db_path);
    for id in &ids {
        let response = review(&fixture, ReviewKind::StageFact, id, ReviewDetail::Exact).await;
        assert!(
            response.payload.is_some(),
            "allowed kind lacked safe material: {:?}",
            response.gaps
        );
        if id != &stored.compiled.artifact_id {
            assert!(!response.diff_complete);
        }
    }
    let legacy_payload = json!({"status":"no_change","reason":"old secret ghp_untrusted-reason"});
    let legacy = fact(
        StageFactKind::StepCompleted,
        OptimizationJournalStage::Merge,
        stored.prepared.input_digest,
        stored
            .prepared
            .dependencies
            .clone()
            .into_iter()
            .chain([StageDependency {
                kind: "artifact".into(),
                id: stored.prepared.artifact_id,
            }])
            .collect(),
        legacy_payload,
    );
    replace_fact(&fixture, &legacy).await;
    let response = review(
        &fixture,
        ReviewKind::StageFact,
        &legacy.artifact_id,
        ReviewDetail::Exact,
    )
    .await;
    let serialized = serde_json::to_string(&response).unwrap();
    assert!(serialized.contains("legacy_unclassified"));
    assert!(!serialized.contains("untrusted-reason"));
    assert!(!serialized.contains("ghp_"));
    // The legacy write is explicit fixture mutation; compare a new pure-read baseline.
    let after_setup = catalog(&fixture.db_path);
    review(
        &fixture,
        ReviewKind::StageFact,
        &legacy.artifact_id,
        ReviewDetail::Metadata,
    )
    .await;
    assert!(catalog(&fixture.db_path) == after_setup);
    assert!(!before.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn every_finite_privacy_class_masks_the_whole_field_and_derived_hashes() {
    let markers = [
        "BEGIN PRIVATE KEY",
        "GhP_fixture",
        "/Users/fixture/item",
        "GROUND_TRUTH: hidden",
        "192.168.1.2",
        r#"{"role" : "user", "content":"private"}"#,
        "password=fixture",
        "TODO: secret",
        "/absolute/item",
        "C:\\private\\item",
        "file:///item",
        "line\ncontrol",
        "\u{202e}hidden",
        "位置：/opt/acme/config.db",
        "位置“/opt/acme/config.db”",
        "中文/opt/acme/config.db",
        "中文C:\\private\\item🙂",
    ];
    for marker in markers {
        for field in [
            format!("{marker} 中文🙂suffix"),
            format!("prefix中文🙂{marker}"),
            format!("prefix中文 {marker} 🙂suffix"),
        ] {
            let mut skill = parent();
            skill.content = field;
            let fixture = material_fixture(skill).await;
            for detail in [ReviewDetail::Metadata, ReviewDetail::Exact] {
                let response = review(
                    &fixture,
                    ReviewKind::TypedCandidate,
                    &fixture.candidate.id,
                    detail,
                )
                .await;
                let value = as_json(&response);
                let bundle = &value["payload"]["materials"]["bundle"];
                assert_four_null(&bundle["skill"]["content"]);
                assert!(bundle["skill_snapshot_digest"].is_null());
                let origin = bundle["origins"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|o| o["field"] == "skill.content")
                    .unwrap();
                assert!(origin["value_digest"].is_null());
                assert!(!response.diff_complete);
                assert!(!serde_json::to_string(&response).unwrap().contains(marker));
            }
        }
    }
    let mut skill = parent();
    skill.content = "abc harmless text then ghp_fixture-secret".into();
    let fixture = material_fixture(skill).await;
    let stored = seed_edit(
        &fixture,
        vec![edit(
            &fixture.parent,
            SkillTextField::Content,
            0,
            3,
            TextEditOperation::Replace { text: "new".into() },
        )],
    )
    .await;
    let response = review(
        &fixture,
        ReviewKind::StageFact,
        &stored.compiled.artifact_id,
        ReviewDetail::Exact,
    )
    .await;
    let value = as_json(&response);
    let diff = &value["payload"]["materials"]["diff"];
    assert_four_null(&diff["edits"][0]["removed"]);
    assert_four_null(&diff["edits"][0]["inserted"]);
    assert!(diff["edits"][0]["start"].is_null() && diff["edits"][0]["end"].is_null());
    assert!(!response.diff_complete);
}

#[tokio::test]
async fn a_masked_edit_fragment_cannot_leak_positions_or_aggregate_length() {
    let mut skill = parent();
    skill.content = "api".into();
    let fixture = material_fixture(skill).await;
    let stored = seed_edit(
        &fixture,
        vec![edit(
            &fixture.parent,
            SkillTextField::Content,
            3,
            3,
            TextEditOperation::Insert { text: "/v1".into() },
        )],
    )
    .await;
    let before = catalog(&fixture.db_path);
    let response = review(
        &fixture,
        ReviewKind::StageFact,
        &stored.compiled.artifact_id,
        ReviewDetail::Exact,
    )
    .await;
    let value = as_json(&response);
    let diff = &value["payload"]["materials"]["diff"];
    assert_four_null(&diff["edits"][0]["removed"]);
    assert_four_null(&diff["edits"][0]["inserted"]);
    assert!(diff["edits"][0]["start"].is_null() && diff["edits"][0]["end"].is_null());
    assert!(diff["changed_bytes"].is_null());
    assert!(!response.diff_complete && response.gaps.contains(&ReviewGap::PrivacyBlocked));
    assert_eq!(catalog(&fixture.db_path), before);
}

#[tokio::test]
async fn scope_pb_span_digest_and_output_corruption_never_yield_a_complete_diff() {
    for corruption in [
        "scope",
        "parent",
        "baseline",
        "utf8",
        "removed_digest",
        "bytes",
        "output",
        "schema",
    ] {
        let fixture = material_fixture(parent()).await;
        let stored = seed_edit(
            &fixture,
            vec![edit(
                &fixture.parent,
                SkillTextField::Content,
                2,
                8,
                TextEditOperation::Replace { text: "新".into() },
            )],
        )
        .await;
        let mut candidate = stored.compiled.clone();
        match corruption {
            "scope" => candidate.episode_id = "another-episode".into(),
            "parent" | "baseline" => {
                let mut prepared = stored.prepared.clone();
                let field = if corruption == "parent" {
                    "parent_digest"
                } else {
                    "baseline_digest"
                };
                prepared.payload["profile"][field] = json!(d("incorrect-context"));
                prepared.input_digest = fingerprint(&prepared.payload).unwrap();
                prepared = prepared.seal().unwrap();
                replace_fact(&fixture, &prepared).await;
            }
            "utf8" => {
                candidate.payload["merge"]["edits"][0]["start"] = json!(3);
                candidate.payload["report"]["edits"][0]["start"] = json!(3);
                let mut batch = stored.template.clone();
                batch.evidence =
                    serde_json::from_value(candidate.payload["merge"]["evidence"].clone()).unwrap();
                batch.edits =
                    serde_json::from_value(candidate.payload["merge"]["edits"].clone()).unwrap();
                candidate.input_digest = fingerprint(&batch).unwrap();
            }
            "removed_digest" => {
                candidate.payload["report"]["edits"][0]["removed_digest"] =
                    json!(d("wrong-fragment"))
            }
            "bytes" => candidate.payload["report"]["edits"][0]["removed_bytes"] = json!(7),
            "output" => candidate.payload["output"]["content"] = json!("independent wrong output"),
            "schema" => {
                candidate.payload["report"]["schema_version"] =
                    json!("rsia.skill_edit.apply_report.v999")
            }
            _ => unreachable!(),
        }
        candidate = candidate.seal().unwrap();
        replace_fact(&fixture, &candidate).await;
        let response = review(
            &fixture,
            ReviewKind::StageFact,
            &candidate.artifact_id,
            ReviewDetail::Exact,
        )
        .await;
        assert!(!response.diff_complete);
        assert!(response.payload.is_none());
        assert!(
            response.gaps.contains(&ReviewGap::MaterialInvalid)
                || response.gaps.contains(&ReviewGap::UnsupportedSchema)
        );
    }
}

#[tokio::test]
async fn roles_namespaces_wrong_primary_and_hidden_kinds_are_unavailable() {
    let fixture = material_fixture(parent()).await;
    let service = HostService::new(fixture.store.clone(), fixture.host.clone()).unwrap();
    for role in [Role::Agent, Role::Worker, Role::Host, Role::Evaluator] {
        let request = ReviewRequest {
            kind: ReviewKind::TypedCandidate,
            id: fixture.candidate.id.clone(),
            detail: ReviewDetail::Exact,
        };
        assert!(matches!(
            service.review(&ctx("unauthorized", role), request).await,
            Err(Error::NotFound)
        ));
    }
    let other = Context::new("other", "admin", Role::Admin).unwrap();
    assert!(matches!(
        service
            .review(
                &other,
                ReviewRequest {
                    kind: ReviewKind::TypedCandidate,
                    id: fixture.candidate.id.clone(),
                    detail: ReviewDetail::Metadata
                }
            )
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        review_stored(
            &fixture.admin,
            &fixture.store,
            ReviewRequest {
                kind: ReviewKind::StageFact,
                id: fixture.candidate.id.clone(),
                detail: ReviewDetail::Exact
            }
        )
        .await,
        Err(Error::NotFound)
    ));
    let hidden = fact(
        StageFactKind::ResponseObserved,
        OptimizationJournalStage::ReflectSuccess,
        d("input"),
        vec![],
        json!({"output":"ANSWER: private fixture"}),
    );
    replace_fact(&fixture, &hidden).await;
    assert!(matches!(
        review_stored(
            &fixture.admin,
            &fixture.store,
            ReviewRequest {
                kind: ReviewKind::StageFact,
                id: hidden.artifact_id,
                detail: ReviewDetail::Exact
            }
        )
        .await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn queued_unselected_source_change_occurs_after_first_full_read_and_before_fresh_read() {
    let fixture = material_fixture(parent()).await;
    let stored = seed_edit(
        &fixture,
        vec![edit(
            &fixture.parent,
            SkillTextField::Content,
            0,
            2,
            TextEditOperation::Replace { text: "new".into() },
        )],
    )
    .await;
    assert!(
        review(
            &fixture,
            ReviewKind::StageFact,
            &stored.compiled.artifact_id,
            ReviewDetail::Exact
        )
        .await
        .diff_complete
    );
    let held = fixture.store.session().await.unwrap();
    let request = ReviewRequest {
        kind: ReviewKind::StageFact,
        id: stored.compiled.artifact_id,
        detail: ReviewDetail::Exact,
    };
    let mut reader = Box::pin(review_stored(&fixture.admin, &fixture.store, request));
    poll_fn(|cx| {
        assert!(reader.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    let mut writer = Box::pin(async {
        let mut session = fixture.store.session().await.unwrap();
        let replacement = source("source-two", "changed unselected support body");
        session
            .put(
                &fixture.host,
                "run",
                "source-two",
                fixture.host.actor(),
                &replacement,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
    });
    poll_fn(|cx| {
        assert!(writer.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    held.commit().await.unwrap();
    let (actual, ()) = tokio::join!(reader, writer);
    let actual = actual.unwrap();
    // read_changed is exclusive to different two-session results. A mutation
    // before BOTH recipes would instead produce the same source_changed gap.
    assert!(actual.gaps.contains(&ReviewGap::ReadChanged));
    assert!(actual.payload.is_none());
    assert!(!actual.diff_complete);
    let mut session = fixture.store.session().await.unwrap();
    let authority: StoredTraceAuthority = session
        .need(&fixture.admin, "run", "source-two")
        .await
        .unwrap();
    assert_eq!(
        authority.trace.source_digest,
        d("changed unselected support body")
    );
    assert_eq!(
        session.watermark(&fixture.admin).await.unwrap(),
        Some((1, d("watermark-one")))
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn support_limit_refuses_the_whole_recipe_without_truncation() {
    let fixture = material_fixture(parent()).await;
    let mut candidate = fixture.candidate.clone();
    candidate.sources = vec![candidate.sources[0].clone(); 129];
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &candidate.id,
            fixture.admin.actor(),
            &candidate,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let response = review(
        &fixture,
        ReviewKind::TypedCandidate,
        &candidate.id,
        ReviewDetail::Exact,
    )
    .await;
    assert!(response.payload.is_none());
    assert!(response.gaps.contains(&ReviewGap::SupportLimit));
    assert!(!response.diff_complete);
    let mut large = parent();
    large.content = "a".repeat(16 * 1024);
    let fixture = material_fixture(large).await;
    let response = review(
        &fixture,
        ReviewKind::TypedCandidate,
        &fixture.candidate.id,
        ReviewDetail::Exact,
    )
    .await;
    let ReviewPayload::TypedCandidate(candidate) = response.payload.unwrap() else {
        panic!("candidate")
    };
    assert_eq!(
        candidate.bundle.skill.content.display.unwrap().len(),
        16 * 1024
    );
    // The compiler already enforces this skill limit. Inject an independently
    // rehashed oversized stored record to exercise review's own finite limit.
    let fixture = material_fixture(parent()).await;
    let mut oversized = fixture.candidate.clone();
    oversized.bundle.skill.content = "a".repeat(16 * 1024 + 1);
    for origin in &mut oversized.bundle.origins {
        if origin.field == "skill.content" {
            origin.value_digest = fingerprint(&oversized.bundle.skill.content).unwrap();
        }
    }
    oversized.bundle.digest.clear();
    oversized.bundle.digest = fingerprint(&oversized.bundle).unwrap();
    oversized.bundle_digest = oversized.bundle.digest.clone();
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            &oversized.id,
            fixture.admin.actor(),
            &oversized,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let before = catalog(&fixture.db_path);
    let response = review(
        &fixture,
        ReviewKind::TypedCandidate,
        &fixture.candidate.id,
        ReviewDetail::Exact,
    )
    .await;
    assert!(response.payload.is_none() && response.gaps.contains(&ReviewGap::SupportLimit));
    assert_eq!(catalog(&fixture.db_path), before);

    // All fields are legal and <= 16KiB. JSON escaping makes the complete
    // parent + baseline + output projection exceed the independent 256KiB cap.
    let mut large_projection = parent();
    large_projection.content = "\"".repeat(16 * 1024);
    large_projection.applicability = "\"".repeat(4096);
    large_projection.counterexample = large_projection.applicability.clone();
    large_projection.required_capabilities = (0..32)
        .map(|i| format!("capability-{i:02}-{}", "x".repeat(100)))
        .collect();
    large_projection.dependencies = (0..32)
        .map(|i| format!("dependency-{i:02}-{}", "x".repeat(100)))
        .collect();
    let mut fixture = material_fixture(large_projection.clone()).await;
    fixture.baseline = large_projection;
    fixture.parent_strategy.instruction = "\"".repeat(8192);
    fixture.baseline_strategy = fixture.parent_strategy.clone();
    let stored = seed_edit(
        &fixture,
        vec![edit(
            &fixture.parent,
            SkillTextField::Content,
            0,
            1,
            TextEditOperation::Replace { text: "a".into() },
        )],
    )
    .await;
    let before = catalog(&fixture.db_path);
    let response = review(
        &fixture,
        ReviewKind::StageFact,
        &stored.compiled.artifact_id,
        ReviewDetail::Exact,
    )
    .await;
    assert!(response.payload.is_none() && response.gaps.contains(&ReviewGap::SupportLimit));
    assert_eq!(catalog(&fixture.db_path), before);

    let fixture = material_fixture(parent()).await;
    let mut predecessor = None;
    for index in 0..32 {
        let deps = predecessor
            .into_iter()
            .map(|id| StageDependency {
                kind: "artifact".into(),
                id,
            })
            .collect();
        let mut hidden = fact(
            StageFactKind::ResponseObserved,
            OptimizationJournalStage::ReflectSuccess,
            d("hidden-input"),
            deps,
            json!({"internal_fixture_response":index}),
        );
        hidden.request_id = format!("hidden-request-{index}");
        hidden = hidden.seal().unwrap();
        replace_fact(&fixture, &hidden).await;
        predecessor = Some(hidden.artifact_id);
    }
    let root = fact(
        StageFactKind::StepCompleted,
        OptimizationJournalStage::Merge,
        d("unbound-step-input"),
        vec![StageDependency {
            kind: "artifact".into(),
            id: predecessor.unwrap(),
        }],
        json!({"status":"no_change","reason":"old untrusted reason"}),
    );
    replace_fact(&fixture, &root).await;
    let before = catalog(&fixture.db_path);
    let response = review(
        &fixture,
        ReviewKind::StageFact,
        &root.artifact_id,
        ReviewDetail::Exact,
    )
    .await;
    assert!(response.payload.is_none() && response.gaps.contains(&ReviewGap::SupportLimit));
    assert_eq!(catalog(&fixture.db_path), before);
}

// Existing E05 setup semantics, isolated here so review verification can compare
// against the stored verifier without changing its earlier acceptance target.
fn money(amount: &str) -> Money {
    Money {
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
        amount: amount.into(),
    }
}

fn optimizer(arm: FixedOptimizerArm, implementation: &str) -> FixedOptimizerSpec {
    FixedOptimizerSpec {
        arm,
        implementation_digest: d(implementation),
        initial_s0_digest: d("s0"),
        base_model_digest: d("model"),
        tools_digest: d("tools"),
        authorized_materials_digest: d("materials"),
        task_partition_digest: d("partition"),
        context_limit_tokens: 4096,
        root_budget_scope_id: "billing-1".into(),
        budget_limit: money("0.001"),
    }
}

struct FormalFixture {
    _dir: tempfile::TempDir,
    db_path: PathBuf,
    store: Store,
    evaluator: Context,
    worker: Context,
    ticket_id: String,
}

async fn formal_fixture(n: u32, sequential: bool) -> FormalFixture {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("e05.sqlite3");
    let store = Store::open(&db_path).await.unwrap();
    let admin = Context::new(NS, "admin", Role::Admin).unwrap();
    let evaluator = Context::new(NS, "evaluator", Role::Evaluator).unwrap();
    let worker = Context::new(NS, "executor", Role::Worker).unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "billing-1".into(),
                allowed_namespaces: vec![NS.into(), "other".into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "fixture-only".into(),
                authorization_receipt_digest: d("admin-budget-receipt"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();

    let mut v1 = ExperimentPlan::first_low_risk("formal-e05", 60).unwrap();
    v1.n_planned = n as usize;
    v1.monetary_budget = money("0.001");
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
    budget.root_total = money("0.001");
    budget.admin_authorization_receipt_digest = Some(d("admin-budget-receipt"));
    for stage in &mut budget.stages {
        stage.per_call_limit.pricing_version = "pricing-v1".into();
        stage.experiment_total.pricing_version = "pricing-v1".into();
    }
    budget.validate().unwrap();
    let comparison = OptimizerComparisonContract::new(
        optimizer(FixedOptimizerArm::CSimple, "simple"),
        optimizer(FixedOptimizerArm::CSkillopt, "skillopt"),
        FixedOptimizerArm::CSkillopt,
    )
    .unwrap();
    assert!(v1.conditions.contains(&ControlCondition::FixedImproverC));
    let formal = FormalExperimentPlanV41::new(
        &v1,
        d("preregistration"),
        &alpha,
        &comparison,
        &budget,
        anchors.digest().unwrap(),
        d("sampling"),
        d("common-mean"),
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
            d("common-mean"),
            d("sampling"),
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
        oracle_digest: d("oracle-version"),
        grader_digest: evo_core::fingerprint(&grader).unwrap(),
        fixed_grader: grader,
        evidence_scope: EvaluationEvidenceScope::ProgramFixture,
        created_at_unix_seconds: 2,
    };
    IndependentEvaluationControl::register_control(&evaluator, &store, control.clone())
        .await
        .unwrap();

    // The candidate pair binds the actual E01 snapshot, not an arbitrary label.
    let pair =
        FrozenCandidatePair::new(v1.digest().unwrap(), d("candidate"), d("baseline"), 3).unwrap();
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
            .map(|id| d(&format!("source-{id}")))
            .collect(),
        "sampler-v1",
        d("oracle-version"),
        anchors.digest().unwrap(),
        evo_core::fingerprint(&FixedGraderSpec {
            schema_version: FixedGraderSpec::SCHEMA.into(),
            version: "exact-json-v1".into(),
            method: FixedGraderMethod::ExactJsonAnswerV1,
        })
        .unwrap(),
        d("environment"),
        d("private-seed"),
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
            input_digest: d(&format!("input-r{index}")),
        })
        .collect();
    let anchor_inputs = vec![ProtectedInputRef {
        target_id: "a0".into(),
        cluster_or_anchor_id: "anchor-1".into(),
        input_artifact_id: "input-a0".into(),
        input_digest: d("input-a0"),
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
        candidate_bundle_digest: d("candidate-bundle"),
        baseline_bundle_digest: d("baseline-bundle"),
        rotating_inputs,
        anchor_inputs,
        oracle_payload_digest: evo_core::fingerprint(&oracle_entries).unwrap(),
        oracle_entries,
    };
    IndependentEvaluationControl::register_holdout(&evaluator, &store, holdout.clone())
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
    FormalFixture {
        _dir: dir,
        db_path,
        store,
        evaluator,
        worker,
        ticket_id: ticket.id,
    }
}

async fn formal_execute_output(
    fixture: &FormalFixture,
    target_id: &str,
    side: ExecutionSide,
    output_utf8: &str,
    ordinal: u32,
) -> String {
    let view = IndependentEvaluationControl::broker_task(
        &fixture.worker,
        &fixture.store,
        &fixture.ticket_id,
        target_id,
        side,
    )
    .await
    .unwrap();
    let broker_json = serde_json::to_string(&view).unwrap();
    assert!(!broker_json.contains("oracle"));
    assert!(!broker_json.contains("grader"));
    assert!(!broker_json.contains("\"ok\""));
    let call_id = format!("call-{target_id}-{side:?}-{ordinal}").to_lowercase();
    let lease = format!("lease-{ordinal}");
    let now = 10 + i64::from(ordinal) * 5;
    fixture
        .store
        .reserve_budget_call(
            &fixture.worker,
            &BudgetCallReservation {
                billing_scope: "billing-1".into(),
                call_id: call_id.clone(),
                dispatch_group_id: fixture.ticket_id.clone(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: view.request_digest.clone(),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: lease.clone(),
                lease_until: now + 1_000,
                now,
            },
        )
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: "billing-1".into(),
        call_id: call_id.clone(),
        actual_input_digest: view.request_digest.clone(),
        lease_token: lease,
        lease_epoch: 1,
        now: now + 1,
    };
    let dispatched = IndependentEvaluationControl::start_execution(
        &fixture.worker,
        &fixture.store,
        StartExecutionRequest {
            ticket_id: fixture.ticket_id.clone(),
            target_id: target_id.into(),
            side,
            fence: fence.clone(),
        },
    )
    .await
    .unwrap();
    let receipt_id = format!("receipt-{target_id}-{side:?}-{ordinal}").to_lowercase();
    let receipt = IndependentEvaluationControl::record_execution_receipt(
        &fixture.worker,
        &fixture.store,
        ExecutionReceiptRequest {
            receipt_id: receipt_id.clone(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: target_id.into(),
            side,
            request_digest: view.request_digest,
            output_artifact_id: format!("output-{target_id}-{side:?}-{ordinal}").to_lowercase(),
            output_utf8: output_utf8.into(),
            budget_call_id: call_id.clone(),
            latency_micros: 100 + u64::from(ordinal),
            issued_at_unix_seconds: now + 2,
        },
    )
    .await
    .unwrap();
    let mut final_fence = fence;
    final_fence.now = now + 3;
    fixture
        .store
        .finalize_budget_call(
            &fixture.worker,
            &final_fence,
            &UsageCharge {
                amount_micros: 10,
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                provider_request_id: format!("provider-{ordinal}"),
                usage_record_id: format!("usage-{ordinal}"),
                output_digest: receipt.output_digest,
            },
        )
        .await
        .unwrap();
    fixture
        .store
        .close_budget_call_execution(
            &fixture.worker,
            "billing-1",
            &call_id,
            dispatched.call.dispatch_id.as_deref().unwrap(),
            "fixture_complete",
            now + 4,
        )
        .await
        .unwrap();
    receipt_id
}

async fn formal_execute_side(
    fixture: &FormalFixture,
    target_id: &str,
    side: ExecutionSide,
    answer: &str,
    ordinal: u32,
) -> String {
    formal_execute_output(
        fixture,
        target_id,
        side,
        &json!({"answer": answer}).to_string(),
        ordinal,
    )
    .await
}

async fn formal_grade_target(
    fixture: &FormalFixture,
    target_id: &str,
    candidate_answer: &str,
    baseline_answer: &str,
    ordinal: u32,
) -> String {
    let candidate = formal_execute_side(
        fixture,
        target_id,
        ExecutionSide::Candidate,
        candidate_answer,
        ordinal * 2,
    )
    .await;
    let baseline = formal_execute_side(
        fixture,
        target_id,
        ExecutionSide::Baseline,
        baseline_answer,
        ordinal * 2 + 1,
    )
    .await;
    let receipt_id = format!("grade-{target_id}-{ordinal}");
    IndependentEvaluationControl::record_grader_receipt(
        &fixture.evaluator,
        &fixture.store,
        GraderReceiptRequest {
            receipt_id: receipt_id.clone(),
            ticket_id: fixture.ticket_id.clone(),
            target_id: target_id.into(),
            candidate_execution_receipt_id: candidate,
            baseline_execution_receipt_id: baseline,
            issued_at_unix_seconds: 500 + i64::from(ordinal),
        },
    )
    .await
    .unwrap();
    receipt_id
}

#[tokio::test]
async fn existing_formal_complete_early_and_invalid_views_keep_every_limitation_and_no_writes() {
    for variant in ["complete_batch", "early_stopped", "invalid"] {
        let fixture = formal_fixture(2, variant == "early_stopped").await;
        match variant {
            "complete_batch" => {
                formal_grade_target(&fixture, "r0", "ok", "wrong", 10).await;
                formal_grade_target(&fixture, "r1", "ok", "wrong", 20).await;
                formal_grade_target(&fixture, "a0", "ok", "ok", 30).await;
                IndependentEvaluationControl::finalize_complete(
                    &fixture.evaluator,
                    &fixture.store,
                    &fixture.ticket_id,
                    1_000,
                )
                .await
                .unwrap();
            }
            "early_stopped" => {
                formal_grade_target(&fixture, "a0", "wrong", "ok", 1).await;
            }
            _ => {
                IndependentEvaluationControl::invalidate(
                    &fixture.evaluator,
                    &fixture.store,
                    &fixture.ticket_id,
                    "missing_execution_rows",
                    20,
                )
                .await
                .unwrap();
            }
        }
        let admin = ctx("admin", Role::Admin);
        let reference = IndependentEvaluationControl::verified_report_view(
            &admin,
            &fixture.store,
            &fixture.ticket_id,
        )
        .await
        .unwrap();
        let mut expected = serde_json::to_value(&reference).unwrap();
        let before = catalog(&fixture.db_path);
        let service = HostService::new(fixture.store.clone(), ctx("host", Role::Host)).unwrap();
        let response = service
            .review(
                &admin,
                ReviewRequest {
                    kind: ReviewKind::FormalReport,
                    id: fixture.ticket_id.clone(),
                    detail: ReviewDetail::Exact,
                },
            )
            .await
            .unwrap();
        let value = as_json(&response);
        assert!(!response.diff_complete);
        assert!(response.trusted_tokens.is_none() && response.total_cost.is_none());
        let actual = &value["payload"]["materials"];
        assert_eq!(actual["variant"], variant);
        assert_eq!(actual["evidence_scope"], "program_fixture");
        assert_eq!(actual["dependency_status"], "namespace_watermark_only");
        assert_eq!(actual["promotion_eligible"], false);
        for field in [
            "report_id",
            "ticket_id",
            "proposer_actor",
            "evaluator_actor",
            "approver_actor",
        ] {
            assert_eq!(actual[field]["display"], expected[field]);
            expected[field] = actual[field].clone();
        }
        if let Some((seq, digest)) = reference.revoke_watermark {
            expected["revoke_watermark"] = json!({"seq":seq,"digest":digest});
        }
        assert_eq!(
            *actual, expected,
            "each existing verified-report limitation is preserved"
        );
        let printed = value.to_string();
        for hidden in [
            "oracle_entries",
            "expected_answer_json",
            "output_utf8",
            "lease_token",
            "private-seed",
            "\"answer\"",
        ] {
            assert!(!printed.contains(hidden), "hidden formal material leaked");
        }
        let metadata = service
            .review(
                &admin,
                ReviewRequest {
                    kind: ReviewKind::FormalReport,
                    id: fixture.ticket_id.clone(),
                    detail: ReviewDetail::Metadata,
                },
            )
            .await
            .unwrap();
        assert_metadata(&as_json(&metadata));
        assert_eq!(
            catalog(&fixture.db_path),
            before,
            "review must not mutate any business table or schema"
        );
    }
}

async fn attach_budget_reference(fixture: &MaterialFixture) -> BudgetCallRef {
    fixture
        .store
        .authorize_root_budget(
            &fixture.admin,
            &RootBudgetAuthorization {
                root_budget_id: "review-root-budget".into(),
                billing_scope: "review-billing".into(),
                allowed_namespaces: vec![NS.into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "fixture-only".into(),
                authorization_receipt_digest: d("review-budget-authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    fixture
        .store
        .reserve_budget_call_with_sources(
            &fixture.admin,
            &BudgetCallReservation {
                billing_scope: "review-billing".into(),
                call_id: "review-source-call".into(),
                dispatch_group_id: "review-source-group".into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: d("review-source-input"),
                request_artifact: None,
                max_cost_micros: 100,
                lease_token: "fixture-only-lease".into(),
                lease_until: 1_000,
                now: 2,
            },
            &[fixture.sources[0].record.id.clone()],
        )
        .await
        .unwrap();
    let digest = fingerprint(&(
        "rsia.budget_call_ref.v1",
        NS,
        "review-billing",
        "review-source-call",
    ))
    .unwrap();
    let id = format!("budget-ref-{}", &digest[..32]);
    let mut session = fixture.store.session().await.unwrap();
    let reference = session.need(&fixture.admin, "artifact", &id).await.unwrap();
    session
        .put_edge(
            &fixture.admin,
            "artifact",
            &fixture.candidate.id,
            "artifact",
            &id,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    reference
}

#[tokio::test]
async fn budget_reference_requires_canonical_identity_real_edges_and_all_declared_live_sources() {
    for case in [
        "noncanonical",
        "missing_revoked_edge",
        "present_revoked_edge",
    ] {
        let fixture = material_fixture(parent()).await;
        let mut reference = attach_budget_reference(&fixture).await;
        let before = catalog(&fixture.db_path);
        let control = review(
            &fixture,
            ReviewKind::TypedCandidate,
            &fixture.candidate.id,
            ReviewDetail::Exact,
        )
        .await;
        assert!(control.payload.is_some());
        assert_eq!(catalog(&fixture.db_path), before);
        let mut session = fixture.store.session().await.unwrap();
        if case == "noncanonical" {
            // The storage identity and body agree, but neither matches the real
            // budget.rs canonical identity for this billing scope and call.
            reference.id = "budget-ref-wrong-canonical-identity".into();
            session
                .put(
                    &fixture.admin,
                    "artifact",
                    &reference.id,
                    fixture.admin.actor(),
                    &reference,
                )
                .await
                .unwrap();
            session
                .put_edge(
                    &fixture.admin,
                    "artifact",
                    &fixture.candidate.id,
                    "artifact",
                    &reference.id,
                )
                .await
                .unwrap();
            session
                .put_edge(
                    &fixture.admin,
                    "artifact",
                    &reference.id,
                    "run",
                    &fixture.sources[0].record.id,
                )
                .await
                .unwrap();
        } else {
            session.commit().await.unwrap();
            let hidden = source(
                "review-revoked-declared-source",
                "revoked source outside candidate sources",
            );
            store_trace_authority(&fixture.store, &fixture.host, &hidden)
                .await
                .unwrap();
            session = fixture.store.session().await.unwrap();
            reference.source_ids.push(hidden.record.id.clone());
            reference.source_ids.sort();
            session
                .put(
                    &fixture.admin,
                    "artifact",
                    &reference.id,
                    fixture.admin.actor(),
                    &reference,
                )
                .await
                .unwrap();
            session
                .put(
                    &fixture.admin,
                    "tombstone",
                    &hidden.record.id,
                    fixture.admin.actor(),
                    &RevokeTombstone {
                        id: hidden.record.id.clone(),
                        schema_version: REVOKE_TOMBSTONE_SCHEMA.into(),
                        source_kind: "run".into(),
                        reason: "fixture-only-revocation".into(),
                        watermark_seq: 1,
                        watermark_digest: d("wm"),
                        created_at: 3,
                    },
                )
                .await
                .unwrap();
            if case == "present_revoked_edge" {
                session
                    .put_edge(
                        &fixture.admin,
                        "artifact",
                        &reference.id,
                        "run",
                        &hidden.record.id,
                    )
                    .await
                    .unwrap();
            }
        }
        session.commit().await.unwrap();
        let before = catalog(&fixture.db_path);
        let response = review(
            &fixture,
            ReviewKind::TypedCandidate,
            &fixture.candidate.id,
            ReviewDetail::Exact,
        )
        .await;
        assert!(
            response.payload.is_none() && !response.diff_complete,
            "{case}"
        );
        let expected_gap = if case == "present_revoked_edge" {
            ReviewGap::SourceChanged
        } else {
            ReviewGap::ReferenceUnavailable
        };
        assert!(
            response.gaps.contains(&expected_gap),
            "{case}: {:?}",
            response.gaps
        );
        assert!(
            !as_json(&response)
                .to_string()
                .contains("review-revoked-declared-source")
        );
        assert_eq!(catalog(&fixture.db_path), before);
    }
}
