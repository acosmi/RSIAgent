//! Actual management consumers, scoped program fixtures and real persistent costs.
//! No external provider, OS isolation or real payment is claimed.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;
use async_trait::async_trait;
use evo_core::contract::{HostCapabilities, ImproverPatch, SkillSnapshot};
use evo_core::evidence::{ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::optimization::{
    EditSuggestion, ModelRequest, ModelStage, OptimizationTrace, SkillFailureDiagnosis,
    SkillFailureKind, TraceOutcome,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, skill_snapshot_digest,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash, now};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::curriculum_profiles::RegisteredPureFunctionProfileV1;
use evo_engine::development::{
    DevelopmentControlV1, DevelopmentEvidenceScope, DevelopmentTaskSpecV1,
    RegisteredDevelopmentRunner, RegisteredTargetInputV1, register_development_control,
    registered_runner_digest,
};
use evo_engine::dispatch::{
    ExperimentRegisterRequestV2, ManagementDispatcher, ManagementJobState, ManagementJobV2,
    OptimizationStepManagementRequest, VersionedManagementJob,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::formal_authority::CompilerInputRequest;
use evo_engine::model::ModelExecutionProvenance;
use evo_engine::optimization::{ManagementOptimizationRuntime, ManagementOptimizationSettings};
use evo_engine::streaming_evaluator::{
    FixedGraderMethod, FixedGraderSpec, IndependentEvaluationControl, ProtectedHoldoutRecordV2,
    RegisteredEvaluationControlV3,
};
use evo_storage::budget::{
    BudgetCallRecord, BudgetCallState, RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use evo_storage::{Store, decode_management_job, sqlx};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

const LEGACY: &str = r###"{"world_terminal_suffix":{"namespace":"n","actor":"admin","role":"admin","job_id":"management-job-3ce1459b090ee3ab44e071cf","input_id":"management-input-3ce1459b090ee3ab44e071cf","request":{"request_key":"ag155-capture-world","schema_version":"rsia.management.exploration_start.v1","world":{"approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","caps":{"max_depth":4,"max_nodes":12,"max_repair_dispatches_per_episode":1,"schema_version":"rsia.exploration_caps.v1","w_online":1},"context_signature":"5370337add2265337fd9b30a572fd87af8ae81dcdf0d86ca69fb99a5d1ac2665","current_branch_focus_actions":0,"current_branch_seq":null,"decision_round":0,"dependencies":[{"id":"run-failure","kind":"run"},{"id":"run-success","kind":"run"}],"dispatch_ids":[],"environment_digest":"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d","grader_digest":"cb8ef6444b43f18fedf1f8347c5cd667f442473878cc1c9547f74564ed6f865d","history_ids":[],"id":"ag155-capture-world","initial_baseline_quality_micros":500000,"model_digest":"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4","node_ids":[],"parent_bundle_digest":"a251f2c86e5b625537281fa11f41cfc88802d13d8be50d8af71bc63c045ea3cd","parent_skill_digest":"a8cf8c36ddeff7527a3e261587c39ab61b9347aa5f11f7e45c1f57a66778a8b3","policy":{"fairness_wait_rounds":4,"max_focus_actions":2,"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2},"remaining_recovery_dispatches":2,"remaining_root_micros":1000,"root_opportunities":[{"action_seq":2,"branch_seq":2,"estimated_cost_upper_micros":10,"root_slot":2},{"action_seq":1,"branch_seq":1,"estimated_cost_upper_micros":10,"root_slot":1}],"rules_digest":"6c621d1a05138a7888d37d9269a9da8e2e11e4aced2f6cfd24b05ab1b9e61bb0","schema_version":"rsia.exploration_world.v1","simulation":{"online":{"fixed_seed":7}},"source_watermark":1,"state":"collecting","successor_cost_upper_micros":10,"tools_digest":"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281","waits":[]}},"objects":[{"namespace":"n","kind":"artifact","id":"management-input-3ce1459b090ee3ab44e071cf","owner":"admin","body":"{\"id\":\"management-input-3ce1459b090ee3ab44e071cf\",\"schema_version\":\"rsia.management_private_input.v1\",\"operation\":\"exploration.start\",\"payload_digest\":\"972e83d33d7b6d5cd497f2bab921f434a8b6a3fef9a4834331680199d2470b52\",\"payload\":{\"operation\":\"exploration_start\",\"request_key\":\"ag155-capture-world\",\"schema_version\":\"rsia.management.exploration_start.v1\",\"world\":{\"approved_parent_digest\":\"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58\",\"caps\":{\"max_depth\":4,\"max_nodes\":12,\"max_repair_dispatches_per_episode\":1,\"schema_version\":\"rsia.exploration_caps.v1\",\"w_online\":1},\"context_signature\":\"5370337add2265337fd9b30a572fd87af8ae81dcdf0d86ca69fb99a5d1ac2665\",\"current_branch_focus_actions\":0,\"current_branch_seq\":null,\"decision_round\":0,\"dependencies\":[{\"id\":\"run-failure\",\"kind\":\"run\"},{\"id\":\"run-success\",\"kind\":\"run\"}],\"dispatch_ids\":[],\"environment_digest\":\"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d\",\"grader_digest\":\"cb8ef6444b43f18fedf1f8347c5cd667f442473878cc1c9547f74564ed6f865d\",\"history_ids\":[],\"id\":\"ag155-capture-world\",\"initial_baseline_quality_micros\":500000,\"model_digest\":\"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4\",\"node_ids\":[],\"parent_bundle_digest\":\"a251f2c86e5b625537281fa11f41cfc88802d13d8be50d8af71bc63c045ea3cd\",\"parent_skill_digest\":\"a8cf8c36ddeff7527a3e261587c39ab61b9347aa5f11f7e45c1f57a66778a8b3\",\"policy\":{\"fairness_wait_rounds\":4,\"max_focus_actions\":2,\"schema_version\":\"rsia.elastic_priority.v1\",\"significant_gain_micros\":20000,\"stagnation_abs_gain_micros\":5000,\"stagnation_window\":2},\"remaining_recovery_dispatches\":2,\"remaining_root_micros\":1000,\"root_opportunities\":[{\"action_seq\":2,\"branch_seq\":2,\"estimated_cost_upper_micros\":10,\"root_slot\":2},{\"action_seq\":1,\"branch_seq\":1,\"estimated_cost_upper_micros\":10,\"root_slot\":1}],\"rules_digest\":\"6c621d1a05138a7888d37d9269a9da8e2e11e4aced2f6cfd24b05ab1b9e61bb0\",\"schema_version\":\"rsia.exploration_world.v1\",\"simulation\":{\"online\":{\"fixed_seed\":7}},\"source_watermark\":1,\"state\":\"collecting\",\"successor_cost_upper_micros\":10,\"tools_digest\":\"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281\",\"waits\":[]}}}","revision":1},{"namespace":"n","kind":"job","id":"management-job-3ce1459b090ee3ab44e071cf","owner":"admin","body":"{\"id\":\"management-job-3ce1459b090ee3ab44e071cf\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"exploration.start\",\"request_key\":\"ag155-capture-world\",\"payload_digest\":\"972e83d33d7b6d5cd497f2bab921f434a8b6a3fef9a4834331680199d2470b52\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"succeeded\",\"step\":\"exploration_started:cancel_after_side_effect\",\"private_input_ref\":\"management-input-3ce1459b090ee3ab44e071cf\",\"result\":{\"result\":\"exploration_started\",\"world_id\":\"ag155-capture-world\",\"context_signature\":\"5370337add2265337fd9b30a572fd87af8ae81dcdf0d86ca69fb99a5d1ac2665\",\"prefix_digest\":\"4a489b56ff0ebcd3df831e27cc78c048c449396746ba7d424efef4b357fc0dcc\",\"legal_actions_digest\":\"0f8eebdaca13c2e768864211215051e4ef4ce329f238828b293af86e5d82763c\",\"policy_digest\":\"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423\",\"caps_digest\":\"81f3bf209b41d41276ab87a00fa7fd30e9fdf3792ed08b27174c0f27a0cdf67f\",\"action\":{\"decision\":\"dispatch\",\"action_ids\":[\"root-1\"],\"action_seqs\":[1],\"reason\":\"first_preregistered_root\",\"estimated_cost_upper_micros\":10}},\"error_code\":null,\"cancel_requested\":true,\"lease_token\":null,\"lease_until\":0,\"generation\":1,\"diagnostics\":[],\"created_at\":1793306575}","revision":5}],"cache":[{"namespace":"n","actor":"admin","operation":"exploration.start","request_key":"ag155-capture-world","payload_hash":"972e83d33d7b6d5cd497f2bab921f434a8b6a3fef9a4834331680199d2470b52","subject_id":"management-job-3ce1459b090ee3ab44e071cf","response":"{\"id\":\"management-job-3ce1459b090ee3ab44e071cf\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"exploration.start\",\"request_key\":\"ag155-capture-world\",\"payload_digest\":\"972e83d33d7b6d5cd497f2bab921f434a8b6a3fef9a4834331680199d2470b52\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"queued\",\"step\":\"accepted\",\"private_input_ref\":\"management-input-3ce1459b090ee3ab44e071cf\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":null,\"lease_until\":0,\"generation\":0,\"diagnostics\":[],\"created_at\":1793306575}","redacted":0}]},"queued":{"namespace":"n","actor":"admin","role":"admin","job_id":"management-job-00afd430bef6dd4b64430af5","input_id":"management-input-00afd430bef6dd4b64430af5","request":{"request_key":"ag155-capture-meta","schema_version":"rsia.management.meta_start.v1"},"objects":[{"namespace":"n","kind":"artifact","id":"management-input-00afd430bef6dd4b64430af5","owner":"admin","body":"{\"id\":\"management-input-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_private_input.v1\",\"operation\":\"meta.start\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"payload\":{\"operation\":\"blocked\",\"request_key\":\"ag155-capture-meta\",\"schema_version\":\"rsia.management.meta_start.v1\"}}","revision":1},{"namespace":"n","kind":"job","id":"management-job-00afd430bef6dd4b64430af5","owner":"admin","body":"{\"id\":\"management-job-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"meta.start\",\"request_key\":\"ag155-capture-meta\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"queued\",\"step\":\"accepted\",\"private_input_ref\":\"management-input-00afd430bef6dd4b64430af5\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":null,\"lease_until\":0,\"generation\":0,\"diagnostics\":[],\"created_at\":1793306574}","revision":1}],"cache":[{"namespace":"n","actor":"admin","operation":"meta.start","request_key":"ag155-capture-meta","payload_hash":"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242","subject_id":"management-job-00afd430bef6dd4b64430af5","response":"{\"id\":\"management-job-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"meta.start\",\"request_key\":\"ag155-capture-meta\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"queued\",\"step\":\"accepted\",\"private_input_ref\":\"management-input-00afd430bef6dd4b64430af5\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":null,\"lease_until\":0,\"generation\":0,\"diagnostics\":[],\"created_at\":1793306574}","redacted":0}]},"running":{"namespace":"n","actor":"admin","role":"admin","job_id":"management-job-00afd430bef6dd4b64430af5","input_id":"management-input-00afd430bef6dd4b64430af5","request":{"request_key":"ag155-capture-meta","schema_version":"rsia.management.meta_start.v1"},"objects":[{"namespace":"n","kind":"artifact","id":"management-input-00afd430bef6dd4b64430af5","owner":"admin","body":"{\"id\":\"management-input-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_private_input.v1\",\"operation\":\"meta.start\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"payload\":{\"operation\":\"blocked\",\"request_key\":\"ag155-capture-meta\",\"schema_version\":\"rsia.management.meta_start.v1\"}}","revision":1},{"namespace":"n","kind":"job","id":"management-job-00afd430bef6dd4b64430af5","owner":"admin","body":"{\"id\":\"management-job-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"meta.start\",\"request_key\":\"ag155-capture-meta\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"running\",\"step\":\"claimed\",\"private_input_ref\":\"management-input-00afd430bef6dd4b64430af5\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":\"fbd1eb475a4795774e9fbdd927aa81c47dab87b4ffda41fc3d898af871d175af\",\"lease_until\":1793306634,\"generation\":1,\"diagnostics\":[],\"created_at\":1793306574}","revision":2}],"cache":[{"namespace":"n","actor":"admin","operation":"meta.start","request_key":"ag155-capture-meta","payload_hash":"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242","subject_id":"management-job-00afd430bef6dd4b64430af5","response":"{\"id\":\"management-job-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"meta.start\",\"request_key\":\"ag155-capture-meta\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"queued\",\"step\":\"accepted\",\"private_input_ref\":\"management-input-00afd430bef6dd4b64430af5\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":null,\"lease_until\":0,\"generation\":0,\"diagnostics\":[],\"created_at\":1793306574}","redacted":0}]},"control_committed_holdout_not_committed":{"namespace":"n","actor":"evaluator","role":"evaluator","job_id":"management-job-ae10c9ec67325b06cea1ff97","input_id":"management-input-ae10c9ec67325b06cea1ff97","request":{"control":{"alpha_plan":{"allocations":[{"alpha":"0.04","attempt_id":"attempt-1","claim_id":"fixed-gain","kind":"fixed_sample_gain"},{"alpha":"0.01","attempt_id":"attempt-1","claim_id":"early-reject","kind":"sequential_reject"}],"alpha_total":"0.05","research_family_id":"family-e05","schema_version":"rsia.research_family_alpha_plan.v1"},"anchor_matrix":{"critical_capabilities":[{"anchor_ids":["anchor-1"],"capability_id":"safety"}],"profile_id":"quality-gain","schema_version":"rsia.anchor_coverage_matrix.v1","version":"anchors-v1"},"approver_actor":"approver","attempt_id":"attempt-1","billing_scope":"billing-1","created_at_unix_seconds":2,"early_stop_plan":null,"evaluator_actor":"evaluator","evidence_scope":"program_fixture","executor_actor":"executor","fixed_grader":{"method":"exact_json_answer_v1","schema_version":"rsia.fixed_grader.v1","version":"exact-json-v1"},"fixed_sample_claim_id":"fixed-gain","formal_plan":{"alpha_plan_digest":"b4c896cd5e152cdcd465cb0c3739601a95b6b99a6dd10534bbab13196e0c9e0d","anchor_coverage_digest":"98744fd878573309c1dee0b58fe926e814adb024a48281c303de833650ef021b","execution_authority":"not_issued","n_planned":2,"optimization_budget_digest":"95f5e72267264a4f42a4071c13303db9660b3d813a0e087f7504b295d263cdb6","optimizer_comparison_digest":"f5a65644332761fde1c427b9ed687748f2bc2c58c866b80d8d621eeaacec4e57","prefix_selection":"frozen_continuous_order","preregistration_receipt_digest":"3e8ac5236ba3cb5c5018eb5dda6f156d02eef753bf2420bc73593d861365d05e","profile":"quality_gain","research_family_id":"family-e05","schema_version":"rsia.formal_experiment_plan.v4_1","source_sampling_digest":"52630e4131f9862ea34c8ded9741fae2176c56c55c74827b2bd30cd1ef9eb37b","statistical_assumptions_digest":"50b8c3d1ac87e1da574e773ec4a396a4c74213407d7845ab17672f4cf2ad30b2","statistical_unit":"independent_task_cluster","v1_plan_snapshot_digest":"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1","weighting":"equal_units"},"grader_digest":"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a","id":"control-1","optimization_budget":{"admin_authorization_receipt_digest":"ec70f47b06c5879902c6546b934c89514f6b315fb04d20d94e9ebe9fcb6c7ce9","payer_id":"payer","root_budget_scope_id":"billing-1","root_total":{"amount":"0.001","currency":"USD","pricing_version":"pricing-v1"},"schema_version":"rsia.optimization_budget_plan.v1","stages":[{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"history_collection"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"reflection"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"merge"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"ranking"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"development_execution"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"development_scoring"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"practice"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"consolidation"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"guidance"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"human_review"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"storage_cpu"}]},"optimizer_comparison":{"c_simple":{"arm":"c_simple","authorized_materials_digest":"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88","base_model_digest":"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4","budget_limit":{"amount":"0.001","currency":"USD","pricing_version":"pricing-v1"},"context_limit_tokens":4096,"implementation_digest":"a7a39b72f29718e653e73503210fbb597057b7a1c77d1fe321a1afcff041d4e1","initial_s0_digest":"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513","root_budget_scope_id":"billing-1","task_partition_digest":"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254","tools_digest":"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281"},"c_skillopt":{"arm":"c_skillopt","authorized_materials_digest":"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88","base_model_digest":"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4","budget_limit":{"amount":"0.001","currency":"USD","pricing_version":"pricing-v1"},"context_limit_tokens":4096,"implementation_digest":"d97dddf1b4635724f6d5eea620a2679bd7667da5dd6a3b3b9a2245852ea1c33a","initial_s0_digest":"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513","root_budget_scope_id":"billing-1","task_partition_digest":"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254","tools_digest":"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281"},"d_starts_from":"c_skillopt","schema_version":"rsia.optimizer_comparison_contract.v1"},"oracle_digest":"9297ea092eae8ad974be8d2c9d294283d0e5aeed5a3e34886dfb954dcf1ac6e3","proposer_actor":"proposer","schema_version":"rsia.registered_evaluation_control.v2","v1_plan_snapshot":{"alpha_total":0.05,"candidate_digest":null,"conditions":["frozen_a","static_harness_b1","fixed_improver_c"],"dataset_epoch":"dev_pilot_unfunded","first_round":true,"frozen":true,"frozen_at":1,"id":"formal-e05","max_cost_ratio":1.1,"max_p95_latency_ratio":1.2,"min_effect":0.02,"monetary_budget":{"amount":"0.001","currency":"USD","pricing_version":"pricing-v1"},"n_planned":2,"noninferior_bound":-0.01,"objective_version":"rsia.attainment_auc.v1","profile":"quality_gain","query_limit":3,"savings_ratio":0.1,"schema_version":"rsia.experiment_plan.v1","stats_version":"rsia.empirical_bernstein.v2","stop_on_harm":true}},"holdout":{"anchor_inputs":[{"cluster_or_anchor_id":"anchor-1","input_artifact_id":"input-a0","input_digest":"b9805f3bf0893bf6519130720c2e7d21f85d4eb30d611a00512953d9ec93e459","target_id":"a0"}],"baseline_bundle_digest":"42d582727e80985cc3d2140697d6cd801fb0e79deea844f4fb3a48ea4469b2ae","candidate_bundle_digest":"f0f5e9a60c5fbf07649b2e866d97133a74f613e613759fdcaf645cabd0a8d3e1","id":"holdout-1","manifest":{"alpha_plan_digest":"b4c896cd5e152cdcd465cb0c3739601a95b6b99a6dd10534bbab13196e0c9e0d","anchor_coverage_digest":"98744fd878573309c1dee0b58fe926e814adb024a48281c303de833650ef021b","candidate_pair_digest":"ce7d106a252e85ee6fd0da0bba6613ba7b500690fc75e36edd80585ad02d751f","environment_digest":"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d","epoch_id":"epoch-1","grader_digest":"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a","issued_at_unix_ms":4,"oracle_version":"9297ea092eae8ad974be8d2c9d294283d0e5aeed5a3e34886dfb954dcf1ac6e3","ordered_task_cluster_ids":["cluster-0","cluster-1"],"private_seed_commitment":"075612c8ddff488680ca76418cd909f39954a7ce13390c9f1e5bae1c1d1307d4","research_family_id":"family-e05","sampler_version":"sampler-v1","schema_version":"rsia.dual_timescale_holdout.v1","sequence_commitment":"92c6d50a920a2eadade4aedcb47d8761d2436411147fbecabdcbcffcf1939e32","sequential_mode":{"mode":"disabled"},"slice_id":"slice-1","source_closure_digests":["cf3d9e40a268fd493c714ed760a8a7fbc85aeba6f0cbf13c6f539d5b5bc03fc8","e371df7ae3fa5d8c47400dd643ff7f10232089df78f7bf21c3bc21a8e2e05c26"],"v1_plan_snapshot_digest":"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1"},"oracle_entries":[{"expected_answer_json":"\"ok\"","target_id":"r0"},{"expected_answer_json":"\"ok\"","target_id":"r1"},{"expected_answer_json":"\"ok\"","target_id":"a0"}],"oracle_payload_digest":"fd25ac3b5815657ea841251905e5e1aa6132b547cb2c6d148e7e2ea73560fe4a","pair":{"baseline_digest":"09fc333c79554abaf2b12d0aa64c34c1aa4f9dd2fe552f2bc18d71e8a86a77d7","candidate_digest":"dda18a0e21ae47c53b4309434cbc02ae8bf764fa83a6defbb719431242722aa7","frozen_at_unix_ms":3,"schema_version":"rsia.frozen_candidate_pair.v1","v1_plan_snapshot_digest":"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1"},"registration_id":"control-1","rotating_inputs":[{"cluster_or_anchor_id":"cluster-0","input_artifact_id":"input-r0","input_digest":"50ebdd6498f1d21eda818311e14b64d456ba12479633c68bafd0308a41dfff8d","target_id":"r0"},{"cluster_or_anchor_id":"cluster-1","input_artifact_id":"input-r1","input_digest":"79f20cb26ce6973c742cf84981d1eb180a330de4f174bc83de8b8397abc0c779","target_id":"r1"}],"schema_version":"rsia.protected_holdout_record.v1"},"request_key":"ag155-capture-registration","schema_version":"rsia.management.experiment_register.v1"},"objects":[{"namespace":"n","kind":"artifact","id":"management-input-ae10c9ec67325b06cea1ff97","owner":"evaluator","body":"{\"id\":\"management-input-ae10c9ec67325b06cea1ff97\",\"schema_version\":\"rsia.management_private_input.v1\",\"operation\":\"experiment.register\",\"payload_digest\":\"d22f58f331a893f330c9280c5a8570ebaea912a807678a2bbadbf5c1124aebc6\",\"payload\":{\"control\":{\"alpha_plan\":{\"allocations\":[{\"alpha\":\"0.04\",\"attempt_id\":\"attempt-1\",\"claim_id\":\"fixed-gain\",\"kind\":\"fixed_sample_gain\"},{\"alpha\":\"0.01\",\"attempt_id\":\"attempt-1\",\"claim_id\":\"early-reject\",\"kind\":\"sequential_reject\"}],\"alpha_total\":\"0.05\",\"research_family_id\":\"family-e05\",\"schema_version\":\"rsia.research_family_alpha_plan.v1\"},\"anchor_matrix\":{\"critical_capabilities\":[{\"anchor_ids\":[\"anchor-1\"],\"capability_id\":\"safety\"}],\"profile_id\":\"quality-gain\",\"schema_version\":\"rsia.anchor_coverage_matrix.v1\",\"version\":\"anchors-v1\"},\"approver_actor\":\"approver\",\"attempt_id\":\"attempt-1\",\"billing_scope\":\"billing-1\",\"created_at_unix_seconds\":2,\"early_stop_plan\":null,\"evaluator_actor\":\"evaluator\",\"evidence_scope\":\"program_fixture\",\"executor_actor\":\"executor\",\"fixed_grader\":{\"method\":\"exact_json_answer_v1\",\"schema_version\":\"rsia.fixed_grader.v1\",\"version\":\"exact-json-v1\"},\"fixed_sample_claim_id\":\"fixed-gain\",\"formal_plan\":{\"alpha_plan_digest\":\"b4c896cd5e152cdcd465cb0c3739601a95b6b99a6dd10534bbab13196e0c9e0d\",\"anchor_coverage_digest\":\"98744fd878573309c1dee0b58fe926e814adb024a48281c303de833650ef021b\",\"execution_authority\":\"not_issued\",\"n_planned\":2,\"optimization_budget_digest\":\"95f5e72267264a4f42a4071c13303db9660b3d813a0e087f7504b295d263cdb6\",\"optimizer_comparison_digest\":\"f5a65644332761fde1c427b9ed687748f2bc2c58c866b80d8d621eeaacec4e57\",\"prefix_selection\":\"frozen_continuous_order\",\"preregistration_receipt_digest\":\"3e8ac5236ba3cb5c5018eb5dda6f156d02eef753bf2420bc73593d861365d05e\",\"profile\":\"quality_gain\",\"research_family_id\":\"family-e05\",\"schema_version\":\"rsia.formal_experiment_plan.v4_1\",\"source_sampling_digest\":\"52630e4131f9862ea34c8ded9741fae2176c56c55c74827b2bd30cd1ef9eb37b\",\"statistical_assumptions_digest\":\"50b8c3d1ac87e1da574e773ec4a396a4c74213407d7845ab17672f4cf2ad30b2\",\"statistical_unit\":\"independent_task_cluster\",\"v1_plan_snapshot_digest\":\"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1\",\"weighting\":\"equal_units\"},\"grader_digest\":\"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a\",\"id\":\"control-1\",\"optimization_budget\":{\"admin_authorization_receipt_digest\":\"ec70f47b06c5879902c6546b934c89514f6b315fb04d20d94e9ebe9fcb6c7ce9\",\"payer_id\":\"payer\",\"root_budget_scope_id\":\"billing-1\",\"root_total\":{\"amount\":\"0.001\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"schema_version\":\"rsia.optimization_budget_plan.v1\",\"stages\":[{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"history_collection\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"reflection\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"merge\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"ranking\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"development_execution\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"development_scoring\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"practice\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"consolidation\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"guidance\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"human_review\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"storage_cpu\"}]},\"optimizer_comparison\":{\"c_simple\":{\"arm\":\"c_simple\",\"authorized_materials_digest\":\"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88\",\"base_model_digest\":\"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4\",\"budget_limit\":{\"amount\":\"0.001\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"context_limit_tokens\":4096,\"implementation_digest\":\"a7a39b72f29718e653e73503210fbb597057b7a1c77d1fe321a1afcff041d4e1\",\"initial_s0_digest\":\"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513\",\"root_budget_scope_id\":\"billing-1\",\"task_partition_digest\":\"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254\",\"tools_digest\":\"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281\"},\"c_skillopt\":{\"arm\":\"c_skillopt\",\"authorized_materials_digest\":\"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88\",\"base_model_digest\":\"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4\",\"budget_limit\":{\"amount\":\"0.001\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"context_limit_tokens\":4096,\"implementation_digest\":\"d97dddf1b4635724f6d5eea620a2679bd7667da5dd6a3b3b9a2245852ea1c33a\",\"initial_s0_digest\":\"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513\",\"root_budget_scope_id\":\"billing-1\",\"task_partition_digest\":\"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254\",\"tools_digest\":\"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281\"},\"d_starts_from\":\"c_skillopt\",\"schema_version\":\"rsia.optimizer_comparison_contract.v1\"},\"oracle_digest\":\"9297ea092eae8ad974be8d2c9d294283d0e5aeed5a3e34886dfb954dcf1ac6e3\",\"proposer_actor\":\"proposer\",\"schema_version\":\"rsia.registered_evaluation_control.v2\",\"v1_plan_snapshot\":{\"alpha_total\":0.05,\"candidate_digest\":null,\"conditions\":[\"frozen_a\",\"static_harness_b1\",\"fixed_improver_c\"],\"dataset_epoch\":\"dev_pilot_unfunded\",\"first_round\":true,\"frozen\":true,\"frozen_at\":1,\"id\":\"formal-e05\",\"max_cost_ratio\":1.1,\"max_p95_latency_ratio\":1.2,\"min_effect\":0.02,\"monetary_budget\":{\"amount\":\"0.001\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"n_planned\":2,\"noninferior_bound\":-0.01,\"objective_version\":\"rsia.attainment_auc.v1\",\"profile\":\"quality_gain\",\"query_limit\":3,\"savings_ratio\":0.1,\"schema_version\":\"rsia.experiment_plan.v1\",\"stats_version\":\"rsia.empirical_bernstein.v2\",\"stop_on_harm\":true}},\"holdout\":{\"anchor_inputs\":[{\"cluster_or_anchor_id\":\"anchor-1\",\"input_artifact_id\":\"input-a0\",\"input_digest\":\"b9805f3bf0893bf6519130720c2e7d21f85d4eb30d611a00512953d9ec93e459\",\"target_id\":\"a0\"}],\"baseline_bundle_digest\":\"42d582727e80985cc3d2140697d6cd801fb0e79deea844f4fb3a48ea4469b2ae\",\"candidate_bundle_digest\":\"f0f5e9a60c5fbf07649b2e866d97133a74f613e613759fdcaf645cabd0a8d3e1\",\"id\":\"holdout-1\",\"manifest\":{\"alpha_plan_digest\":\"b4c896cd5e152cdcd465cb0c3739601a95b6b99a6dd10534bbab13196e0c9e0d\",\"anchor_coverage_digest\":\"98744fd878573309c1dee0b58fe926e814adb024a48281c303de833650ef021b\",\"candidate_pair_digest\":\"ce7d106a252e85ee6fd0da0bba6613ba7b500690fc75e36edd80585ad02d751f\",\"environment_digest\":\"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d\",\"epoch_id\":\"epoch-1\",\"grader_digest\":\"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a\",\"issued_at_unix_ms\":4,\"oracle_version\":\"9297ea092eae8ad974be8d2c9d294283d0e5aeed5a3e34886dfb954dcf1ac6e3\",\"ordered_task_cluster_ids\":[\"cluster-0\",\"cluster-1\"],\"private_seed_commitment\":\"075612c8ddff488680ca76418cd909f39954a7ce13390c9f1e5bae1c1d1307d4\",\"research_family_id\":\"family-e05\",\"sampler_version\":\"sampler-v1\",\"schema_version\":\"rsia.dual_timescale_holdout.v1\",\"sequence_commitment\":\"92c6d50a920a2eadade4aedcb47d8761d2436411147fbecabdcbcffcf1939e32\",\"sequential_mode\":{\"mode\":\"disabled\"},\"slice_id\":\"slice-1\",\"source_closure_digests\":[\"cf3d9e40a268fd493c714ed760a8a7fbc85aeba6f0cbf13c6f539d5b5bc03fc8\",\"e371df7ae3fa5d8c47400dd643ff7f10232089df78f7bf21c3bc21a8e2e05c26\"],\"v1_plan_snapshot_digest\":\"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1\"},\"oracle_entries\":[{\"expected_answer_json\":\"\\\"ok\\\"\",\"target_id\":\"r0\"},{\"expected_answer_json\":\"\\\"ok\\\"\",\"target_id\":\"r1\"},{\"expected_answer_json\":\"\\\"ok\\\"\",\"target_id\":\"a0\"}],\"oracle_payload_digest\":\"fd25ac3b5815657ea841251905e5e1aa6132b547cb2c6d148e7e2ea73560fe4a\",\"pair\":{\"baseline_digest\":\"09fc333c79554abaf2b12d0aa64c34c1aa4f9dd2fe552f2bc18d71e8a86a77d7\",\"candidate_digest\":\"dda18a0e21ae47c53b4309434cbc02ae8bf764fa83a6defbb719431242722aa7\",\"frozen_at_unix_ms\":3,\"schema_version\":\"rsia.frozen_candidate_pair.v1\",\"v1_plan_snapshot_digest\":\"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1\"},\"registration_id\":\"control-1\",\"rotating_inputs\":[{\"cluster_or_anchor_id\":\"cluster-0\",\"input_artifact_id\":\"input-r0\",\"input_digest\":\"50ebdd6498f1d21eda818311e14b64d456ba12479633c68bafd0308a41dfff8d\",\"target_id\":\"r0\"},{\"cluster_or_anchor_id\":\"cluster-1\",\"input_artifact_id\":\"input-r1\",\"input_digest\":\"79f20cb26ce6973c742cf84981d1eb180a330de4f174bc83de8b8397abc0c779\",\"target_id\":\"r1\"}],\"schema_version\":\"rsia.protected_holdout_record.v1\"},\"operation\":\"experiment_register\",\"request_key\":\"ag155-capture-registration\",\"schema_version\":\"rsia.management.experiment_register.v1\"}}","revision":1},{"namespace":"n","kind":"job","id":"management-job-ae10c9ec67325b06cea1ff97","owner":"evaluator","body":"{\"id\":\"management-job-ae10c9ec67325b06cea1ff97\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"experiment.register\",\"request_key\":\"ag155-capture-registration\",\"payload_digest\":\"d22f58f331a893f330c9280c5a8570ebaea912a807678a2bbadbf5c1124aebc6\",\"owner_actor\":\"evaluator\",\"owner_role\":\"evaluator\",\"state\":\"running\",\"step\":\"control_registered\",\"private_input_ref\":\"management-input-ae10c9ec67325b06cea1ff97\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":\"14dcb05f1c1b8ff1e55c12b53fd5c9ae80c0b8347a39930412db3ac0a1297bed\",\"lease_until\":1793306635,\"generation\":1,\"diagnostics\":[],\"created_at\":1793306575}","revision":3},{"namespace":"n","kind":"artifact","id":"e05-c08afb9d9cc84a6be5fb00e2f78986e0cef55ce1b808ec7c1c15464d26fa4724","owner":"evaluator","body":"{\"schema_version\":\"rsia.typed_artifact_envelope.v1\",\"id\":\"e05-c08afb9d9cc84a6be5fb00e2f78986e0cef55ce1b808ec7c1c15464d26fa4724\",\"record_kind\":\"registered_evaluation_control_v41\",\"payload\":{\"schema_version\":\"rsia.registered_evaluation_control.v2\",\"id\":\"control-1\",\"v1_plan_snapshot\":{\"schema_version\":\"rsia.experiment_plan.v1\",\"id\":\"formal-e05\",\"objective_version\":\"rsia.attainment_auc.v1\",\"stats_version\":\"rsia.empirical_bernstein.v2\",\"profile\":\"quality_gain\",\"conditions\":[\"frozen_a\",\"static_harness_b1\",\"fixed_improver_c\"],\"first_round\":true,\"min_effect\":0.02,\"noninferior_bound\":-0.01,\"savings_ratio\":0.1,\"max_cost_ratio\":1.1,\"max_p95_latency_ratio\":1.2,\"alpha_total\":0.05,\"n_planned\":2,\"query_limit\":3,\"monetary_budget\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0.001\"},\"stop_on_harm\":true,\"frozen\":true,\"frozen_at\":1,\"candidate_digest\":null,\"dataset_epoch\":\"dev_pilot_unfunded\"},\"formal_plan\":{\"schema_version\":\"rsia.formal_experiment_plan.v4_1\",\"v1_plan_snapshot_digest\":\"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1\",\"preregistration_receipt_digest\":\"3e8ac5236ba3cb5c5018eb5dda6f156d02eef753bf2420bc73593d861365d05e\",\"research_family_id\":\"family-e05\",\"alpha_plan_digest\":\"b4c896cd5e152cdcd465cb0c3739601a95b6b99a6dd10534bbab13196e0c9e0d\",\"optimizer_comparison_digest\":\"f5a65644332761fde1c427b9ed687748f2bc2c58c866b80d8d621eeaacec4e57\",\"optimization_budget_digest\":\"95f5e72267264a4f42a4071c13303db9660b3d813a0e087f7504b295d263cdb6\",\"anchor_coverage_digest\":\"98744fd878573309c1dee0b58fe926e814adb024a48281c303de833650ef021b\",\"source_sampling_digest\":\"52630e4131f9862ea34c8ded9741fae2176c56c55c74827b2bd30cd1ef9eb37b\",\"statistical_assumptions_digest\":\"50b8c3d1ac87e1da574e773ec4a396a4c74213407d7845ab17672f4cf2ad30b2\",\"profile\":\"quality_gain\",\"statistical_unit\":\"independent_task_cluster\",\"weighting\":\"equal_units\",\"prefix_selection\":\"frozen_continuous_order\",\"execution_authority\":\"not_issued\",\"n_planned\":2},\"alpha_plan\":{\"schema_version\":\"rsia.research_family_alpha_plan.v1\",\"research_family_id\":\"family-e05\",\"alpha_total\":\"0.05\",\"allocations\":[{\"claim_id\":\"fixed-gain\",\"attempt_id\":\"attempt-1\",\"kind\":\"fixed_sample_gain\",\"alpha\":\"0.04\"},{\"claim_id\":\"early-reject\",\"attempt_id\":\"attempt-1\",\"kind\":\"sequential_reject\",\"alpha\":\"0.01\"}]},\"optimizer_comparison\":{\"schema_version\":\"rsia.optimizer_comparison_contract.v1\",\"c_simple\":{\"arm\":\"c_simple\",\"implementation_digest\":\"a7a39b72f29718e653e73503210fbb597057b7a1c77d1fe321a1afcff041d4e1\",\"initial_s0_digest\":\"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513\",\"base_model_digest\":\"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4\",\"tools_digest\":\"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281\",\"authorized_materials_digest\":\"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88\",\"task_partition_digest\":\"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254\",\"context_limit_tokens\":4096,\"root_budget_scope_id\":\"billing-1\",\"budget_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0.001\"}},\"c_skillopt\":{\"arm\":\"c_skillopt\",\"implementation_digest\":\"d97dddf1b4635724f6d5eea620a2679bd7667da5dd6a3b3b9a2245852ea1c33a\",\"initial_s0_digest\":\"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513\",\"base_model_digest\":\"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4\",\"tools_digest\":\"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281\",\"authorized_materials_digest\":\"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88\",\"task_partition_digest\":\"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254\",\"context_limit_tokens\":4096,\"root_budget_scope_id\":\"billing-1\",\"budget_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0.001\"}},\"d_starts_from\":\"c_skillopt\"},\"optimization_budget\":{\"schema_version\":\"rsia.optimization_budget_plan.v1\",\"root_budget_scope_id\":\"billing-1\",\"payer_id\":\"payer\",\"root_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0.001\"},\"admin_authorization_receipt_digest\":\"ec70f47b06c5879902c6546b934c89514f6b315fb04d20d94e9ebe9fcb6c7ce9\",\"stages\":[{\"stage\":\"history_collection\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"reflection\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"merge\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"ranking\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"development_execution\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"development_scoring\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"practice\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"consolidation\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"guidance\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"human_review\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"storage_cpu\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}}]},\"early_stop_plan\":null,\"anchor_matrix\":{\"schema_version\":\"rsia.anchor_coverage_matrix.v1\",\"profile_id\":\"quality-gain\",\"version\":\"anchors-v1\",\"critical_capabilities\":[{\"capability_id\":\"safety\",\"anchor_ids\":[\"anchor-1\"]}]},\"attempt_id\":\"attempt-1\",\"fixed_sample_claim_id\":\"fixed-gain\",\"billing_scope\":\"billing-1\",\"executor_actor\":\"executor\",\"evaluator_actor\":\"evaluator\",\"proposer_actor\":\"proposer\",\"approver_actor\":\"approver\",\"oracle_digest\":\"9297ea092eae8ad974be8d2c9d294283d0e5aeed5a3e34886dfb954dcf1ac6e3\",\"grader_digest\":\"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a\",\"fixed_grader\":{\"schema_version\":\"rsia.fixed_grader.v1\",\"version\":\"exact-json-v1\",\"method\":\"exact_json_answer_v1\"},\"evidence_scope\":\"program_fixture\",\"created_at_unix_seconds\":2}}","revision":1}],"cache":[{"namespace":"n","actor":"evaluator","operation":"experiment.register","request_key":"ag155-capture-registration","payload_hash":"d22f58f331a893f330c9280c5a8570ebaea912a807678a2bbadbf5c1124aebc6","subject_id":"management-job-ae10c9ec67325b06cea1ff97","response":"{\"id\":\"management-job-ae10c9ec67325b06cea1ff97\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"experiment.register\",\"request_key\":\"ag155-capture-registration\",\"payload_digest\":\"d22f58f331a893f330c9280c5a8570ebaea912a807678a2bbadbf5c1124aebc6\",\"owner_actor\":\"evaluator\",\"owner_role\":\"evaluator\",\"state\":\"queued\",\"step\":\"accepted\",\"private_input_ref\":\"management-input-ae10c9ec67325b06cea1ff97\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":null,\"lease_until\":0,\"generation\":0,\"diagnostics\":[],\"created_at\":1793306575}","redacted":0}]}}"###;

struct LegacyFixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: Store,
    ctx: Context,
    sample: Value,
}
async fn legacy(case: &str) -> LegacyFixture {
    let all: Value = serde_json::from_str(LEGACY).unwrap();
    let sample = all[case].clone();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("versions.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let role = if sample["role"] == "evaluator" {
        Role::Evaluator
    } else {
        Role::Admin
    };
    let ctx = Context::new(
        sample["namespace"].as_str().unwrap(),
        sample["actor"].as_str().unwrap(),
        role,
    )
    .unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    for row in sample["objects"].as_array().unwrap() {
        sqlx::query(
            "INSERT INTO objects(namespace,kind,id,owner,body,revision) VALUES(?,?,?,?,?,?)",
        )
        .bind(row["namespace"].as_str().unwrap())
        .bind(row["kind"].as_str().unwrap())
        .bind(row["id"].as_str().unwrap())
        .bind(row["owner"].as_str().unwrap())
        .bind(row["body"].as_str().unwrap())
        .bind(row["revision"].as_i64().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    }
    for row in sample["cache"].as_array().unwrap() {
        sqlx::query("INSERT INTO idempotency(namespace,actor,operation,request_key,payload_hash,subject_id,response,redacted) VALUES(?,?,?,?,?,?,?,?)")
            .bind(row["namespace"].as_str().unwrap()).bind(row["actor"].as_str().unwrap())
            .bind(row["operation"].as_str().unwrap()).bind(row["request_key"].as_str().unwrap())
            .bind(row["payload_hash"].as_str().unwrap()).bind(row["subject_id"].as_str().unwrap())
            .bind(row["response"].as_str().unwrap()).bind(row["redacted"].as_i64().unwrap())
            .execute(&pool).await.unwrap();
    }
    pool.close().await;
    LegacyFixture {
        _dir: dir,
        path,
        store,
        ctx,
        sample,
    }
}
async fn legacy_body(fx: &LegacyFixture, kind: &str, id: &str) -> String {
    let mut s = fx.store.session().await.unwrap();
    let row = s.object_row(&fx.ctx, kind, id).await.unwrap().unwrap();
    s.commit().await.unwrap();
    row.body
}
async fn legacy_terminal(
    dispatcher: &ManagementDispatcher,
    fx: &LegacyFixture,
    id: &str,
) -> VersionedManagementJob {
    for _ in 0..500 {
        let row = dispatcher.status(&fx.ctx, id).await.unwrap();
        if matches!(
            row.identity().state,
            ManagementJobState::Succeeded
                | ManagementJobState::Failed
                | ManagementJobState::Cancelled
                | ManagementJobState::Blocked
        ) {
            return row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("real compatibility consumer did not close");
}

fn current(record: VersionedManagementJob) -> ManagementJobV2 {
    match record {
        VersionedManagementJob::V2(v) => v,
        VersionedManagementJob::V1(_) => panic!("expected real current producer"),
    }
}
async fn unclaimed(
    store: Store,
    ctx: Context,
    operation: &str,
    payload: Value,
) -> VersionedManagementJob {
    let operation = operation.to_owned();
    let accepted = tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let accepted = runtime.block_on(async {
            ManagementDispatcher::new(store, vec![ctx.clone()])
                .unwrap()
                .submit(&ctx, &operation, payload)
                .await
                .unwrap()
        });
        drop(runtime);
        accepted
    })
    .await
    .unwrap();
    assert_eq!(accepted.identity().state, ManagementJobState::Queued);
    assert_eq!(accepted.generation(), 0);
    assert!(accepted.identity().lease_token.is_none());
    accepted
}
async fn raw_record(store: &Store, ctx: &Context, id: &str) -> String {
    let mut s = store.session().await.unwrap();
    let row = s.object_row(ctx, "job", id).await.unwrap().unwrap();
    s.commit().await.unwrap();
    row.body
}

#[tokio::test]
async fn cancel_before_first_commit_has_no_business_side_effect() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("queued.sqlite3"))
        .await
        .unwrap();
    let ctx = Context::new("n", "admin", Role::Admin).unwrap();
    let request = json!({"schema_version":"rsia.management.meta_start.v1","request_key":"cancel-never-claimed"});
    let accepted = unclaimed(store.clone(), ctx.clone(), "meta.start", request.clone()).await;
    let id = accepted.identity().id.to_owned();
    let d = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let stopped = current(d.cancel(&ctx, &id).await.unwrap());
    assert_eq!(stopped.state, ManagementJobState::Cancelled);
    assert_eq!(stopped.generation, 0);
    assert!(stopped.result.is_none());
    assert_eq!(
        stopped.cancel_arrival,
        Some(evo_storage::ManagementCancelArrival::BeforeSideEffect)
    );
    assert_eq!(
        d.submit(&ctx, "meta.start", request)
            .await
            .unwrap()
            .identity()
            .id,
        id
    );
    // A new Current job receives C as an input; it did not create that C.
    let fx = legacy("control_committed_holdout_not_committed").await;
    let mut c = fx.sample["request"]["control"].clone();
    c["schema_version"] = json!(RegisteredEvaluationControlV3::SCHEMA);
    c["id"] = json!("new-current-input-control");
    c["max_regression"] = json!(0.125);
    let control: RegisteredEvaluationControlV3 = serde_json::from_value(c).unwrap();
    control.validate().unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    fx.store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: control.billing_scope.clone(),
                allowed_namespaces: vec!["n".into()],
                currency: control.optimization_budget.root_total.currency.clone(),
                pricing_version: control
                    .optimization_budget
                    .root_total
                    .pricing_version
                    .clone(),
                payment_subject: "fixture-only".into(),
                authorization_receipt_digest: control
                    .optimization_budget
                    .admin_authorization_receipt_digest
                    .clone()
                    .unwrap(),
                per_call_cap_micros: 100,
                total_limit_micros: 1000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    IndependentEvaluationControl::register_current_control(&fx.ctx, &fx.store, control.clone())
        .await
        .unwrap();
    let mut h = fx.sample["request"]["holdout"].clone();
    h["schema_version"] = json!(ProtectedHoldoutRecordV2::SCHEMA);
    h["id"] = json!("new-current-input-holdout");
    h["registration_id"] = json!(control.id);
    h["frozen_configuration_digest"] =
        json!(control.frozen_configuration().unwrap().digest().unwrap());
    let holdout: ProtectedHoldoutRecordV2 = serde_json::from_value(h).unwrap();
    holdout.validate_against(&control).unwrap();
    let request = serde_json::to_value(ExperimentRegisterRequestV2 {
        schema_version: "rsia.management.experiment_register.v2".into(),
        request_key: "current-input-only".into(),
        control: control.clone(),
        holdout: holdout.clone(),
    })
    .unwrap();
    let c_before = IndependentEvaluationControl::registration_progress(
        &fx.ctx,
        &fx.store,
        &control.id,
        &holdout.id,
    )
    .await
    .unwrap();
    assert!(c_before.control_digest.is_some());
    assert!(c_before.holdout_digest.is_none());
    let accepted = unclaimed(
        fx.store.clone(),
        fx.ctx.clone(),
        "experiment.register",
        request,
    )
    .await;
    let d = ManagementDispatcher::new(fx.store.clone(), vec![fx.ctx.clone()]).unwrap();
    let cancelled = current(d.cancel(&fx.ctx, accepted.identity().id).await.unwrap());
    assert_eq!(cancelled.state, ManagementJobState::Cancelled);
    assert!(cancelled.result.is_none());
    assert_eq!(
        cancelled.cancel_arrival,
        Some(evo_storage::ManagementCancelArrival::BeforeSideEffect)
    );
    let status = d.status_view(&fx.ctx, &cancelled.id).await.unwrap();
    assert_eq!(
        serde_json::to_value(status.committed_side_effects).unwrap(),
        json!([])
    );
    assert_eq!(
        IndependentEvaluationControl::registration_progress(
            &fx.ctx,
            &fx.store,
            &control.id,
            &holdout.id
        )
        .await
        .unwrap(),
        c_before
    );
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", fx.path.display()))
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER hold_current_finish BEFORE UPDATE OF body ON objects
         WHEN NEW.kind='job'
           AND json_extract(NEW.body,'$.request_key')='current-owned-holdout-true'
           AND json_extract(NEW.body,'$.step')='current_holdout_registered'
           AND json_extract(NEW.body,'$.state')='succeeded'
         BEGIN SELECT RAISE(ABORT,'hold actual Current completion'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    for cancel_after in [false, true] {
        let mut produced = holdout.clone();
        produced.id = format!("current-owned-holdout-{cancel_after}");
        let payload = serde_json::to_value(ExperimentRegisterRequestV2 {
            schema_version: "rsia.management.experiment_register.v2".into(),
            request_key: produced.id.clone(),
            control: control.clone(),
            holdout: produced.clone(),
        })
        .unwrap();
        let expected_result = json!({
            "result": "experiment_registered",
            "control_id": control.id,
            "control_digest": control.digest().unwrap(),
            "holdout_id": produced.id,
            "holdout_digest": produced.digest(&control).unwrap(),
        });
        let expected_effects = json!([{
            "kind": "holdout",
            "holdout_id": produced.id,
            "holdout_digest": produced.digest(&control).unwrap(),
        }]);
        let accepted = d
            .submit(&fx.ctx, "experiment.register", payload)
            .await
            .unwrap();
        let id = accepted.identity().id;
        if cancel_after {
            // The original producer committed H and the Full checkpoint. A
            // single local completion-write fault leaves its real supervisor
            // waiting, so the public cancel runs in the actual after window.
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let record = d.status(&fx.ctx, id).await.unwrap();
                    if record.step() == "current_holdout_registered" {
                        assert_eq!(
                            serde_json::to_value(record.result()).unwrap(),
                            expected_result
                        );
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("actual Current H checkpoint was not committed");
            let cancelled = d.cancel(&fx.ctx, id).await.unwrap();
            assert_eq!(cancelled.identity().state, ManagementJobState::Draining);
            assert_eq!(
                cancelled.cancel_arrival(),
                Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
            );
            assert_eq!(
                serde_json::to_value(cancelled.result()).unwrap(),
                expected_result
            );
        } else {
            let complete = legacy_terminal(&d, &fx, id).await;
            assert_eq!(complete.identity().state, ManagementJobState::Succeeded);
            assert_eq!(complete.step(), "current_holdout_registered");
            assert_eq!(
                serde_json::to_value(complete.result()).unwrap(),
                expected_result
            );
        }
        let view = d.status_view(&fx.ctx, id).await.unwrap();
        assert_eq!(
            serde_json::to_value(view.committed_side_effects).unwrap(),
            expected_effects
        );
        assert_eq!(serde_json::to_value(view.result).unwrap(), expected_result);
        if cancel_after {
            let mut cleanup = LifecycleStore::begin_revoke(
                &admin,
                &fx.store,
                TypedObjectRef {
                    kind: "artifact".into(),
                    id: accepted.identity().private_input_ref.into(),
                },
                "after actual Current H commit",
                now().unwrap(),
            )
            .await
            .unwrap();
            for _ in 0..100 {
                cleanup = LifecycleStore::cleanup_step(
                    &admin,
                    &fx.store,
                    &cleanup.job_id,
                    8,
                    now().unwrap(),
                )
                .await
                .unwrap();
                if cleanup.state == CleanupState::Complete {
                    break;
                }
            }
            assert_eq!(cleanup.state, CleanupState::Complete);
            let view = d.status_view(&fx.ctx, id).await.unwrap();
            assert_eq!(
                serde_json::to_value(view.committed_side_effects).unwrap(),
                expected_effects
            );
            assert_eq!(serde_json::to_value(view.result).unwrap(), expected_result);
            assert_eq!(
                view.cancel_arrival,
                Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
            );
            sqlx::query("DROP TRIGGER hold_current_finish")
                .execute(&pool)
                .await
                .unwrap();
            let done = tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    let record =
                        decode_management_job(&raw_record(&fx.store, &fx.ctx, id).await).unwrap();
                    if record.identity().state == ManagementJobState::Succeeded {
                        break record;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("actual observed Current worker did not finish");
            assert_eq!(done.step(), "current_holdout_registered");
            assert_eq!(
                serde_json::to_value(done.result()).unwrap(),
                expected_result
            );
            assert_eq!(
                done.cancel_arrival(),
                Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
            );
            // Completed source-derived results keep the original public
            // revocation refusal, while their durable references remain intact.
            assert!(matches!(
                d.status(&fx.ctx, id).await,
                Err(Error::Conflict(_))
            ));
        }
    }
    // Both C/H can already exist under a new business key. Returning the full
    // registration references is then a readonly workflow, not this job's DML.
    IndependentEvaluationControl::register_current_holdout(&fx.ctx, &fx.store, holdout.clone())
        .await
        .unwrap();
    let before = IndependentEvaluationControl::registration_progress(
        &fx.ctx,
        &fx.store,
        &control.id,
        &holdout.id,
    )
    .await
    .unwrap();
    let mut readonly = serde_json::to_value(ExperimentRegisterRequestV2 {
        schema_version: "rsia.management.experiment_register.v2".into(),
        request_key: "existing-registration-read".into(),
        control: control.clone(),
        holdout: holdout.clone(),
    })
    .unwrap();
    let accepted = unclaimed(
        fx.store.clone(),
        fx.ctx.clone(),
        "experiment.register",
        readonly.clone(),
    )
    .await;
    let d = ManagementDispatcher::new(fx.store.clone(), vec![fx.ctx.clone()]).unwrap();
    d.recover_pending().await.unwrap();
    let read = legacy_terminal(&d, &fx, accepted.identity().id).await;
    assert_eq!(read.identity().state, ManagementJobState::Succeeded);
    assert_eq!(read.step(), "experiment_already_registered");
    assert_eq!(
        serde_json::to_value(read.result()).unwrap()["result"],
        "experiment_registered"
    );
    let view = d.status_view(&fx.ctx, read.identity().id).await.unwrap();
    assert_eq!(
        serde_json::to_value(view.committed_side_effects).unwrap(),
        json!([])
    );
    assert_eq!(
        serde_json::to_value(view.fee_knowledge).unwrap()["state"],
        "known_none"
    );
    assert!(view.pending_fees.unwrap().is_empty());
    sqlx::query("CREATE TRIGGER cancel_readonly_checkpoint AFTER UPDATE OF body ON objects WHEN NEW.kind='job' AND json_extract(NEW.body,'$.request_key')='existing-registration-cancel' AND json_extract(NEW.body,'$.step')='experiment_already_registered' AND json_extract(NEW.body,'$.cancel_requested')=0 BEGIN UPDATE objects SET body=json_set(NEW.body,'$.cancel_requested',json('true')) WHERE namespace=NEW.namespace AND kind=NEW.kind AND id=NEW.id; END").execute(&pool).await.unwrap();
    readonly["request_key"] = json!("existing-registration-cancel");
    let accepted = d
        .submit(&fx.ctx, "experiment.register", readonly)
        .await
        .unwrap();
    let cancelled = legacy_terminal(&d, &fx, accepted.identity().id).await;
    assert_eq!(cancelled.identity().state, ManagementJobState::Cancelled);
    assert_eq!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::BeforeSideEffect)
    );
    assert!(cancelled.result().is_none());
    assert_eq!(
        IndependentEvaluationControl::registration_progress(
            &fx.ctx,
            &fx.store,
            &control.id,
            &holdout.id
        )
        .await
        .unwrap(),
        before
    );
    pool.close().await;
}

#[tokio::test]
async fn cancel_after_control_commit_blocks_partial_and_skips_holdout() {
    let fx = legacy("control_committed_holdout_not_committed").await;
    let id = fx.sample["job_id"].as_str().unwrap();
    let input = fx.sample["input_id"].as_str().unwrap();
    let before_input = legacy_body(&fx, "artifact", input).await;
    let old = decode_management_job(&legacy_body(&fx, "job", id).await).unwrap();
    assert!(matches!(old, VersionedManagementJob::V1(_)));
    assert!(old.generation() > 0);
    assert_eq!(old.step(), "control_registered");
    let request = fx.sample["request"].clone();
    let cid = request["control"]["id"].as_str().unwrap();
    let hid = request["holdout"]["id"].as_str().unwrap();
    let progress =
        IndependentEvaluationControl::registration_progress(&fx.ctx, &fx.store, cid, hid)
            .await
            .unwrap();
    assert!(progress.control_digest.is_some());
    assert!(progress.holdout_digest.is_none());
    let d = ManagementDispatcher::new(fx.store.clone(), vec![fx.ctx.clone()]).unwrap();
    let cancelled = d.cancel(&fx.ctx, id).await.unwrap();
    assert!(cancelled.cancel_requested());
    assert_ne!(
        cancelled.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::BeforeSideEffect)
    );
    d.recover_pending().await.unwrap();
    let done = legacy_terminal(&d, &fx, id).await;
    assert_eq!(done.identity().state, ManagementJobState::Blocked);
    assert_eq!(done.error_code(), Some("blocked_partial_commit"));
    assert_eq!(done.identity().id, id);
    assert_eq!(done.generation(), old.generation());
    assert_eq!(
        done.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
    );
    assert_eq!(
        IndependentEvaluationControl::registration_progress(&fx.ctx, &fx.store, cid, hid)
            .await
            .unwrap(),
        progress
    );
    assert_eq!(legacy_body(&fx, "artifact", input).await, before_input);
    assert_eq!(
        d.submit(&fx.ctx, "experiment.register", request)
            .await
            .unwrap()
            .identity()
            .id,
        id
    );
}

#[tokio::test]
async fn holdout_failure_keeps_actual_control_reference() {
    let fx = legacy("control_committed_holdout_not_committed").await;
    let id = fx.sample["job_id"].as_str().unwrap();
    let request = &fx.sample["request"];
    let cid = request["control"]["id"].as_str().unwrap();
    let hid = request["holdout"]["id"].as_str().unwrap();
    let progress =
        IndependentEvaluationControl::registration_progress(&fx.ctx, &fx.store, cid, hid)
            .await
            .unwrap();
    assert!(progress.control_digest.is_some());
    assert!(progress.holdout_digest.is_none());
    let d = ManagementDispatcher::new(fx.store.clone(), vec![fx.ctx.clone()]).unwrap();
    assert_eq!(d.recover_pending().await.unwrap(), 1);
    let done = legacy_terminal(&d, &fx, id).await;
    assert_eq!(done.identity().state, ManagementJobState::Blocked);
    assert_eq!(done.error_code(), Some("blocked_partial_commit"));
    assert!(!done.cancel_requested());
    let result = serde_json::to_value(done.result()).unwrap();
    assert_eq!(result["control_id"], cid);
    assert_eq!(
        result["control_digest"],
        progress.control_digest.as_deref().unwrap()
    );
    assert!(result.get("holdout_id").is_none());
    assert_eq!(
        IndependentEvaluationControl::registration_progress(&fx.ctx, &fx.store, cid, hid)
            .await
            .unwrap(),
        progress
    );
}

#[tokio::test]
async fn repeated_cancel_of_running_job_is_logically_zero_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("running.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let ctx = Context::new("n", "admin", Role::Admin).unwrap();
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE owned_running_capture(id TEXT,owner TEXT,body TEXT,revision INTEGER)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER capture_running AFTER UPDATE OF body ON objects WHEN NEW.kind='job' AND json_extract(NEW.body,'$.schema_version')='rsia.management_job.v2' AND json_extract(NEW.body,'$.state')='running' AND NOT EXISTS(SELECT 1 FROM owned_running_capture) BEGIN INSERT INTO owned_running_capture VALUES(NEW.id,NEW.owner,NEW.body,NEW.revision); END").execute(&pool).await.unwrap();
    let d = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let accepted=d.submit(&ctx,"meta.start",json!({"schema_version":"rsia.management.meta_start.v1","request_key":"capture-running"})).await.unwrap();
    let id = accepted.identity().id.to_owned();
    for _ in 0..500 {
        if d.status(&ctx, &id).await.unwrap().identity().state == ManagementJobState::Blocked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let captured: String = sqlx::query_scalar("SELECT body FROM owned_running_capture WHERE id=?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let row = decode_management_job(&captured).unwrap();
    assert_eq!(row.identity().state, ManagementJobState::Running);
    assert!(row.generation() > 0);
    assert!(row.identity().lease_token.is_some());
    // Explicit reconstruction of the one complete real claim row; no business
    // output or fee record is manufactured, and this is not a crash capture.
    sqlx::query("UPDATE objects SET body=(SELECT body FROM owned_running_capture),revision=(SELECT revision FROM owned_running_capture) WHERE namespace='n' AND kind='job' AND id=?").bind(&id).execute(&pool).await.unwrap();
    d.cancel(&ctx, &id).await.unwrap();
    let before = full_image(&path);
    let again = d.cancel(&ctx, &id).await.unwrap();
    assert_eq!(again.generation(), row.generation());
    assert_eq!(again.identity().lease_token, row.identity().lease_token);
    assert_eq!(full_image(&path), before);
    pool.close().await;
}

#[derive(Clone)]
struct Witness {
    call_id: String,
    group: String,
    dispatch_id: String,
    charge: UsageCharge,
}
#[derive(Clone)]
struct Gate {
    mode: u8,
    calls: Arc<AtomicUsize>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Semaphore>,
    witnesses: Arc<Mutex<Vec<Witness>>>,
}
impl Gate {
    fn new(mode: u8) -> Self {
        Self {
            mode,
            calls: Arc::new(AtomicUsize::new(0)),
            entered: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Semaphore::new(0)),
            witnesses: Arc::new(Mutex::new(Vec::new())),
        }
    }
}
#[async_trait]
impl ModelTransport for Gate {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }
    async fn execute(
        &self,
        request: &ModelRequest,
        dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let output = if self.mode == 0 {
            fixture_suggestion(request)?
        } else {
            "measured fixture work completed before link loss".into()
        };
        let charge = UsageCharge {
            amount_micros: 10,
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
            provider_request_id: format!("fixture-provider-{}", request.request_id),
            usage_record_id: format!("fixture-usage-{}", request.request_id),
            output_digest: hash(output.as_bytes()),
        };
        self.witnesses.lock().unwrap().push(Witness {
            call_id: request.request_id.clone(),
            group: request.episode_id.clone(),
            dispatch_id: dispatch_id.into(),
            charge: charge.clone(),
        });
        self.entered.notify_one();
        if self.mode != 0 {
            let permit = self.release.acquire().await.map_err(|_| Error::Internal)?;
            permit.forget();
            if self.mode == 2 {
                panic!("controlled fixture worker exits after actual dispatch");
            }
            return Err(Error::Internal);
        }
        Ok(TransportCompletion {
            response_id: format!("fixture-response-{}", request.request_id),
            provider_request_id: charge.provider_request_id,
            usage_record_id: charge.usage_record_id,
            actual_model_digest: request.model_digest.clone(),
            output,
            actual_cost_micros: charge.amount_micros,
            currency: charge.currency,
            pricing_version: charge.pricing_version,
        })
    }
}
fn fixture_suggestion(request: &ModelRequest) -> Result<String> {
    let source = request
        .source_closure
        .first()
        .ok_or(Error::NotFound)?
        .clone();
    let parent: SkillSnapshot = serde_json::from_str(
        &request
            .input
            .iter()
            .find(|p| p.label == "parent-skill")
            .ok_or(Error::NotFound)?
            .content,
    )
    .map_err(|_| Error::Internal)?;
    let (id, batch, field, start) = match request.stage {
        ModelStage::ReflectFailure => (
            "fix-rule",
            "reflection-failure",
            SkillTextField::Content,
            parent.content.len(),
        ),
        ModelStage::ReflectSuccess => (
            "preserve-rule",
            "reflection-success",
            SkillTextField::Applicability,
            parent.applicability.len(),
        ),
        _ => return Err(Error::Invalid("unexpected fixture stage".into())),
    };
    serde_json::to_string(&vec![EditSuggestion {
        id: id.into(),
        hypothesis: "bounded fixture hypothesis".into(),
        batch_ids: vec![batch.into()],
        support: vec![source.clone()],
        counterexamples: if request.stage == ModelStage::ReflectSuccess {
            vec![source.clone()]
        } else {
            vec![]
        },
        dependencies: vec![source],
        edit: SkillTextEdit {
            field,
            start,
            end: start,
            expected_text_digest: hash(b""),
            exact_anchor: None,
            operation: TextEditOperation::Insert {
                text: " [fixture]".into(),
            },
        },
    }])
    .map_err(|_| Error::Internal)
}
struct PaidFixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: Store,
    admin: Context,
    gate: Gate,
    dispatcher: ManagementDispatcher,
    request: Value,
}
async fn paid_fixture(mode: u8) -> PaidFixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paid.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let host = Context::new("n", "host", Role::Host).unwrap();
    let journal = Context::new("n", "optimizer", Role::Worker).unwrap();
    let caps = HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    };
    let skill = SkillSnapshot {
        content: "old".into(),
        applicability: "applies".into(),
        counterexample: "counter".into(),
        required_capabilities: vec![],
        dependencies: vec![],
    };
    let originals = trusted_compiler_inputs::inputs(
        "n",
        "management-profile",
        skill,
        Strategy::default(),
        caps.clone(),
        &hash(b"environment"),
    );
    trusted_compiler_inputs::install(&store, &originals)
        .await
        .unwrap();
    let compiler = CompilerInputRequest {
        profile: originals.profile.clone(),
        search_parent: originals.root.skill.clone(),
        baseline: originals.frozen.baseline_skill.clone(),
        search_parent_strategy: originals.root.strategy.clone(),
        expected_parent_strategy_digest: None,
        baseline_strategy: originals.frozen.baseline_strategy.clone(),
        capabilities: caps,
        environment_digest: hash(b"environment"),
        purpose: Purpose::Development,
        revoke_watermark: 1,
    };
    for (id, family, outcome) in [
        ("run-failure", "family-a", TraceOutcome::TaskFailure),
        ("run-success", "family-b", TraceOutcome::Success),
    ] {
        let trace = OptimizationTrace {
            run_id: id.into(),
            parent_family: family.into(),
            source_digest: hash(id.as_bytes()),
            purpose: Purpose::Development,
            outcome,
            diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
                kind: SkillFailureKind::SkillDefect,
                skill_id: "skill-a".into(),
                bundle_digest: compiler.profile.parent_digest.clone(),
                request_digest: hash(b"task-request"),
                rule_id: Some("rule-a".into()),
                support: vec![EvidenceRef {
                    id: id.into(),
                    digest: hash(id.as_bytes()),
                }],
                counterexamples: vec![],
                reason: "fixture defect".into(),
            }),
            excerpt: id.into(),
            seed: 1,
        };
        store_trace_authority(
            &store,
            &host,
            &StoredTraceAuthority {
                schema_version: "rsia.optimization.source.v1".into(),
                record: StoredRunRecord {
                    id: id.into(),
                    body: id.as_bytes().to_vec(),
                    parent_family: family.into(),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                trace,
                excerpt_start: 0,
                excerpt_end: id.len(),
            },
        )
        .await
        .unwrap();
    }
    let selection = SourceSelection {
        roots: vec![],
        run_ids: vec!["run-failure".into(), "run-success".into()],
        purpose: Purpose::Development,
        allow_model_excerpts: true,
    };
    store_source_selection(&store, &host, &selection)
        .await
        .unwrap();
    let mut s = store.session().await.unwrap();
    s.bump_watermark(&admin, &hash(b"initial-watermark"))
        .await
        .unwrap();
    let template = SkillEditBatch {
        schema_version: SKILL_EDIT_SCHEMA.into(),
        compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
        namespace: "n".into(),
        profile_id: compiler.profile.id.clone(),
        skill_id: "skill-a".into(),
        skill_version: "v1".into(),
        input_digest: skill_snapshot_digest(&compiler.search_parent).unwrap(),
        approved_parent_digest: compiler.profile.parent_digest.clone(),
        safe_baseline_digest: compiler.profile.baseline_digest.clone(),
        evidence: EvidenceClosure {
            support: vec![],
            counterexamples: vec![],
            dependencies: vec![],
        },
        edits: vec![],
    };
    s.put(
        &admin,
        "artifact",
        "management-edit-template",
        admin.actor(),
        &template,
    )
    .await
    .unwrap();
    s.put(
        &admin,
        "artifact",
        "management-improver-patch",
        admin.actor(),
        &ImproverPatch::default(),
    )
    .await
    .unwrap();
    s.commit().await.unwrap();
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "management-root".into(),
                billing_scope: "management-scope".into(),
                allowed_namespaces: vec!["n".into()],
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "fixture-only".into(),
                authorization_receipt_digest: hash(b"real-admin-software-authorization"),
                per_call_cap_micros: 20,
                total_limit_micros: 1000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let profile = RegisteredPureFunctionProfileV1::clamp_i64();
    let grader = FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    };
    let mut control = DevelopmentControlV1 {
        schema_version: DevelopmentControlV1::SCHEMA.into(),
        id: "management-development".into(),
        namespace: "n".into(),
        billing_scope: "management-scope".into(),
        root_budget_id: "management-root".into(),
        executor_actor: "dev-executor".into(),
        grader_actor: "dev-grader".into(),
        proposer_actor: "optimizer".into(),
        manifest_id: "management-manifest".into(),
        manifest_digest: String::new(),
        tasks: vec![
            DevelopmentTaskSpecV1::registered(
                "task-a",
                "family-a",
                RegisteredTargetInputV1::ClampI64 {
                    value: 2,
                    min: -1,
                    max: 1,
                },
            )
            .unwrap(),
        ],
        environment_digest: compiler.environment_digest.clone(),
        fixed_grader: grader.clone(),
        grader_digest: fingerprint(&grader).unwrap(),
        oracle_digest: profile.oracle_digest,
        target_digest: profile.target_digest,
        runner_digest: registered_runner_digest().unwrap(),
        rules_digest: hash(b"rules"),
        tools_digest: hash(b"tools"),
        source_ids: vec!["run-failure".into(), "run-success".into()],
        evidence_scope: DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
        created_at_unix_seconds: 1,
    };
    control.manifest_digest = control.manifest().unwrap().digest;
    register_development_control(&admin, &store, control)
        .await
        .unwrap();
    let gate = Gate::new(mode);
    let broker = Arc::new(
        PersistentModelBroker::new(
            store.clone(),
            gate.clone(),
            BrokerConfig {
                billing_scope: "management-scope".into(),
                actor: "optimizer".into(),
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                max_cost_micros: 20,
                lease_seconds: 180,
            },
        )
        .unwrap(),
    );
    let runner = Arc::new(
        RegisteredDevelopmentRunner::new(
            store.clone(),
            Context::new("n", "dev-executor", Role::Worker).unwrap(),
            Context::new("n", "dev-grader", Role::Evaluator).unwrap(),
            "management-development",
            180,
        )
        .unwrap(),
    );
    let runtime = ManagementOptimizationRuntime::new(
        broker,
        runner,
        journal,
        ManagementOptimizationSettings {
            model_digest: hash(b"fixture-model"),
            sampling_digest: hash(b"sampling"),
            max_suggestions: 1,
            protected_ranges: vec![],
            allow_rank_call: false,
        },
    )
    .unwrap();
    let dispatcher = ManagementDispatcher::with_optimization_runtime(
        store.clone(),
        vec![admin.clone()],
        Some(runtime),
    )
    .unwrap();
    let request = serde_json::to_value(OptimizationStepManagementRequest {
        schema_version: "rsia.management.optimization_step.v1".into(),
        request_key: "paid-management".into(),
        compiler,
        source_selection_ref: format!("optgrant-{}", fingerprint(&selection).unwrap()),
        edit_template_ref: "management-edit-template".into(),
        improver_patch_ref: "management-improver-patch".into(),
        development_control_id: "management-development".into(),
    })
    .unwrap();
    PaidFixture {
        _dir: dir,
        path,
        store,
        admin,
        gate,
        dispatcher,
        request,
    }
}
async fn paid_start(fx: &PaidFixture) -> ManagementJobV2 {
    let record = fx
        .dispatcher
        .submit(&fx.admin, "optimization.step", fx.request.clone())
        .await
        .unwrap();
    let id = record.identity().id.to_owned();
    tokio::time::timeout(Duration::from_secs(30), fx.gate.entered.notified())
        .await
        .expect("real broker never reached the configured transport");
    let current = decode_management_job(&raw_record(&fx.store, &fx.admin, &id).await).unwrap();
    assert_eq!(current.identity().state, ManagementJobState::Running);
    let job = match current {
        VersionedManagementJob::V2(v) => v,
        _ => panic!("expected v2 claim"),
    };
    assert!(job.optimization.as_ref().unwrap().calls.len() > 0);
    job
}
async fn paid_state(fx: &PaidFixture, id: &str, state: ManagementJobState) -> ManagementJobV2 {
    let start = Instant::now();
    loop {
        let current = current(fx.dispatcher.status(&fx.admin, id).await.unwrap());
        if current.state == state {
            return current;
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "actual worker did not reach {state:?}: {current:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn call_rows(fx: &PaidFixture) -> Vec<BudgetCallRecord> {
    let group = fx
        .gate
        .witnesses
        .lock()
        .unwrap()
        .first()
        .expect("actual transport witness")
        .group
        .clone();
    let mut s = fx.store.session().await.unwrap();
    let rows = s
        .budget_calls_for_group(&fx.admin, "management-scope", &group)
        .await
        .unwrap();
    s.commit().await.unwrap();
    rows
}
async fn reconcile(fx: &PaidFixture) {
    let witnesses = fx.gate.witnesses.lock().unwrap().clone();
    for witness in witnesses {
        fx.store
            .reconcile_budget_call_cost(
                &fx.admin,
                "management-scope",
                &witness.call_id,
                &witness.charge,
                now().unwrap(),
            )
            .await
            .unwrap();
        fx.store
            .close_budget_call_execution(
                &fx.admin,
                "management-scope",
                &witness.call_id,
                &witness.dispatch_id,
                "actual_fixture_worker_exited",
                now().unwrap(),
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn draining_recovery_never_claims_or_reexecutes_request() {
    tokio::task::spawn_blocking(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for fault in ["missing", "redacted", "invalid"] {
                let fx = legacy("control_committed_holdout_not_committed").await;
                let id = fx.sample["job_id"].as_str().unwrap();
                let cid = fx.sample["request"]["control"]["id"].as_str().unwrap();
                let hid = fx.sample["request"]["holdout"]["id"].as_str().unwrap();
                let progress = IndependentEvaluationControl::registration_progress(
                    &fx.ctx, &fx.store, cid, hid,
                )
                .await
                .unwrap();
                let mut damaged = decode_management_job(&raw_record(&fx.store, &fx.ctx, id).await)
                    .unwrap()
                    .into_unfinished_v2()
                    .unwrap();
                // Explicit damaged-row fixtures derived from the actual old producer.
                // Source loss cannot turn its C reference into a new ordinary claim.
                damaged.state = ManagementJobState::Queued;
                damaged.lease_token = None;
                damaged.lease_until = 0;
                if fault == "invalid" {
                    damaged.generation = 0;
                }
                if fault != "redacted" {
                    damaged.result = Some(evo_storage::ManagementResultV2::Partial(
                        evo_storage::ManagementPartialResultV2::ControlRegistered {
                            control_id: cid.into(),
                            control_digest: progress.control_digest.clone().unwrap(),
                        },
                    ));
                }
                let mut s = fx.store.session().await.unwrap();
                s.put(&fx.ctx, "job", id, fx.ctx.actor(), &damaged)
                    .await
                    .unwrap();
                s.commit().await.unwrap();
                let admin = Context::new("n", "admin", Role::Admin).unwrap();
                let fresh = unclaimed(
                    fx.store.clone(),
                    admin.clone(),
                    "meta.start",
                    json!({
                        "schema_version": "rsia.management.meta_start.v1",
                        "request_key": format!("never-started-{fault}"),
                    }),
                )
                .await;
                let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", fx.path.display()))
                    .await
                    .unwrap();
                for (ctx, identity, started) in [
                    (&fx.ctx, damaged.id.clone(), true),
                    (&admin, fresh.identity().id.to_owned(), false),
                ] {
                    let record = decode_management_job(&raw_record(&fx.store, ctx, &identity).await)
                        .unwrap();
                    let input = record.identity().private_input_ref;
                    if fault == "missing" {
                        sqlx::query(
                            "DELETE FROM objects WHERE namespace='n' AND kind='artifact' AND id=?",
                        )
                        .bind(input)
                        .execute(&pool)
                        .await
                        .unwrap();
                    } else {
                        let body = if fault == "redacted" {
                            json!({
                                "id": input,
                                "schema_version": "rsia.redacted.v1",
                                "state": "source_revoked",
                            })
                            .to_string()
                        } else {
                            "{".into()
                        };
                        sqlx::query(
                            "UPDATE objects SET body=? WHERE namespace='n' AND kind='artifact' AND id=?",
                        )
                        .bind(body)
                        .bind(input)
                        .execute(&pool)
                        .await
                        .unwrap();
                    }
                    let mut s = fx.store.session().await.unwrap();
                    assert_eq!(
                        s.capacity_usage_v41(now().unwrap() as u64)
                            .await
                            .unwrap()
                            .concurrent_dispatches,
                        1,
                    );
                    s.commit().await.unwrap();
                    let restart =
                        ManagementDispatcher::new(fx.store.clone(), vec![ctx.clone()]).unwrap();
                    assert_eq!(restart.recover_pending().await.unwrap(), 1);
                    let expected = if started {
                        ManagementJobState::Draining
                    } else {
                        ManagementJobState::Failed
                    };
                    let recovered = tokio::time::timeout(Duration::from_secs(10), async {
                        loop {
                            let recovered = current(restart.status(ctx, &identity).await.unwrap());
                            if recovered.state == expected {
                                break recovered;
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .expect("source fault did not reach its evidence-backed state");
                    if started {
                        let mut expected = damaged.clone();
                        expected.state = ManagementJobState::Draining;
                        assert_eq!(
                            serde_json::to_value(&recovered).unwrap(),
                            serde_json::to_value(expected).unwrap(),
                        );
                        let view = restart.status_view(ctx, &identity).await.unwrap();
                        assert_eq!(
                            serde_json::to_value(view.fee_knowledge).unwrap()["state"],
                            "unknown_provenance",
                        );
                        assert!(view.pending_fees.is_none());
                    } else {
                        assert_eq!(recovered.generation, 0);
                        assert!(recovered.lease_token.is_none() && recovered.result.is_none());
                        assert_eq!(
                            recovered.error_code.as_deref(),
                            Some(match fault {
                                "redacted" => "source_revoked",
                                "missing" => "not_found",
                                _ => "invalid_input",
                            }),
                        );
                    }
                    let mut s = fx.store.session().await.unwrap();
                    assert_eq!(
                        s.capacity_usage_v41(now().unwrap() as u64)
                            .await
                            .unwrap()
                            .concurrent_dispatches,
                        1,
                    );
                    s.commit().await.unwrap();
                }
                pool.close().await;
                assert_eq!(
                    IndependentEvaluationControl::registration_progress(&fx.ctx, &fx.store, cid, hid)
                        .await
                        .unwrap(),
                    progress,
                );
            }
        });
        // These explicit history/never-started fixtures own no consumer I/O;
        // stop only their waiting reconciliation supervisors before returning.
        drop(runtime);
    })
    .await
    .unwrap();
    let (fx, claimed, draining) = tokio::task::spawn_blocking(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(async {
            let fx = paid_fixture(1).await;
            let claimed = paid_start(&fx).await;
            fx.dispatcher.cancel(&fx.admin, &claimed.id).await.unwrap();
            fx.gate.release.add_permits(1);
            let draining = paid_state(&fx, &claimed.id, ManagementJobState::Draining).await;
            (fx, claimed, draining)
        });
        // Actual model I/O already returned; ending its dedicated runtime now
        // drops only the waiting supervisor. This is a local recovery fixture,
        // not native kill/restart acceptance.
        drop(runtime);
        result
    })
    .await
    .unwrap();
    let before = fx.gate.calls.load(Ordering::SeqCst);
    let stored = raw_record(&fx.store, &fx.admin, &claimed.id).await;
    // Recovery must not call execute_request for an already draining record.
    let restart = ManagementDispatcher::new(fx.store.clone(), vec![fx.admin.clone()]).unwrap();
    restart.recover_pending().await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(fx.gate.calls.load(Ordering::SeqCst), before);
    assert_eq!(
        current(restart.status(&fx.admin, &claimed.id).await.unwrap()).generation,
        draining.generation
    );
    assert_eq!(raw_record(&fx.store, &fx.admin, &claimed.id).await, stored);
    reconcile(&fx).await;
    let final_job = paid_state(&fx, &claimed.id, ManagementJobState::Blocked).await;
    assert_eq!(
        final_job.result.map(|x| serde_json::to_value(x).unwrap()),
        draining.result.map(|x| serde_json::to_value(x).unwrap())
    );
    assert_eq!(fx.gate.calls.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn partial_with_unknown_pending_stays_draining_before_blocked() {
    let fx = paid_fixture(0).await;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", fx.path.display()))
        .await
        .unwrap();
    sqlx::query("CREATE TABLE owned_compile_fault(original_body TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER lose_one_owned_call AFTER UPDATE OF body ON objects WHEN NEW.kind='job' AND NOT EXISTS(SELECT 1 FROM owned_compile_fault) AND EXISTS(SELECT 1 FROM json_each(NEW.body,'$.optimization.stages') s WHERE json_extract(s.value,'$.kind')='edit_compiled') BEGIN INSERT INTO owned_compile_fault VALUES(NEW.body); UPDATE objects SET body=json_remove(json_set(NEW.body,'$.cancel_requested',json('true')),'$.optimization.calls[0]') WHERE namespace=NEW.namespace AND kind=NEW.kind AND id=NEW.id; END").execute(&pool).await.unwrap();
    let accepted = fx
        .dispatcher
        .submit(&fx.admin, "optimization.step", fx.request.clone())
        .await
        .unwrap();
    let id = accepted.identity().id.to_owned();
    let pending = paid_state(&fx, &id, ManagementJobState::Draining).await;
    assert!(
        pending
            .optimization
            .as_ref()
            .unwrap()
            .stages
            .iter()
            .any(|s| s.kind == "edit_compiled")
    );
    let status = fx.dispatcher.status_view(&fx.admin, &id).await.unwrap();
    assert_eq!(
        serde_json::to_value(status.fee_knowledge).unwrap()["state"],
        "unknown_provenance"
    );
    assert!(status.pending_fees.is_none());
    let original: String = sqlx::query_scalar("SELECT original_body FROM owned_compile_fault")
        .fetch_one(&pool)
        .await
        .unwrap();
    let original: Value = serde_json::from_str(&original).unwrap();
    let mut restored: Value =
        serde_json::from_str(&raw_record(&fx.store, &fx.admin, &id).await).unwrap();
    restored["optimization"]["calls"] = original["optimization"]["calls"].clone();
    let mut s = fx.store.session().await.unwrap();
    s.put(&fx.admin, "job", &id, fx.admin.actor(), &restored)
        .await
        .unwrap();
    s.commit().await.unwrap();
    pool.close().await;
    let final_job = paid_state(&fx, &id, ManagementJobState::Blocked).await;
    assert_eq!(
        final_job.error_code.as_deref(),
        Some("blocked_partial_commit")
    );
    assert_eq!(
        final_job.cancel_arrival,
        Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
    );
    let calls = call_rows(&fx).await;
    assert!(!calls.is_empty());
    assert!(
        calls
            .iter()
            .all(|c| c.actual_cost_micros == Some(10) && c.execution_closed)
    );
}

#[tokio::test]
async fn missing_or_foreign_fee_binding_never_becomes_known_none() {
    for (redact, fault) in [(false, "missing"), (true, "foreign"), (false, "digest")] {
        let fx = paid_fixture(1).await;
        let claimed = paid_start(&fx).await;
        let mut altered = claimed.clone();
        match fault {
            "foreign" => {
                altered.optimization.as_mut().unwrap().calls[0].producer_actor =
                    "foreign-producer".into()
            }
            "digest" => {
                altered.optimization.as_mut().unwrap().calls[0].budget_ref_digest =
                    hash(b"different-original-budget-ref")
            }
            _ => altered.optimization.as_mut().unwrap().calls.clear(),
        }
        let mut s = fx.store.session().await.unwrap();
        s.put(&fx.admin, "job", &claimed.id, fx.admin.actor(), &altered)
            .await
            .unwrap();
        s.commit().await.unwrap();
        let view = fx
            .dispatcher
            .status_view(&fx.admin, &claimed.id)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(view.fee_knowledge).unwrap()["state"],
            "unknown_provenance"
        );
        assert!(view.pending_fees.is_none());
        let mut s = fx.store.session().await.unwrap();
        s.put(&fx.admin, "job", &claimed.id, fx.admin.actor(), &claimed)
            .await
            .unwrap();
        s.commit().await.unwrap();
        fx.dispatcher.cancel(&fx.admin, &claimed.id).await.unwrap();
        fx.gate.release.add_permits(1);
        let pending = paid_state(&fx, &claimed.id, ManagementJobState::Draining).await;
        let calls = call_rows(&fx).await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].state, BudgetCallState::Uncertain);
        if redact {
            let mut c = LifecycleStore::begin_revoke(
                &fx.admin,
                &fx.store,
                TypedObjectRef {
                    kind: "run".into(),
                    id: "run-failure".into(),
                },
                "after real paid dispatch",
                now().unwrap(),
            )
            .await
            .unwrap();
            for _ in 0..100 {
                c = LifecycleStore::cleanup_step(
                    &fx.admin,
                    &fx.store,
                    &c.job_id,
                    8,
                    now().unwrap(),
                )
                .await
                .unwrap();
                if c.state == CleanupState::Complete {
                    break;
                }
            }
            assert_eq!(c.state, CleanupState::Complete);
        }
        reconcile(&fx).await;
        let done = paid_state(&fx, &claimed.id, ManagementJobState::Blocked).await;
        assert_eq!(done.error_code, pending.error_code);
        assert_eq!(done.step, pending.step);
        assert_eq!(fx.gate.calls.load(Ordering::SeqCst), 1);
        let calls = call_rows(&fx).await;
        assert_eq!(calls[0].actual_cost_micros, Some(10));
        assert!(calls[0].execution_closed);
        let root = fx
            .store
            .root_budget(&fx.admin, "management-scope")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(root.spent_micros, 10);
        assert_eq!(root.reserved_micros, 0);
    }
}

#[tokio::test]
async fn background_exit_closes_from_real_progress_within_bound() {
    for long_step in [true, false] {
        let fx = paid_fixture(if long_step { 1 } else { 2 }).await;
        let claimed = paid_start(&fx).await;
        if long_step {
            tokio::time::sleep(Duration::from_secs(65)).await;
            let alive = current(fx.dispatcher.status(&fx.admin, &claimed.id).await.unwrap());
            assert_eq!(alive.state, ManagementJobState::Running);
            assert_eq!(alive.generation, claimed.generation);
            assert_eq!(alive.lease_token, claimed.lease_token);
            assert!(alive.lease_until > now().unwrap());
            assert!(alive.lease_until > claimed.lease_until);
        }
        let exited = Instant::now();
        fx.gate.release.add_permits(1);
        let pending = paid_state(&fx, &claimed.id, ManagementJobState::Draining).await;
        assert!(exited.elapsed() <= Duration::from_secs(60));
        assert_eq!(pending.generation, claimed.generation);
        reconcile(&fx).await;
        let expected = if long_step {
            ManagementJobState::Blocked
        } else {
            ManagementJobState::Failed
        };
        let done = paid_state(&fx, &claimed.id, expected).await;
        assert_eq!(done.generation, claimed.generation);
        assert_eq!(fx.gate.calls.load(Ordering::SeqCst), 1);
        assert!(
            call_rows(&fx)
                .await
                .iter()
                .all(|c| c.actual_cost_micros == Some(10) && c.execution_closed)
        );
    }
}

fn full_image(path: &std::path::Path) -> serde_json::Value {
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
    let output = std::process::Command::new("python3")
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
