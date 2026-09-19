use evo_core::contract::{AppliedReceipt, CapabilityLevel};
use evo_core::evidence::{
    EvidenceMember, EvidenceSet, ExecutionAttestation, Purpose, SourceCoverage, TaskOrigin,
};
use evo_core::hash;
use evo_core::optimization::{
    ApplicationObservation, EditSuggestion, ModelInputPart, ModelInputRole, ModelRequest,
    ModelRequestContext, ModelStage, OptimizationTrace, ReflectionBatchKind, RuleHypothesis,
    RuleHypothesisKind, SelectionEvidence, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
    build_reflection_batches, diagnose_application, merge_suggestions,
};
use evo_core::skill_edit::{EvidenceRef, SkillTextEdit, SkillTextField, TextEditOperation};

fn evidence_ref(id: &str) -> EvidenceRef {
    EvidenceRef {
        id: id.into(),
        digest: hash(id.as_bytes()),
    }
}

fn evidence_set() -> EvidenceSet {
    EvidenceSet::build(
        "evidence-set",
        vec![
            EvidenceMember {
                source_id: "run-failure".into(),
                content_digest: hash(b"run-failure"),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            EvidenceMember {
                source_id: "run-success".into(),
                content_digest: hash(b"run-success"),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            EvidenceMember {
                source_id: "run-env".into(),
                content_digest: hash(b"run-env"),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
        ],
        SourceCoverage {
            discovery_exhausted: true,
            files_known: true,
            ..SourceCoverage::default()
        },
        ["family-a".into(), "family-b".into()].into_iter().collect(),
    )
    .unwrap()
}

fn receipt(attached: bool) -> AppliedReceipt {
    AppliedReceipt {
        offered: vec!["skill-a".into()],
        attached: if attached {
            vec!["skill-a".into()]
        } else {
            vec![]
        },
        used: vec![],
        verified_benefit: vec![],
        bundle_digest: hash(b"bundle-a"),
        request_digest: hash(b"request-a"),
        capability_level: if attached {
            CapabilityLevel::Attached
        } else {
            CapabilityLevel::ToolOnly
        },
        truncated: false,
        attested_by: "trusted-host".into(),
    }
}

fn hypothesis(kind: RuleHypothesisKind) -> RuleHypothesis {
    RuleHypothesis {
        kind,
        rule_id: "rule-a".into(),
        behavior_evidence_digest: hash(b"behavior"),
        support: vec![evidence_ref("run-failure")],
        counterexamples: vec![evidence_ref("run-success")],
    }
}

#[test]
fn v087_application_evidence_routes_host_and_rule_failures_separately() {
    let attached = receipt(true);
    let lapse = hypothesis(RuleHypothesisKind::NotFollowed);
    let diagnosed = diagnose_application(ApplicationObservation {
        skill_id: "skill-a",
        expected_bundle_digest: &hash(b"bundle-a"),
        expected_request_digest: &hash(b"request-a"),
        selection: SelectionEvidence::Selected,
        receipt: Some(&attached),
        request_attestation: Some(ExecutionAttestation::TrustedHost),
        context_matches: true,
        environment_or_capability_failure: false,
        hypothesis: Some(&lapse),
    })
    .unwrap();
    assert_eq!(diagnosed.kind, SkillFailureKind::ExecutionLapse);
    assert_eq!(diagnosed.counterexamples, lapse.counterexamples);

    let missing_attestation = diagnose_application(ApplicationObservation {
        request_attestation: None,
        receipt: Some(&attached),
        hypothesis: Some(&lapse),
        skill_id: "skill-a",
        expected_bundle_digest: &hash(b"bundle-a"),
        expected_request_digest: &hash(b"request-a"),
        selection: SelectionEvidence::Selected,
        context_matches: true,
        environment_or_capability_failure: false,
    })
    .unwrap();
    assert_eq!(missing_attestation.kind, SkillFailureKind::Uncertain);

    let unattached = receipt(false);
    let host_gap = diagnose_application(ApplicationObservation {
        receipt: Some(&unattached),
        hypothesis: Some(&hypothesis(RuleHypothesisKind::Incorrect)),
        skill_id: "skill-a",
        expected_bundle_digest: &hash(b"bundle-a"),
        expected_request_digest: &hash(b"request-a"),
        selection: SelectionEvidence::Selected,
        request_attestation: Some(ExecutionAttestation::TrustedHost),
        context_matches: true,
        environment_or_capability_failure: false,
    })
    .unwrap();
    assert_eq!(host_gap.kind, SkillFailureKind::NotAttached);
}

fn trace(
    run_id: &str,
    family: &str,
    outcome: TraceOutcome,
    diagnosis: Option<SkillFailureKind>,
) -> OptimizationTrace {
    OptimizationTrace {
        run_id: run_id.into(),
        parent_family: family.into(),
        source_digest: hash(run_id.as_bytes()),
        purpose: Purpose::Development,
        outcome,
        diagnosis: diagnosis.map(|kind| evo_core::optimization::SkillFailureDiagnosis {
            kind,
            skill_id: "skill-a".into(),
            bundle_digest: hash(b"bundle-a"),
            request_digest: hash(b"request-a"),
            rule_id: Some("rule-a".into()),
            support: vec![evidence_ref(run_id)],
            counterexamples: vec![],
            reason: "fixture diagnosis".into(),
        }),
        excerpt: format!("evidence from {run_id}"),
        seed: 7,
    }
}

fn bindings() -> Vec<TrustedSourceBinding> {
    vec![
        TrustedSourceBinding {
            source_id: "run-failure".into(),
            source_digest: hash(b"run-failure"),
            parent_family: "family-a".into(),
        },
        TrustedSourceBinding {
            source_id: "run-success".into(),
            source_digest: hash(b"run-success"),
            parent_family: "family-b".into(),
        },
        TrustedSourceBinding {
            source_id: "run-env".into(),
            source_digest: hash(b"run-env"),
            parent_family: "family-a".into(),
        },
    ]
}

#[test]
fn v088_failure_then_success_batches_preserve_sources_and_exclude_environment() {
    let build = build_reflection_batches(
        &evidence_set(),
        &bindings(),
        vec![
            trace("run-success", "family-b", TraceOutcome::Success, None),
            trace(
                "run-failure",
                "family-a",
                TraceOutcome::TaskFailure,
                Some(SkillFailureKind::SkillDefect),
            ),
            trace(
                "run-env",
                "family-a",
                TraceOutcome::EnvironmentFailure,
                Some(SkillFailureKind::EnvironmentOrCapability),
            ),
        ],
    )
    .unwrap();
    assert_eq!(build.batches.len(), 2);
    assert_eq!(build.batches[0].kind, ReflectionBatchKind::Failure);
    assert_eq!(build.batches[1].kind, ReflectionBatchKind::Success);
    assert_eq!(build.excluded_run_ids, vec!["run-env"]);
    assert_eq!(build.batches[0].source_closure[0].id, "run-failure");

    let duplicated = build_reflection_batches(
        &evidence_set(),
        &bindings(),
        vec![
            trace("run-success", "family-b", TraceOutcome::Success, None),
            trace("run-success", "family-b", TraceOutcome::Success, None),
        ],
    );
    assert!(duplicated.is_err());

    let spoofed_family = build_reflection_batches(
        &evidence_set(),
        &bindings(),
        vec![trace(
            "run-failure",
            "family-b",
            TraceOutcome::TaskFailure,
            Some(SkillFailureKind::SkillDefect),
        )],
    );
    assert!(spoofed_family.is_err());
}

#[test]
fn v088_merge_keeps_every_support_counterexample_and_read_dependency() {
    let batches = build_reflection_batches(
        &evidence_set(),
        &bindings(),
        vec![
            trace(
                "run-failure",
                "family-a",
                TraceOutcome::TaskFailure,
                Some(SkillFailureKind::SkillDefect),
            ),
            trace("run-success", "family-b", TraceOutcome::Success, None),
        ],
    )
    .unwrap()
    .batches;
    let failure = evidence_ref("run-failure");
    let success = evidence_ref("run-success");
    let suggestion = EditSuggestion {
        id: "suggestion-a".into(),
        hypothesis: "clarify a rule while preserving the successful behavior".into(),
        batch_ids: vec!["reflection-failure".into(), "reflection-success".into()],
        support: vec![failure.clone()],
        counterexamples: vec![success.clone()],
        dependencies: vec![failure.clone(), success.clone()],
        edit: SkillTextEdit {
            field: SkillTextField::Content,
            start: 0,
            end: 0,
            expected_text_digest: hash(b""),
            exact_anchor: None,
            operation: TextEditOperation::Insert {
                text: "clarification".into(),
            },
        },
    };
    let merged = merge_suggestions(&batches, &[suggestion], &["suggestion-a".into()]).unwrap();
    assert_eq!(merged.evidence.support, vec![failure]);
    assert_eq!(merged.evidence.counterexamples, vec![success]);
    assert_eq!(merged.read_dependencies.len(), 2);
}

fn model_context() -> ModelRequestContext {
    ModelRequestContext {
        request_id: "model-request".into(),
        namespace: "tenant".into(),
        purpose: Purpose::Development,
        stage: ModelStage::ReflectFailure,
        episode_id: "episode-a".into(),
        step: 1,
        attempt: 1,
        parent_skill_digest: hash(b"parent"),
        bundle_digest: hash(b"bundle"),
        source_closure: vec![evidence_ref("run-failure")],
        model_digest: hash(b"model"),
        tools_digest: hash(b"tools"),
        rules_digest: hash(b"rules"),
        sampling_digest: hash(b"sampling"),
        revoke_watermark: 3,
        max_suggestions: 4,
    }
}

#[test]
fn v094_model_request_cache_identity_binds_payload_and_world_inputs() {
    let input = vec![ModelInputPart {
        role: ModelInputRole::Evidence,
        label: "failure-batch".into(),
        content: "full authorized failure payload".into(),
    }];
    let first = ModelRequest::build(model_context(), input.clone()).unwrap();
    let mut changed = model_context();
    changed.revoke_watermark += 1;
    let second = ModelRequest::build(changed, input).unwrap();
    assert_ne!(first.cache_key_digest, second.cache_key_digest);

    let changed_payload = ModelRequest::build(
        model_context(),
        vec![ModelInputPart {
            role: ModelInputRole::Evidence,
            label: "failure-batch".into(),
            content: "different full payload".into(),
        }],
    )
    .unwrap();
    assert_ne!(first.input_digest, changed_payload.input_digest);
    assert_ne!(first.cache_key_digest, changed_payload.cache_key_digest);
}
