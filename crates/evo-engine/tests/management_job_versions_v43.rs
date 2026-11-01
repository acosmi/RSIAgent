//! AG155 mutable-job v1/v2 compatibility. Historical strings below are exact
//! qualified producer rows; the local fixture restores selected rows only,
//! not a native database image or Current authority.
use evo_core::{Context, Error, Role};
use evo_engine::dispatch::{ManagementDispatcher, ManagementJobState, VersionedManagementJob};
use evo_storage::{Store, decode_management_job, sqlx};
use serde_json::{Value, json};

const LEGACY: &str = r###"{"world_terminal_suffix":{"namespace":"n","actor":"admin","role":"admin","job_id":"management-job-3ce1459b090ee3ab44e071cf","input_id":"management-input-3ce1459b090ee3ab44e071cf","request":{"request_key":"ag155-capture-world","schema_version":"rsia.management.exploration_start.v1","world":{"approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","caps":{"max_depth":4,"max_nodes":12,"max_repair_dispatches_per_episode":1,"schema_version":"rsia.exploration_caps.v1","w_online":1},"context_signature":"5370337add2265337fd9b30a572fd87af8ae81dcdf0d86ca69fb99a5d1ac2665","current_branch_focus_actions":0,"current_branch_seq":null,"decision_round":0,"dependencies":[{"id":"run-failure","kind":"run"},{"id":"run-success","kind":"run"}],"dispatch_ids":[],"environment_digest":"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d","grader_digest":"cb8ef6444b43f18fedf1f8347c5cd667f442473878cc1c9547f74564ed6f865d","history_ids":[],"id":"ag155-capture-world","initial_baseline_quality_micros":500000,"model_digest":"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4","node_ids":[],"parent_bundle_digest":"a251f2c86e5b625537281fa11f41cfc88802d13d8be50d8af71bc63c045ea3cd","parent_skill_digest":"a8cf8c36ddeff7527a3e261587c39ab61b9347aa5f11f7e45c1f57a66778a8b3","policy":{"fairness_wait_rounds":4,"max_focus_actions":2,"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2},"remaining_recovery_dispatches":2,"remaining_root_micros":1000,"root_opportunities":[{"action_seq":2,"branch_seq":2,"estimated_cost_upper_micros":10,"root_slot":2},{"action_seq":1,"branch_seq":1,"estimated_cost_upper_micros":10,"root_slot":1}],"rules_digest":"6c621d1a05138a7888d37d9269a9da8e2e11e4aced2f6cfd24b05ab1b9e61bb0","schema_version":"rsia.exploration_world.v1","simulation":{"online":{"fixed_seed":7}},"source_watermark":1,"state":"collecting","successor_cost_upper_micros":10,"tools_digest":"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281","waits":[]}},"objects":[{"namespace":"n","kind":"artifact","id":"management-input-3ce1459b090ee3ab44e071cf","owner":"admin","body":"{\"id\":\"management-input-3ce1459b090ee3ab44e071cf\",\"schema_version\":\"rsia.management_private_input.v1\",\"operation\":\"exploration.start\",\"payload_digest\":\"972e83d33d7b6d5cd497f2bab921f434a8b6a3fef9a4834331680199d2470b52\",\"payload\":{\"operation\":\"exploration_start\",\"request_key\":\"ag155-capture-world\",\"schema_version\":\"rsia.management.exploration_start.v1\",\"world\":{\"approved_parent_digest\":\"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58\",\"caps\":{\"max_depth\":4,\"max_nodes\":12,\"max_repair_dispatches_per_episode\":1,\"schema_version\":\"rsia.exploration_caps.v1\",\"w_online\":1},\"context_signature\":\"5370337add2265337fd9b30a572fd87af8ae81dcdf0d86ca69fb99a5d1ac2665\",\"current_branch_focus_actions\":0,\"current_branch_seq\":null,\"decision_round\":0,\"dependencies\":[{\"id\":\"run-failure\",\"kind\":\"run\"},{\"id\":\"run-success\",\"kind\":\"run\"}],\"dispatch_ids\":[],\"environment_digest\":\"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d\",\"grader_digest\":\"cb8ef6444b43f18fedf1f8347c5cd667f442473878cc1c9547f74564ed6f865d\",\"history_ids\":[],\"id\":\"ag155-capture-world\",\"initial_baseline_quality_micros\":500000,\"model_digest\":\"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4\",\"node_ids\":[],\"parent_bundle_digest\":\"a251f2c86e5b625537281fa11f41cfc88802d13d8be50d8af71bc63c045ea3cd\",\"parent_skill_digest\":\"a8cf8c36ddeff7527a3e261587c39ab61b9347aa5f11f7e45c1f57a66778a8b3\",\"policy\":{\"fairness_wait_rounds\":4,\"max_focus_actions\":2,\"schema_version\":\"rsia.elastic_priority.v1\",\"significant_gain_micros\":20000,\"stagnation_abs_gain_micros\":5000,\"stagnation_window\":2},\"remaining_recovery_dispatches\":2,\"remaining_root_micros\":1000,\"root_opportunities\":[{\"action_seq\":2,\"branch_seq\":2,\"estimated_cost_upper_micros\":10,\"root_slot\":2},{\"action_seq\":1,\"branch_seq\":1,\"estimated_cost_upper_micros\":10,\"root_slot\":1}],\"rules_digest\":\"6c621d1a05138a7888d37d9269a9da8e2e11e4aced2f6cfd24b05ab1b9e61bb0\",\"schema_version\":\"rsia.exploration_world.v1\",\"simulation\":{\"online\":{\"fixed_seed\":7}},\"source_watermark\":1,\"state\":\"collecting\",\"successor_cost_upper_micros\":10,\"tools_digest\":\"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281\",\"waits\":[]}}}","revision":1},{"namespace":"n","kind":"job","id":"management-job-3ce1459b090ee3ab44e071cf","owner":"admin","body":"{\"id\":\"management-job-3ce1459b090ee3ab44e071cf\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"exploration.start\",\"request_key\":\"ag155-capture-world\",\"payload_digest\":\"972e83d33d7b6d5cd497f2bab921f434a8b6a3fef9a4834331680199d2470b52\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"succeeded\",\"step\":\"exploration_started:cancel_after_side_effect\",\"private_input_ref\":\"management-input-3ce1459b090ee3ab44e071cf\",\"result\":{\"result\":\"exploration_started\",\"world_id\":\"ag155-capture-world\",\"context_signature\":\"5370337add2265337fd9b30a572fd87af8ae81dcdf0d86ca69fb99a5d1ac2665\",\"prefix_digest\":\"4a489b56ff0ebcd3df831e27cc78c048c449396746ba7d424efef4b357fc0dcc\",\"legal_actions_digest\":\"0f8eebdaca13c2e768864211215051e4ef4ce329f238828b293af86e5d82763c\",\"policy_digest\":\"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423\",\"caps_digest\":\"81f3bf209b41d41276ab87a00fa7fd30e9fdf3792ed08b27174c0f27a0cdf67f\",\"action\":{\"decision\":\"dispatch\",\"action_ids\":[\"root-1\"],\"action_seqs\":[1],\"reason\":\"first_preregistered_root\",\"estimated_cost_upper_micros\":10}},\"error_code\":null,\"cancel_requested\":true,\"lease_token\":null,\"lease_until\":0,\"generation\":1,\"diagnostics\":[],\"created_at\":1793306575}","revision":5}],"cache":[{"namespace":"n","actor":"admin","operation":"exploration.start","request_key":"ag155-capture-world","payload_hash":"972e83d33d7b6d5cd497f2bab921f434a8b6a3fef9a4834331680199d2470b52","subject_id":"management-job-3ce1459b090ee3ab44e071cf","response":"{\"id\":\"management-job-3ce1459b090ee3ab44e071cf\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"exploration.start\",\"request_key\":\"ag155-capture-world\",\"payload_digest\":\"972e83d33d7b6d5cd497f2bab921f434a8b6a3fef9a4834331680199d2470b52\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"queued\",\"step\":\"accepted\",\"private_input_ref\":\"management-input-3ce1459b090ee3ab44e071cf\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":null,\"lease_until\":0,\"generation\":0,\"diagnostics\":[],\"created_at\":1793306575}","redacted":0}]},"queued":{"namespace":"n","actor":"admin","role":"admin","job_id":"management-job-00afd430bef6dd4b64430af5","input_id":"management-input-00afd430bef6dd4b64430af5","request":{"request_key":"ag155-capture-meta","schema_version":"rsia.management.meta_start.v1"},"objects":[{"namespace":"n","kind":"artifact","id":"management-input-00afd430bef6dd4b64430af5","owner":"admin","body":"{\"id\":\"management-input-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_private_input.v1\",\"operation\":\"meta.start\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"payload\":{\"operation\":\"blocked\",\"request_key\":\"ag155-capture-meta\",\"schema_version\":\"rsia.management.meta_start.v1\"}}","revision":1},{"namespace":"n","kind":"job","id":"management-job-00afd430bef6dd4b64430af5","owner":"admin","body":"{\"id\":\"management-job-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"meta.start\",\"request_key\":\"ag155-capture-meta\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"queued\",\"step\":\"accepted\",\"private_input_ref\":\"management-input-00afd430bef6dd4b64430af5\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":null,\"lease_until\":0,\"generation\":0,\"diagnostics\":[],\"created_at\":1793306574}","revision":1}],"cache":[{"namespace":"n","actor":"admin","operation":"meta.start","request_key":"ag155-capture-meta","payload_hash":"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242","subject_id":"management-job-00afd430bef6dd4b64430af5","response":"{\"id\":\"management-job-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"meta.start\",\"request_key\":\"ag155-capture-meta\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"queued\",\"step\":\"accepted\",\"private_input_ref\":\"management-input-00afd430bef6dd4b64430af5\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":null,\"lease_until\":0,\"generation\":0,\"diagnostics\":[],\"created_at\":1793306574}","redacted":0}]},"running":{"namespace":"n","actor":"admin","role":"admin","job_id":"management-job-00afd430bef6dd4b64430af5","input_id":"management-input-00afd430bef6dd4b64430af5","request":{"request_key":"ag155-capture-meta","schema_version":"rsia.management.meta_start.v1"},"objects":[{"namespace":"n","kind":"artifact","id":"management-input-00afd430bef6dd4b64430af5","owner":"admin","body":"{\"id\":\"management-input-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_private_input.v1\",\"operation\":\"meta.start\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"payload\":{\"operation\":\"blocked\",\"request_key\":\"ag155-capture-meta\",\"schema_version\":\"rsia.management.meta_start.v1\"}}","revision":1},{"namespace":"n","kind":"job","id":"management-job-00afd430bef6dd4b64430af5","owner":"admin","body":"{\"id\":\"management-job-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"meta.start\",\"request_key\":\"ag155-capture-meta\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"running\",\"step\":\"claimed\",\"private_input_ref\":\"management-input-00afd430bef6dd4b64430af5\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":\"fbd1eb475a4795774e9fbdd927aa81c47dab87b4ffda41fc3d898af871d175af\",\"lease_until\":1793306634,\"generation\":1,\"diagnostics\":[],\"created_at\":1793306574}","revision":2}],"cache":[{"namespace":"n","actor":"admin","operation":"meta.start","request_key":"ag155-capture-meta","payload_hash":"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242","subject_id":"management-job-00afd430bef6dd4b64430af5","response":"{\"id\":\"management-job-00afd430bef6dd4b64430af5\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"meta.start\",\"request_key\":\"ag155-capture-meta\",\"payload_digest\":\"c51b53dfa977533870649ff77ebee74378aa647c782d3274fffb24496198d242\",\"owner_actor\":\"admin\",\"owner_role\":\"admin\",\"state\":\"queued\",\"step\":\"accepted\",\"private_input_ref\":\"management-input-00afd430bef6dd4b64430af5\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":null,\"lease_until\":0,\"generation\":0,\"diagnostics\":[],\"created_at\":1793306574}","redacted":0}]},"control_committed_holdout_not_committed":{"namespace":"n","actor":"evaluator","role":"evaluator","job_id":"management-job-ae10c9ec67325b06cea1ff97","input_id":"management-input-ae10c9ec67325b06cea1ff97","request":{"control":{"alpha_plan":{"allocations":[{"alpha":"0.04","attempt_id":"attempt-1","claim_id":"fixed-gain","kind":"fixed_sample_gain"},{"alpha":"0.01","attempt_id":"attempt-1","claim_id":"early-reject","kind":"sequential_reject"}],"alpha_total":"0.05","research_family_id":"family-e05","schema_version":"rsia.research_family_alpha_plan.v1"},"anchor_matrix":{"critical_capabilities":[{"anchor_ids":["anchor-1"],"capability_id":"safety"}],"profile_id":"quality-gain","schema_version":"rsia.anchor_coverage_matrix.v1","version":"anchors-v1"},"approver_actor":"approver","attempt_id":"attempt-1","billing_scope":"billing-1","created_at_unix_seconds":2,"early_stop_plan":null,"evaluator_actor":"evaluator","evidence_scope":"program_fixture","executor_actor":"executor","fixed_grader":{"method":"exact_json_answer_v1","schema_version":"rsia.fixed_grader.v1","version":"exact-json-v1"},"fixed_sample_claim_id":"fixed-gain","formal_plan":{"alpha_plan_digest":"b4c896cd5e152cdcd465cb0c3739601a95b6b99a6dd10534bbab13196e0c9e0d","anchor_coverage_digest":"98744fd878573309c1dee0b58fe926e814adb024a48281c303de833650ef021b","execution_authority":"not_issued","n_planned":2,"optimization_budget_digest":"95f5e72267264a4f42a4071c13303db9660b3d813a0e087f7504b295d263cdb6","optimizer_comparison_digest":"f5a65644332761fde1c427b9ed687748f2bc2c58c866b80d8d621eeaacec4e57","prefix_selection":"frozen_continuous_order","preregistration_receipt_digest":"3e8ac5236ba3cb5c5018eb5dda6f156d02eef753bf2420bc73593d861365d05e","profile":"quality_gain","research_family_id":"family-e05","schema_version":"rsia.formal_experiment_plan.v4_1","source_sampling_digest":"52630e4131f9862ea34c8ded9741fae2176c56c55c74827b2bd30cd1ef9eb37b","statistical_assumptions_digest":"50b8c3d1ac87e1da574e773ec4a396a4c74213407d7845ab17672f4cf2ad30b2","statistical_unit":"independent_task_cluster","v1_plan_snapshot_digest":"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1","weighting":"equal_units"},"grader_digest":"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a","id":"control-1","optimization_budget":{"admin_authorization_receipt_digest":"ec70f47b06c5879902c6546b934c89514f6b315fb04d20d94e9ebe9fcb6c7ce9","payer_id":"payer","root_budget_scope_id":"billing-1","root_total":{"amount":"0.001","currency":"USD","pricing_version":"pricing-v1"},"schema_version":"rsia.optimization_budget_plan.v1","stages":[{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"history_collection"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"reflection"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"merge"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"ranking"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"development_execution"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"development_scoring"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"practice"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"consolidation"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"guidance"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"human_review"},{"experiment_total":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"per_call_limit":{"amount":"0","currency":"USD","pricing_version":"pricing-v1"},"stage":"storage_cpu"}]},"optimizer_comparison":{"c_simple":{"arm":"c_simple","authorized_materials_digest":"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88","base_model_digest":"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4","budget_limit":{"amount":"0.001","currency":"USD","pricing_version":"pricing-v1"},"context_limit_tokens":4096,"implementation_digest":"a7a39b72f29718e653e73503210fbb597057b7a1c77d1fe321a1afcff041d4e1","initial_s0_digest":"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513","root_budget_scope_id":"billing-1","task_partition_digest":"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254","tools_digest":"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281"},"c_skillopt":{"arm":"c_skillopt","authorized_materials_digest":"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88","base_model_digest":"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4","budget_limit":{"amount":"0.001","currency":"USD","pricing_version":"pricing-v1"},"context_limit_tokens":4096,"implementation_digest":"d97dddf1b4635724f6d5eea620a2679bd7667da5dd6a3b3b9a2245852ea1c33a","initial_s0_digest":"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513","root_budget_scope_id":"billing-1","task_partition_digest":"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254","tools_digest":"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281"},"d_starts_from":"c_skillopt","schema_version":"rsia.optimizer_comparison_contract.v1"},"oracle_digest":"9297ea092eae8ad974be8d2c9d294283d0e5aeed5a3e34886dfb954dcf1ac6e3","proposer_actor":"proposer","schema_version":"rsia.registered_evaluation_control.v2","v1_plan_snapshot":{"alpha_total":0.05,"candidate_digest":null,"conditions":["frozen_a","static_harness_b1","fixed_improver_c"],"dataset_epoch":"dev_pilot_unfunded","first_round":true,"frozen":true,"frozen_at":1,"id":"formal-e05","max_cost_ratio":1.1,"max_p95_latency_ratio":1.2,"min_effect":0.02,"monetary_budget":{"amount":"0.001","currency":"USD","pricing_version":"pricing-v1"},"n_planned":2,"noninferior_bound":-0.01,"objective_version":"rsia.attainment_auc.v1","profile":"quality_gain","query_limit":3,"savings_ratio":0.1,"schema_version":"rsia.experiment_plan.v1","stats_version":"rsia.empirical_bernstein.v2","stop_on_harm":true}},"holdout":{"anchor_inputs":[{"cluster_or_anchor_id":"anchor-1","input_artifact_id":"input-a0","input_digest":"b9805f3bf0893bf6519130720c2e7d21f85d4eb30d611a00512953d9ec93e459","target_id":"a0"}],"baseline_bundle_digest":"42d582727e80985cc3d2140697d6cd801fb0e79deea844f4fb3a48ea4469b2ae","candidate_bundle_digest":"f0f5e9a60c5fbf07649b2e866d97133a74f613e613759fdcaf645cabd0a8d3e1","id":"holdout-1","manifest":{"alpha_plan_digest":"b4c896cd5e152cdcd465cb0c3739601a95b6b99a6dd10534bbab13196e0c9e0d","anchor_coverage_digest":"98744fd878573309c1dee0b58fe926e814adb024a48281c303de833650ef021b","candidate_pair_digest":"ce7d106a252e85ee6fd0da0bba6613ba7b500690fc75e36edd80585ad02d751f","environment_digest":"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d","epoch_id":"epoch-1","grader_digest":"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a","issued_at_unix_ms":4,"oracle_version":"9297ea092eae8ad974be8d2c9d294283d0e5aeed5a3e34886dfb954dcf1ac6e3","ordered_task_cluster_ids":["cluster-0","cluster-1"],"private_seed_commitment":"075612c8ddff488680ca76418cd909f39954a7ce13390c9f1e5bae1c1d1307d4","research_family_id":"family-e05","sampler_version":"sampler-v1","schema_version":"rsia.dual_timescale_holdout.v1","sequence_commitment":"92c6d50a920a2eadade4aedcb47d8761d2436411147fbecabdcbcffcf1939e32","sequential_mode":{"mode":"disabled"},"slice_id":"slice-1","source_closure_digests":["cf3d9e40a268fd493c714ed760a8a7fbc85aeba6f0cbf13c6f539d5b5bc03fc8","e371df7ae3fa5d8c47400dd643ff7f10232089df78f7bf21c3bc21a8e2e05c26"],"v1_plan_snapshot_digest":"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1"},"oracle_entries":[{"expected_answer_json":"\"ok\"","target_id":"r0"},{"expected_answer_json":"\"ok\"","target_id":"r1"},{"expected_answer_json":"\"ok\"","target_id":"a0"}],"oracle_payload_digest":"fd25ac3b5815657ea841251905e5e1aa6132b547cb2c6d148e7e2ea73560fe4a","pair":{"baseline_digest":"09fc333c79554abaf2b12d0aa64c34c1aa4f9dd2fe552f2bc18d71e8a86a77d7","candidate_digest":"dda18a0e21ae47c53b4309434cbc02ae8bf764fa83a6defbb719431242722aa7","frozen_at_unix_ms":3,"schema_version":"rsia.frozen_candidate_pair.v1","v1_plan_snapshot_digest":"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1"},"registration_id":"control-1","rotating_inputs":[{"cluster_or_anchor_id":"cluster-0","input_artifact_id":"input-r0","input_digest":"50ebdd6498f1d21eda818311e14b64d456ba12479633c68bafd0308a41dfff8d","target_id":"r0"},{"cluster_or_anchor_id":"cluster-1","input_artifact_id":"input-r1","input_digest":"79f20cb26ce6973c742cf84981d1eb180a330de4f174bc83de8b8397abc0c779","target_id":"r1"}],"schema_version":"rsia.protected_holdout_record.v1"},"request_key":"ag155-capture-registration","schema_version":"rsia.management.experiment_register.v1"},"objects":[{"namespace":"n","kind":"artifact","id":"management-input-ae10c9ec67325b06cea1ff97","owner":"evaluator","body":"{\"id\":\"management-input-ae10c9ec67325b06cea1ff97\",\"schema_version\":\"rsia.management_private_input.v1\",\"operation\":\"experiment.register\",\"payload_digest\":\"d22f58f331a893f330c9280c5a8570ebaea912a807678a2bbadbf5c1124aebc6\",\"payload\":{\"control\":{\"alpha_plan\":{\"allocations\":[{\"alpha\":\"0.04\",\"attempt_id\":\"attempt-1\",\"claim_id\":\"fixed-gain\",\"kind\":\"fixed_sample_gain\"},{\"alpha\":\"0.01\",\"attempt_id\":\"attempt-1\",\"claim_id\":\"early-reject\",\"kind\":\"sequential_reject\"}],\"alpha_total\":\"0.05\",\"research_family_id\":\"family-e05\",\"schema_version\":\"rsia.research_family_alpha_plan.v1\"},\"anchor_matrix\":{\"critical_capabilities\":[{\"anchor_ids\":[\"anchor-1\"],\"capability_id\":\"safety\"}],\"profile_id\":\"quality-gain\",\"schema_version\":\"rsia.anchor_coverage_matrix.v1\",\"version\":\"anchors-v1\"},\"approver_actor\":\"approver\",\"attempt_id\":\"attempt-1\",\"billing_scope\":\"billing-1\",\"created_at_unix_seconds\":2,\"early_stop_plan\":null,\"evaluator_actor\":\"evaluator\",\"evidence_scope\":\"program_fixture\",\"executor_actor\":\"executor\",\"fixed_grader\":{\"method\":\"exact_json_answer_v1\",\"schema_version\":\"rsia.fixed_grader.v1\",\"version\":\"exact-json-v1\"},\"fixed_sample_claim_id\":\"fixed-gain\",\"formal_plan\":{\"alpha_plan_digest\":\"b4c896cd5e152cdcd465cb0c3739601a95b6b99a6dd10534bbab13196e0c9e0d\",\"anchor_coverage_digest\":\"98744fd878573309c1dee0b58fe926e814adb024a48281c303de833650ef021b\",\"execution_authority\":\"not_issued\",\"n_planned\":2,\"optimization_budget_digest\":\"95f5e72267264a4f42a4071c13303db9660b3d813a0e087f7504b295d263cdb6\",\"optimizer_comparison_digest\":\"f5a65644332761fde1c427b9ed687748f2bc2c58c866b80d8d621eeaacec4e57\",\"prefix_selection\":\"frozen_continuous_order\",\"preregistration_receipt_digest\":\"3e8ac5236ba3cb5c5018eb5dda6f156d02eef753bf2420bc73593d861365d05e\",\"profile\":\"quality_gain\",\"research_family_id\":\"family-e05\",\"schema_version\":\"rsia.formal_experiment_plan.v4_1\",\"source_sampling_digest\":\"52630e4131f9862ea34c8ded9741fae2176c56c55c74827b2bd30cd1ef9eb37b\",\"statistical_assumptions_digest\":\"50b8c3d1ac87e1da574e773ec4a396a4c74213407d7845ab17672f4cf2ad30b2\",\"statistical_unit\":\"independent_task_cluster\",\"v1_plan_snapshot_digest\":\"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1\",\"weighting\":\"equal_units\"},\"grader_digest\":\"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a\",\"id\":\"control-1\",\"optimization_budget\":{\"admin_authorization_receipt_digest\":\"ec70f47b06c5879902c6546b934c89514f6b315fb04d20d94e9ebe9fcb6c7ce9\",\"payer_id\":\"payer\",\"root_budget_scope_id\":\"billing-1\",\"root_total\":{\"amount\":\"0.001\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"schema_version\":\"rsia.optimization_budget_plan.v1\",\"stages\":[{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"history_collection\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"reflection\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"merge\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"ranking\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"development_execution\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"development_scoring\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"practice\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"consolidation\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"guidance\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"human_review\"},{\"experiment_total\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"per_call_limit\":{\"amount\":\"0\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"stage\":\"storage_cpu\"}]},\"optimizer_comparison\":{\"c_simple\":{\"arm\":\"c_simple\",\"authorized_materials_digest\":\"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88\",\"base_model_digest\":\"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4\",\"budget_limit\":{\"amount\":\"0.001\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"context_limit_tokens\":4096,\"implementation_digest\":\"a7a39b72f29718e653e73503210fbb597057b7a1c77d1fe321a1afcff041d4e1\",\"initial_s0_digest\":\"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513\",\"root_budget_scope_id\":\"billing-1\",\"task_partition_digest\":\"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254\",\"tools_digest\":\"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281\"},\"c_skillopt\":{\"arm\":\"c_skillopt\",\"authorized_materials_digest\":\"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88\",\"base_model_digest\":\"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4\",\"budget_limit\":{\"amount\":\"0.001\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"context_limit_tokens\":4096,\"implementation_digest\":\"d97dddf1b4635724f6d5eea620a2679bd7667da5dd6a3b3b9a2245852ea1c33a\",\"initial_s0_digest\":\"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513\",\"root_budget_scope_id\":\"billing-1\",\"task_partition_digest\":\"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254\",\"tools_digest\":\"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281\"},\"d_starts_from\":\"c_skillopt\",\"schema_version\":\"rsia.optimizer_comparison_contract.v1\"},\"oracle_digest\":\"9297ea092eae8ad974be8d2c9d294283d0e5aeed5a3e34886dfb954dcf1ac6e3\",\"proposer_actor\":\"proposer\",\"schema_version\":\"rsia.registered_evaluation_control.v2\",\"v1_plan_snapshot\":{\"alpha_total\":0.05,\"candidate_digest\":null,\"conditions\":[\"frozen_a\",\"static_harness_b1\",\"fixed_improver_c\"],\"dataset_epoch\":\"dev_pilot_unfunded\",\"first_round\":true,\"frozen\":true,\"frozen_at\":1,\"id\":\"formal-e05\",\"max_cost_ratio\":1.1,\"max_p95_latency_ratio\":1.2,\"min_effect\":0.02,\"monetary_budget\":{\"amount\":\"0.001\",\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\"},\"n_planned\":2,\"noninferior_bound\":-0.01,\"objective_version\":\"rsia.attainment_auc.v1\",\"profile\":\"quality_gain\",\"query_limit\":3,\"savings_ratio\":0.1,\"schema_version\":\"rsia.experiment_plan.v1\",\"stats_version\":\"rsia.empirical_bernstein.v2\",\"stop_on_harm\":true}},\"holdout\":{\"anchor_inputs\":[{\"cluster_or_anchor_id\":\"anchor-1\",\"input_artifact_id\":\"input-a0\",\"input_digest\":\"b9805f3bf0893bf6519130720c2e7d21f85d4eb30d611a00512953d9ec93e459\",\"target_id\":\"a0\"}],\"baseline_bundle_digest\":\"42d582727e80985cc3d2140697d6cd801fb0e79deea844f4fb3a48ea4469b2ae\",\"candidate_bundle_digest\":\"f0f5e9a60c5fbf07649b2e866d97133a74f613e613759fdcaf645cabd0a8d3e1\",\"id\":\"holdout-1\",\"manifest\":{\"alpha_plan_digest\":\"b4c896cd5e152cdcd465cb0c3739601a95b6b99a6dd10534bbab13196e0c9e0d\",\"anchor_coverage_digest\":\"98744fd878573309c1dee0b58fe926e814adb024a48281c303de833650ef021b\",\"candidate_pair_digest\":\"ce7d106a252e85ee6fd0da0bba6613ba7b500690fc75e36edd80585ad02d751f\",\"environment_digest\":\"ba5285161ba6eed0085fb13784ce5c92f70ebc268b94fd66aa1d68a32884204d\",\"epoch_id\":\"epoch-1\",\"grader_digest\":\"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a\",\"issued_at_unix_ms\":4,\"oracle_version\":\"9297ea092eae8ad974be8d2c9d294283d0e5aeed5a3e34886dfb954dcf1ac6e3\",\"ordered_task_cluster_ids\":[\"cluster-0\",\"cluster-1\"],\"private_seed_commitment\":\"075612c8ddff488680ca76418cd909f39954a7ce13390c9f1e5bae1c1d1307d4\",\"research_family_id\":\"family-e05\",\"sampler_version\":\"sampler-v1\",\"schema_version\":\"rsia.dual_timescale_holdout.v1\",\"sequence_commitment\":\"92c6d50a920a2eadade4aedcb47d8761d2436411147fbecabdcbcffcf1939e32\",\"sequential_mode\":{\"mode\":\"disabled\"},\"slice_id\":\"slice-1\",\"source_closure_digests\":[\"cf3d9e40a268fd493c714ed760a8a7fbc85aeba6f0cbf13c6f539d5b5bc03fc8\",\"e371df7ae3fa5d8c47400dd643ff7f10232089df78f7bf21c3bc21a8e2e05c26\"],\"v1_plan_snapshot_digest\":\"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1\"},\"oracle_entries\":[{\"expected_answer_json\":\"\\\"ok\\\"\",\"target_id\":\"r0\"},{\"expected_answer_json\":\"\\\"ok\\\"\",\"target_id\":\"r1\"},{\"expected_answer_json\":\"\\\"ok\\\"\",\"target_id\":\"a0\"}],\"oracle_payload_digest\":\"fd25ac3b5815657ea841251905e5e1aa6132b547cb2c6d148e7e2ea73560fe4a\",\"pair\":{\"baseline_digest\":\"09fc333c79554abaf2b12d0aa64c34c1aa4f9dd2fe552f2bc18d71e8a86a77d7\",\"candidate_digest\":\"dda18a0e21ae47c53b4309434cbc02ae8bf764fa83a6defbb719431242722aa7\",\"frozen_at_unix_ms\":3,\"schema_version\":\"rsia.frozen_candidate_pair.v1\",\"v1_plan_snapshot_digest\":\"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1\"},\"registration_id\":\"control-1\",\"rotating_inputs\":[{\"cluster_or_anchor_id\":\"cluster-0\",\"input_artifact_id\":\"input-r0\",\"input_digest\":\"50ebdd6498f1d21eda818311e14b64d456ba12479633c68bafd0308a41dfff8d\",\"target_id\":\"r0\"},{\"cluster_or_anchor_id\":\"cluster-1\",\"input_artifact_id\":\"input-r1\",\"input_digest\":\"79f20cb26ce6973c742cf84981d1eb180a330de4f174bc83de8b8397abc0c779\",\"target_id\":\"r1\"}],\"schema_version\":\"rsia.protected_holdout_record.v1\"},\"operation\":\"experiment_register\",\"request_key\":\"ag155-capture-registration\",\"schema_version\":\"rsia.management.experiment_register.v1\"}}","revision":1},{"namespace":"n","kind":"job","id":"management-job-ae10c9ec67325b06cea1ff97","owner":"evaluator","body":"{\"id\":\"management-job-ae10c9ec67325b06cea1ff97\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"experiment.register\",\"request_key\":\"ag155-capture-registration\",\"payload_digest\":\"d22f58f331a893f330c9280c5a8570ebaea912a807678a2bbadbf5c1124aebc6\",\"owner_actor\":\"evaluator\",\"owner_role\":\"evaluator\",\"state\":\"running\",\"step\":\"control_registered\",\"private_input_ref\":\"management-input-ae10c9ec67325b06cea1ff97\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":\"14dcb05f1c1b8ff1e55c12b53fd5c9ae80c0b8347a39930412db3ac0a1297bed\",\"lease_until\":1793306635,\"generation\":1,\"diagnostics\":[],\"created_at\":1793306575}","revision":3},{"namespace":"n","kind":"artifact","id":"e05-c08afb9d9cc84a6be5fb00e2f78986e0cef55ce1b808ec7c1c15464d26fa4724","owner":"evaluator","body":"{\"schema_version\":\"rsia.typed_artifact_envelope.v1\",\"id\":\"e05-c08afb9d9cc84a6be5fb00e2f78986e0cef55ce1b808ec7c1c15464d26fa4724\",\"record_kind\":\"registered_evaluation_control_v41\",\"payload\":{\"schema_version\":\"rsia.registered_evaluation_control.v2\",\"id\":\"control-1\",\"v1_plan_snapshot\":{\"schema_version\":\"rsia.experiment_plan.v1\",\"id\":\"formal-e05\",\"objective_version\":\"rsia.attainment_auc.v1\",\"stats_version\":\"rsia.empirical_bernstein.v2\",\"profile\":\"quality_gain\",\"conditions\":[\"frozen_a\",\"static_harness_b1\",\"fixed_improver_c\"],\"first_round\":true,\"min_effect\":0.02,\"noninferior_bound\":-0.01,\"savings_ratio\":0.1,\"max_cost_ratio\":1.1,\"max_p95_latency_ratio\":1.2,\"alpha_total\":0.05,\"n_planned\":2,\"query_limit\":3,\"monetary_budget\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0.001\"},\"stop_on_harm\":true,\"frozen\":true,\"frozen_at\":1,\"candidate_digest\":null,\"dataset_epoch\":\"dev_pilot_unfunded\"},\"formal_plan\":{\"schema_version\":\"rsia.formal_experiment_plan.v4_1\",\"v1_plan_snapshot_digest\":\"b75ff09d143239740c07008cc47c422c5927791e59afa7b088e583b8e9c2d5c1\",\"preregistration_receipt_digest\":\"3e8ac5236ba3cb5c5018eb5dda6f156d02eef753bf2420bc73593d861365d05e\",\"research_family_id\":\"family-e05\",\"alpha_plan_digest\":\"b4c896cd5e152cdcd465cb0c3739601a95b6b99a6dd10534bbab13196e0c9e0d\",\"optimizer_comparison_digest\":\"f5a65644332761fde1c427b9ed687748f2bc2c58c866b80d8d621eeaacec4e57\",\"optimization_budget_digest\":\"95f5e72267264a4f42a4071c13303db9660b3d813a0e087f7504b295d263cdb6\",\"anchor_coverage_digest\":\"98744fd878573309c1dee0b58fe926e814adb024a48281c303de833650ef021b\",\"source_sampling_digest\":\"52630e4131f9862ea34c8ded9741fae2176c56c55c74827b2bd30cd1ef9eb37b\",\"statistical_assumptions_digest\":\"50b8c3d1ac87e1da574e773ec4a396a4c74213407d7845ab17672f4cf2ad30b2\",\"profile\":\"quality_gain\",\"statistical_unit\":\"independent_task_cluster\",\"weighting\":\"equal_units\",\"prefix_selection\":\"frozen_continuous_order\",\"execution_authority\":\"not_issued\",\"n_planned\":2},\"alpha_plan\":{\"schema_version\":\"rsia.research_family_alpha_plan.v1\",\"research_family_id\":\"family-e05\",\"alpha_total\":\"0.05\",\"allocations\":[{\"claim_id\":\"fixed-gain\",\"attempt_id\":\"attempt-1\",\"kind\":\"fixed_sample_gain\",\"alpha\":\"0.04\"},{\"claim_id\":\"early-reject\",\"attempt_id\":\"attempt-1\",\"kind\":\"sequential_reject\",\"alpha\":\"0.01\"}]},\"optimizer_comparison\":{\"schema_version\":\"rsia.optimizer_comparison_contract.v1\",\"c_simple\":{\"arm\":\"c_simple\",\"implementation_digest\":\"a7a39b72f29718e653e73503210fbb597057b7a1c77d1fe321a1afcff041d4e1\",\"initial_s0_digest\":\"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513\",\"base_model_digest\":\"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4\",\"tools_digest\":\"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281\",\"authorized_materials_digest\":\"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88\",\"task_partition_digest\":\"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254\",\"context_limit_tokens\":4096,\"root_budget_scope_id\":\"billing-1\",\"budget_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0.001\"}},\"c_skillopt\":{\"arm\":\"c_skillopt\",\"implementation_digest\":\"d97dddf1b4635724f6d5eea620a2679bd7667da5dd6a3b3b9a2245852ea1c33a\",\"initial_s0_digest\":\"ec18eac8d758b1eba52d3c10d39adc6dd9806472cb4ae069635d383d9086a513\",\"base_model_digest\":\"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4\",\"tools_digest\":\"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281\",\"authorized_materials_digest\":\"afba1a0ce1c8af744d1951b94989616cd777a1e709b570c717f957784835dd88\",\"task_partition_digest\":\"2df962d37ee229c547fb4ebef038a1eaecd0cd6f1c109746f4a9c1fa80329254\",\"context_limit_tokens\":4096,\"root_budget_scope_id\":\"billing-1\",\"budget_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0.001\"}},\"d_starts_from\":\"c_skillopt\"},\"optimization_budget\":{\"schema_version\":\"rsia.optimization_budget_plan.v1\",\"root_budget_scope_id\":\"billing-1\",\"payer_id\":\"payer\",\"root_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0.001\"},\"admin_authorization_receipt_digest\":\"ec70f47b06c5879902c6546b934c89514f6b315fb04d20d94e9ebe9fcb6c7ce9\",\"stages\":[{\"stage\":\"history_collection\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"reflection\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"merge\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"ranking\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"development_execution\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"development_scoring\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"practice\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"consolidation\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"guidance\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"human_review\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}},{\"stage\":\"storage_cpu\",\"per_call_limit\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"},\"experiment_total\":{\"currency\":\"USD\",\"pricing_version\":\"pricing-v1\",\"amount\":\"0\"}}]},\"early_stop_plan\":null,\"anchor_matrix\":{\"schema_version\":\"rsia.anchor_coverage_matrix.v1\",\"profile_id\":\"quality-gain\",\"version\":\"anchors-v1\",\"critical_capabilities\":[{\"capability_id\":\"safety\",\"anchor_ids\":[\"anchor-1\"]}]},\"attempt_id\":\"attempt-1\",\"fixed_sample_claim_id\":\"fixed-gain\",\"billing_scope\":\"billing-1\",\"executor_actor\":\"executor\",\"evaluator_actor\":\"evaluator\",\"proposer_actor\":\"proposer\",\"approver_actor\":\"approver\",\"oracle_digest\":\"9297ea092eae8ad974be8d2c9d294283d0e5aeed5a3e34886dfb954dcf1ac6e3\",\"grader_digest\":\"649c2cd5a8aebd7c31c6378e9294b5a991bb86dd7c8dd9cedcdae722d1d5b92a\",\"fixed_grader\":{\"schema_version\":\"rsia.fixed_grader.v1\",\"version\":\"exact-json-v1\",\"method\":\"exact_json_answer_v1\"},\"evidence_scope\":\"program_fixture\",\"created_at_unix_seconds\":2}}","revision":1}],"cache":[{"namespace":"n","actor":"evaluator","operation":"experiment.register","request_key":"ag155-capture-registration","payload_hash":"d22f58f331a893f330c9280c5a8570ebaea912a807678a2bbadbf5c1124aebc6","subject_id":"management-job-ae10c9ec67325b06cea1ff97","response":"{\"id\":\"management-job-ae10c9ec67325b06cea1ff97\",\"schema_version\":\"rsia.management_job.v1\",\"operation\":\"experiment.register\",\"request_key\":\"ag155-capture-registration\",\"payload_digest\":\"d22f58f331a893f330c9280c5a8570ebaea912a807678a2bbadbf5c1124aebc6\",\"owner_actor\":\"evaluator\",\"owner_role\":\"evaluator\",\"state\":\"queued\",\"step\":\"accepted\",\"private_input_ref\":\"management-input-ae10c9ec67325b06cea1ff97\",\"result\":null,\"error_code\":null,\"cancel_requested\":false,\"lease_token\":null,\"lease_until\":0,\"generation\":0,\"diagnostics\":[],\"created_at\":1793306575}","redacted":0}]}}"###;

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    ctx: Context,
    sample: Value,
}
async fn legacy(case: &str) -> Fixture {
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
    Fixture {
        _dir: dir,
        store,
        ctx,
        sample,
    }
}
async fn body(fx: &Fixture, kind: &str, id: &str) -> String {
    let mut s = fx.store.session().await.unwrap();
    let row = s.object_row(&fx.ctx, kind, id).await.unwrap().unwrap();
    s.commit().await.unwrap();
    row.body
}
async fn terminal(
    dispatcher: &ManagementDispatcher,
    fx: &Fixture,
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
fn queued_raw() -> String {
    let all: Value = serde_json::from_str(LEGACY).unwrap();
    let id = all["queued"]["job_id"].as_str().unwrap();
    all["queued"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap()["body"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn legacy_v1_terminal_bytes_and_suffix_map_without_rewrite() {
    let fx = legacy("world_terminal_suffix").await;
    let id = fx.sample["job_id"].as_str().unwrap();
    let before = body(&fx, "job", id).await;
    let d = ManagementDispatcher::new(fx.store.clone(), vec![fx.ctx.clone()]).unwrap();
    let read = d.cancel(&fx.ctx, id).await.unwrap();
    assert!(matches!(read, VersionedManagementJob::V1(_)));
    assert_eq!(
        read.cancel_arrival(),
        Some(evo_storage::ManagementCancelArrival::AfterSideEffect)
    );
    assert!(!read.step().ends_with(":cancel_after_side_effect"));
    assert_eq!(body(&fx, "job", id).await, before);
    assert!(
        decode_management_job(&before)
            .unwrap()
            .into_unfinished_v2()
            .is_err()
    );
}

#[test]
fn versioned_job_decoder_rejects_unknown_schema_and_shape() {
    let original = queued_raw();
    let mut v: Value = serde_json::from_str(&original).unwrap();
    v["schema_version"] = json!("rsia.management_job.v3");
    assert!(
        matches!(decode_management_job(&v.to_string()),Err(Error::Conflict(ref s)) if s=="management_job_unknown_schema")
    );
    v["schema_version"] = json!("rsia.management_job.v2");
    assert!(
        decode_management_job(&v.to_string()).is_ok(),
        "an uncancelled v1 shape is a legal v2 shape"
    );
    v["state"] = json!("unregistered_state");
    assert!(
        matches!(decode_management_job(&v.to_string()),Err(Error::Conflict(ref s)) if s=="management_job_invalid_shape")
    );
    let duplicate = original.replacen('{', "{\"schema_version\":\"rsia.management_job.v9\",", 1);
    assert!(decode_management_job(&duplicate).is_err());
}

#[tokio::test]
async fn current_v2_cancel_arrival_is_absent_before_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("fresh.sqlite3"))
        .await
        .unwrap();
    let ctx = Context::new("n", "admin", Role::Admin).unwrap();
    let d = ManagementDispatcher::new(store.clone(), vec![ctx.clone()]).unwrap();
    let accepted=d.submit(&ctx,"meta.start",json!({"schema_version":"rsia.management.meta_start.v1","request_key":"fresh-no-arrival"})).await.unwrap();
    assert!(matches!(accepted, VersionedManagementJob::V2(_)));
    let v = serde_json::to_value(&accepted).unwrap();
    assert!(v.get("cancel_arrival").is_none());
    assert_eq!(v["cancel_requested"], false);
    let id = accepted.identity().id.to_owned();
    for _ in 0..500 {
        let current = d.status(&ctx, &id).await.unwrap();
        if current.identity().state == ManagementJobState::Blocked {
            assert!(
                serde_json::to_value(current)
                    .unwrap()
                    .get("cancel_arrival")
                    .is_none()
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("meta consumer did not return its original blocked outcome");
}

#[test]
fn current_v2_rejects_step_cancel_suffix_and_bad_arrival() {
    let current = decode_management_job(&queued_raw())
        .unwrap()
        .into_unfinished_v2()
        .unwrap();
    let valid = serde_json::to_value(current).unwrap();
    for bad in [json!(null), json!("unknown_arrival"), json!(false)] {
        let mut v = valid.clone();
        v["cancel_requested"] = json!(true);
        v["cancel_arrival"] = bad;
        assert!(decode_management_job(&v.to_string()).is_err());
    }
    let mut v = valid.clone();
    v["step"] = json!("queued:cancel_after_side_effect");
    assert!(decode_management_job(&v.to_string()).is_err());
    let mut v = valid;
    v["cancel_arrival"] = json!("after_side_effect");
    assert!(decode_management_job(&v.to_string()).is_err());
}

#[tokio::test]
async fn same_business_key_returns_same_job_across_v1_v2() {
    let fx = legacy("queued").await;
    let id = fx.sample["job_id"].as_str().unwrap();
    let request = fx.sample["request"].clone();
    let d = ManagementDispatcher::new(fx.store.clone(), vec![fx.ctx.clone()]).unwrap();
    let first = d
        .submit(&fx.ctx, "meta.start", request.clone())
        .await
        .unwrap();
    assert_eq!(first.identity().id, id);
    let closed = terminal(&d, &fx, id).await;
    assert!(matches!(closed, VersionedManagementJob::V2(_)));
    let before = body(&fx, "job", id).await;
    let replay = d
        .submit(&fx.ctx, "meta.start", request.clone())
        .await
        .unwrap();
    assert_eq!(replay.identity().id, id);
    assert_eq!(body(&fx, "job", id).await, before);
    let mut changed = request;
    changed["extra"] = json!(true);
    assert!(d.submit(&fx.ctx, "meta.start", changed).await.is_err());
    let mut s = fx.store.session().await.unwrap();
    assert_eq!(s.list::<Value>(&fx.ctx, "job").await.unwrap().len(), 1);
    s.commit().await.unwrap();
}

#[tokio::test]
async fn nonterminal_v1_resumes_as_v2_with_same_id_input_and_progress() {
    let fx = legacy("running").await;
    let id = fx.sample["job_id"].as_str().unwrap();
    let input = fx.sample["input_id"].as_str().unwrap();
    let old = decode_management_job(&body(&fx, "job", id).await).unwrap();
    let input_before = body(&fx, "artifact", input).await;
    let d = ManagementDispatcher::new(fx.store.clone(), vec![fx.ctx.clone()]).unwrap();
    assert_eq!(d.recover_pending().await.unwrap(), 1);
    let closed = terminal(&d, &fx, id).await;
    assert!(matches!(closed, VersionedManagementJob::V2(_)));
    assert_eq!(closed.identity().id, id);
    assert_eq!(closed.identity().request_key, old.identity().request_key);
    assert_eq!(
        closed.identity().payload_digest,
        old.identity().payload_digest
    );
    assert_eq!(closed.generation(), old.generation());
    assert_eq!(closed.identity().private_input_ref, input);
    assert_eq!(closed.identity().state, ManagementJobState::Failed);
    assert_eq!(closed.error_code(), Some("management_worker_exited"));
    assert_eq!(body(&fx, "artifact", input).await, input_before);
    assert_eq!(d.recover_pending().await.unwrap(), 0);
}

#[tokio::test]
async fn legacy_control_commit_resumes_without_registering_control_again() {
    let fx = legacy("control_committed_holdout_not_committed").await;
    let id = fx.sample["job_id"].as_str().unwrap();
    let row=fx.sample["objects"].as_array().unwrap().iter().find(|r|r["kind"]=="artifact" && serde_json::from_str::<Value>(r["body"].as_str().unwrap()).unwrap()["record_kind"]=="registered_evaluation_control_v41").unwrap();
    let cid = row["id"].as_str().unwrap();
    let original = body(&fx, "artifact", cid).await;
    let d = ManagementDispatcher::new(fx.store.clone(), vec![fx.ctx.clone()]).unwrap();
    assert_eq!(d.recover_pending().await.unwrap(), 1);
    let closed = terminal(&d, &fx, id).await;
    assert_eq!(closed.identity().state, ManagementJobState::Blocked);
    assert_eq!(closed.error_code(), Some("blocked_partial_commit"));
    assert_eq!(closed.identity().id, id);
    assert_eq!(body(&fx, "artifact", cid).await, original);
    assert!(
        matches!(closed,evo_storage::VersionedManagementJob::V2(ref v) if matches!(v.result,Some(evo_storage::ManagementResultV2::Partial(_))))
    );
}

#[tokio::test]
async fn replay_root_original_accepts_exact_v1_v2_job_bindings() {
    replay_compatibility::verify_bindings().await;
}

mod replay_compatibility {
    use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
    use evo_core::optimization::{OptimizationTrace, TraceOutcome};
    use evo_core::replay::{
        PrefixCoverageV1, ReplayWorldV3, WorldPartition, replay_baseline_observation_bytes_v2,
    };
    use evo_core::strategy::{ElasticPolicyV1, ExplorationCapsV1, SimulationContext};
    use evo_core::{Context, Role, fingerprint, hash};
    use evo_engine::dispatch::{ExplorationStartRequest, ManagementDispatcher, ManagementJobState};
    use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
    use evo_engine::exploration::{
        ExplorationDependency, ExplorationWorldV1, PersistentCoordinator, RootOpportunity,
        WorldState,
    };
    use evo_engine::replay::{
        ReplayWorldV3Seed, collect_replay_world_v3_draft, prepare_replay_world_v3,
    };
    use evo_storage::Store;

    fn d(label: &str) -> String {
        hash(label.as_bytes())
    }
    struct Fixture {
        dir: tempfile::TempDir,
        store: Store,
        admin: Context,
        host: Context,
        registered: ExplorationWorldV1,
    }
    impl Fixture {
        fn path(&self) -> std::path::PathBuf {
            self.dir.path().join("replay-v3.sqlite3")
        }
    }
    async fn issue_controlled_source(fixture: &Fixture, id: &str, body: Vec<u8>) {
        let authority = StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: id.into(),
                body: body.clone(),
                parent_family: format!("family-{id}"),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: OptimizationTrace {
                run_id: id.into(),
                parent_family: format!("family-{id}"),
                source_digest: hash(&body),
                purpose: Purpose::Development,
                outcome: TraceOutcome::Success,
                diagnosis: None,
                excerpt: String::from_utf8(body.clone()).unwrap(),
                seed: 7,
            },
            excerpt_start: 0,
            excerpt_end: body.len(),
        };
        store_trace_authority(&fixture.store, &fixture.host, &authority)
            .await
            .unwrap();
    }
    async fn setup() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("replay-v3.sqlite3"))
            .await
            .unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let host = Context::new("n", "host", Role::Host).unwrap();
        let mut registered = ExplorationWorldV1 {
            schema_version: ExplorationWorldV1::SCHEMA.into(),
            id: "e09-original".into(),
            approved_parent_digest: d("controlled-parent"),
            context_signature: String::new(),
            parent_skill_digest: d("skill"),
            parent_bundle_digest: d("bundle"),
            environment_digest: d("environment"),
            model_digest: d("model"),
            tools_digest: d("tools"),
            grader_digest: d("grader"),
            rules_digest: d("rules"),
            source_watermark: 1,
            caps: ExplorationCapsV1::online(),
            policy: ElasticPolicyV1::default(),
            simulation: SimulationContext::Online { fixed_seed: 7 },
            // Preserve the original order; the new material separately sorts slots.
            root_opportunities: vec![
                RootOpportunity {
                    root_slot: 2,
                    branch_seq: 2,
                    action_seq: 2,
                    estimated_cost_upper_micros: 10,
                },
                RootOpportunity {
                    root_slot: 1,
                    branch_seq: 1,
                    action_seq: 1,
                    estimated_cost_upper_micros: 10,
                },
            ],
            dependencies: vec![ExplorationDependency {
                kind: "run".into(),
                id: "registered-input".into(),
            }],
            successor_cost_upper_micros: 10,
            initial_baseline_quality_micros: 100_000,
            remaining_root_micros: 1_000,
            remaining_recovery_dispatches: 1,
            state: WorldState::Collecting,
            node_ids: vec![],
            dispatch_ids: vec![],
            history_ids: vec![],
            current_branch_seq: None,
            current_branch_focus_actions: 0,
            decision_round: 0,
            waits: vec![],
        };
        registered.context_signature = fingerprint(&(
            &registered.parent_skill_digest,
            &registered.parent_bundle_digest,
            &registered.environment_digest,
            &registered.model_digest,
            &registered.tools_digest,
            &registered.grader_digest,
            &registered.rules_digest,
            registered.source_watermark,
        ))
        .unwrap();
        let fixture = Fixture {
            dir,
            store,
            admin,
            host,
            registered,
        };
        issue_controlled_source(
            &fixture,
            "registered-input",
            b"actual controlled S0 input".to_vec(),
        )
        .await;
        let mut session = fixture.store.session().await.unwrap();
        session
            .bump_watermark(&fixture.host, &d("initial-source"))
            .await
            .unwrap();
        session.commit().await.unwrap();
        PersistentCoordinator::new(fixture.store.clone(), fixture.admin.clone(), "admin")
            .unwrap()
            .register_world(fixture.registered.clone())
            .await
            .unwrap();
        fixture
    }
    fn seed(fixture: &Fixture) -> ReplayWorldV3Seed {
        ReplayWorldV3Seed {
            cluster_id: "cluster".into(),
            partition: WorldPartition::Train,
            generation_signature: d("generation"),
            world_context_signature: fixture.registered.context_signature.clone(),
            baseline_context_signature: d("baseline-context"),
            approved_parent_digest: fixture.registered.approved_parent_digest.clone(),
            model_digest: fixture.registered.model_digest.clone(),
            tools_digest: fixture.registered.tools_digest.clone(),
            scorer_digest: fixture.registered.grader_digest.clone(),
            guidance_digest: d("frozen-guidance"),
            repair_template_digest: d("frozen-repair"),
            input_order_digest: d("frozen-order"),
            initial_baseline_quality_micros: 100_000,
            baseline_observation_source_id: "baseline-v2".into(),
            prefix_coverage: vec![PrefixCoverageV1 {
                context_signature: d("baseline-context"),
                exhausted: false,
            }],
            action_catalog: vec![],
        }
    }
    async fn collecting(fixture: &Fixture) -> ReplayWorldV3 {
        let world = prepare_replay_world_v3(
            &fixture.admin,
            &fixture.store,
            &fixture.registered.id,
            seed(fixture),
        )
        .await
        .unwrap();
        assert_eq!(
            world.manifest.root_source.registration_digest,
            fixture.registered.registration_fingerprint().unwrap()
        );
        assert_eq!(
            world
                .manifest
                .root_source
                .roots
                .iter()
                .map(|root| root.root_slot)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        issue_controlled_source(
            fixture,
            &world.manifest.baseline_observation_source_id,
            replay_baseline_observation_bytes_v2(&world.manifest).unwrap(),
        )
        .await;
        collect_replay_world_v3_draft(&fixture.admin, &fixture.store, &world)
            .await
            .unwrap();
        world
    }

    async fn add_original(fixture: &Fixture, key: &str) -> String {
        let dispatcher =
            ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
        let job = dispatcher
            .submit(
                &fixture.admin,
                "exploration.start",
                serde_json::to_value(ExplorationStartRequest {
                    schema_version: "rsia.management.exploration_start.v1".into(),
                    request_key: key.into(),
                    world: fixture.registered.clone(),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        for _ in 0..1000 {
            let current = dispatcher
                .status(&fixture.admin, job.identity().id)
                .await
                .unwrap();
            if current.identity().state == ManagementJobState::Succeeded {
                return job.identity().id.to_owned();
            }
            assert!(!matches!(
                current.identity().state,
                ManagementJobState::Failed | ManagementJobState::Cancelled
            ));
            tokio::task::yield_now().await;
        }
        panic!("actual exploration registration job did not complete");
    }
    pub(super) async fn verify_bindings() {
        for legacy in [true, false] {
            let fixture = setup().await;
            let id = add_original(
                &fixture,
                if legacy {
                    "legacy-original"
                } else {
                    "current-original"
                },
            )
            .await;
            let mut s = fixture.store.session().await.unwrap();
            let row = s
                .object_row(&fixture.admin, "job", &id)
                .await
                .unwrap()
                .unwrap();
            let current = evo_storage::decode_management_job(&row.body).unwrap();
            assert!(matches!(
                current,
                evo_storage::VersionedManagementJob::V2(_)
            ));
            if legacy {
                // A named compatible projection of a real producer is distinct
                // from the qualified historical byte constants tested above.
                let mut value: serde_json::Value = serde_json::from_str(&row.body).unwrap();
                value["schema_version"] = serde_json::json!("rsia.management_job.v1");
                assert!(matches!(
                    evo_storage::decode_management_job(&value.to_string()).unwrap(),
                    evo_storage::VersionedManagementJob::V1(_)
                ));
                s.put(&fixture.admin, "job", &id, fixture.admin.actor(), &value)
                    .await
                    .unwrap();
            }
            s.commit().await.unwrap();
            let path = fixture.path();
            assert!(path.exists());
            let world = collecting(&fixture).await;
            assert_eq!(
                world.manifest.root_source.registration_digest,
                fixture.registered.registration_fingerprint().unwrap()
            );
            let mut s = fixture.store.session().await.unwrap();
            let row = s
                .object_row(&fixture.admin, "job", &id)
                .await
                .unwrap()
                .unwrap();
            s.commit().await.unwrap();
            assert_eq!(
                evo_storage::decode_management_job(&row.body)
                    .unwrap()
                    .identity()
                    .id,
                id
            );
            fixture.store.close().await;
        }
    }
}
