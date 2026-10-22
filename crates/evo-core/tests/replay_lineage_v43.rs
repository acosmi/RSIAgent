// AG147 pure wire/static controls. The eight byte strings below are the
// retained outputs of actual historical parent engines in the existing tests.
// Reading their source text avoids inventing replacement historical JSON.
// These tests grant no source/Host/Store/Current authority.
use evo_core::replay::*;
use evo_core::{fingerprint, hash};

const RECOVERY: &str = include_str!("../../evo-engine/tests/replay_recovery_contract_v42.rs");
const FAIRNESS: &str = include_str!("../../evo-engine/tests/replay_fairness_waits_v42.rs");
const LEGAL_PREFIX: &str = include_str!("../../evo-engine/tests/replay_legal_prefix_v42.rs");

fn fixture<'a>(source: &'a str, name: &str, hashes: &str) -> &'a str {
    let opening = format!("const {name}: &str = r{hashes}\"");
    let closing = format!("\"{hashes};");
    source
        .split_once(opening.as_str())
        .expect("retained fixture opening")
        .1
        .split_once(closing.as_str())
        .expect("retained fixture closing")
        .0
}

fn actual_parent_reports() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            fixture(RECOVERY, "LEGACY_AFFECTED", "#"),
            "64b34091cdbd9b520257d526153257e164bd9db247f332ebfde0990fda5e1137",
        ),
        (
            fixture(RECOVERY, "LEGACY_VALID", "#"),
            "942a65b68e50ec0c2b9c211b6d6607502e137ee67d17499671d6a4243a2a087c",
        ),
        (
            fixture(RECOVERY, "LEGACY_HARD", "#"),
            "27132c6cb1096d34f92f5a8d75ea99490cd87cf5e55c672f331f2d8637bc4e21",
        ),
        (
            fixture(FAIRNESS, "LEGACY_AFFECTED", "##"),
            "7857092bd10b34e2a88ed77f207b2432d7cf9c7f6d2be7cb442b6091b9ca4af7",
        ),
        (
            fixture(FAIRNESS, "LEGACY_UNAFFECTED", "##"),
            "0b90e07557b0a0754cf071ba2ace701649496e4fc02fc29730bc4d6bebbcdc34",
        ),
        (
            fixture(LEGAL_PREFIX, "LEGACY_AFFECTED", "##"),
            "4e12d213a766f20c1408e63b23a9ddf24496b672a2651cfb71ea337a42b8f97b",
        ),
        (
            fixture(LEGAL_PREFIX, "LEGACY_CHAIN", "##"),
            "19be67813d2ef2116e10896278b8da604ef7047e7abe00b2249aecae06f9c715",
        ),
        (
            fixture(LEGAL_PREFIX, "LEGACY_RECOVER", "##"),
            "0a3c74a2a998331d03e51b85bf7c90fdb9b30eae2bbc5f6ff3865be693319781",
        ),
    ]
}

#[test]
fn all_eight_actual_parent_json_ids_and_digest_domains_are_unchanged() {
    for (bytes, expected_sha) in actual_parent_reports() {
        assert_eq!(hash(bytes.as_bytes()), expected_sha);
        let report: StoredReplayReportV1 = serde_json::from_str(bytes).unwrap();
        validate_historical_v2_report_without_pool(&report).unwrap();
        assert_eq!(serde_json::to_string(&report).unwrap(), bytes);
        assert_eq!(
            report.replay_engine_version,
            LEGACY_REPLAY_ENGINE_VERSION_V2
        );
        assert_eq!(report.schema_version, LEGACY_STORED_REPLAY_REPORT_SCHEMA_V1);
        assert!(
            report
                .member_reports
                .iter()
                .all(|member| member.report.schema_version == LEGACY_REPLAY_REPORT_SCHEMA_V2)
        );
        let legacy = replay_report_id_for_version(
            ReplayReportIdentityVersion::LegacyV2,
            &report.namespace,
            report.partition,
            &report.pool_digest,
            &report.policy_digest,
            &report.profile_digest,
            &report.caps_digest,
        )
        .unwrap();
        assert_eq!(legacy, report.report_id);
        let current = replay_report_id(
            &report.namespace,
            report.partition,
            &report.pool_digest,
            &report.policy_digest,
            &report.profile_digest,
            &report.caps_digest,
        )
        .unwrap();
        assert_ne!(current, report.report_id);
        assert_eq!(
            current,
            replay_report_id_for_version(
                ReplayReportIdentityVersion::NextV3,
                &report.namespace,
                report.partition,
                &report.pool_digest,
                &report.policy_digest,
                &report.profile_digest,
                &report.caps_digest,
            )
            .unwrap()
        );
        // Old payload CPU, member order, and both old digest domains survive.
        assert_eq!(
            fingerprint(&report.member_reports).unwrap(),
            report.reports_body_digest
        );
        assert_eq!(
            replay_report_semantic_digest(&report.member_reports).unwrap(),
            report.semantic_reports_digest
        );
    }
}

#[test]
fn historical_version_table_is_closed_and_v2_never_has_a_semantic_verifier() {
    assert_eq!(REPLAY_HISTORICAL_VERSIONS.len(), 2);
    let old = replay_historical_version(LEGACY_REPLAY_ENGINE_VERSION_V2).unwrap();
    assert_eq!(old.stored_schema, LEGACY_STORED_REPLAY_REPORT_SCHEMA_V1);
    assert_eq!(old.member_schema, LEGACY_REPLAY_REPORT_SCHEMA_V2);
    assert!(!old.retains_semantic_verifier);
    let new = replay_historical_version(NEXT_REPLAY_ENGINE_VERSION_V3).unwrap();
    assert_eq!(new.stored_schema, STORED_REPLAY_REPORT_SCHEMA_V2);
    assert_eq!(new.member_schema, REPLAY_REPORT_SCHEMA_V3);
    assert!(new.retains_semantic_verifier);
    for future in ["rsia.replay.engine.v4", "rsia.replay.engine.v3.1", ""] {
        assert!(matches!(
            replay_historical_version(future),
            Err(ReplayReadIsolation::VersionOutsideStaticTable { .. })
        ));
    }
}

#[test]
fn unchanged_v2_payload_cannot_be_retagged_to_the_current_or_future_domain() {
    for (bytes, _) in actual_parent_reports() {
        let original: StoredReplayReportV1 = serde_json::from_str(bytes).unwrap();
        for engine in [NEXT_REPLAY_ENGINE_VERSION_V3, "rsia.replay.engine.v4"] {
            let mut changed = original.clone();
            changed.replay_engine_version = engine.into();
            assert!(validate_historical_v2_report_without_pool(&changed).is_err());
        }
        let mut wrong_inner = original.clone();
        wrong_inner.member_reports[0].report.schema_version = REPLAY_REPORT_SCHEMA_V3.into();
        wrong_inner.reports_body_digest = fingerprint(&wrong_inner.member_reports).unwrap();
        wrong_inner.semantic_reports_digest =
            replay_report_semantic_digest(&wrong_inner.member_reports).unwrap();
        assert!(validate_historical_v2_report_without_pool(&wrong_inner).is_err());
        let mut wrong_outer = original.clone();
        wrong_outer.schema_version = STORED_REPLAY_REPORT_SCHEMA_V2.into();
        assert!(validate_historical_v2_report_without_pool(&wrong_outer).is_err());
        assert_eq!(serde_json::to_string(&original).unwrap(), bytes);
    }
}

#[test]
fn static_invalid_read_hides_payload_and_cannot_be_current() {
    let invalid = ReplayStaticReportRead::invalid("fixture-static-invalid");
    assert_eq!(invalid.state(), ReplayRecordState::Invalid);
    assert!(!invalid.source_legacy());
    assert!(invalid.report().is_none());
    assert_eq!(invalid.invalid_reason(), Some("fixture-static-invalid"));
    assert!(matches!(
        invalid.refuse_new_consumption(),
        ReplayReadIsolation::InvalidRecord { .. }
    ));
}

// Captured by the actual M33 parent engine and reopened Store, not recreated
// JSON or an authority fixture. Original export SHA256:
// 2e7cfac6d2eab7fb90cf4a49edfdc549bfdb94d5d89428d4a5c83cc8271d49b0.
// The controlled namespace, source IDs and cost facts grant no Current/Host
// or real paid effect. CPU 551750 is retained exactly as recorded.
const M33_WORLD_TRAIN: &str = r#"{"schema_version":"rsia.replay_world.v2","manifest":{"schema_version":"rsia.replay_manifest.v2","world_id":"world-train","cluster_id":"cluster-train","partition":"train","purpose":"development","generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","world_context_signature":"9dfc2d475ba34d340ef0031c734aeb9b01099e0012bdb37692ad081f05a0ae23","baseline_context_signature":"8ba8496a2525ae171ffd104d632dede6ef418d9b95962a9d88e2fcdbc8d48d24","approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","model_digest":"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4","tools_digest":"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281","scorer_digest":"c528cab03ad47ed5fceb01fd4f4557f12d8f2bcb2aa9f6e78087380aab8fde5f","guidance_digest":"f8aac527f9c928fcec7b1d67c73839a57741bf86271b16731e01d75bba683c53","repair_template_digest":"a1a14ff4aab4f1d3efbe2f3fe8e32ec686289ba95e5b2fc3e1f38052d64da522","input_order_digest":"3eeb7e96e59ce40f9cb1a089daba079fd699f6867a30f6634af8570967b2375a","initial_baseline_quality_micros":100000,"baseline_observation_source_id":"baseline-source-train","source_closure":[{"source_id":"train-source-1","content_digest":"62a045a534e97186308f5e3ba62cbb0b25d9cf5e1391e3fce844d15d582fec5f"},{"source_id":"train-source-2","content_digest":"3d18bbe1b70956ea9a20a7a5d7b36bb4f3a44959c1b26f5e89549d6bd5cf2be8"},{"source_id":"baseline-source-train","content_digest":"3077d0b88986899031c0a091db54998584a27d3e64ffd409667e87645bf8cded"}],"revoke_watermark":7,"prefix_coverage":[{"context_signature":"8ba8496a2525ae171ffd104d632dede6ef418d9b95962a9d88e2fcdbc8d48d24","exhausted":false},{"context_signature":"b6d44cbebe5ecbaf699fa507c2c268d7748dea144e89de07a767f18cb686176d","exhausted":false},{"context_signature":"e5f4a81e15f1cf5a8be63b8b9eb4a9a4b18da704796d165a696235091aad1af3","exhausted":true}],"action_catalog":[{"record_seq":1,"generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","parent_context_signature":"8ba8496a2525ae171ffd104d632dede6ef418d9b95962a9d88e2fcdbc8d48d24","branch_seq":1,"target_depth":1,"action_kind":{"action":"widen","root_slot":1},"estimated_cost_upper_micros":10,"writes_shared_workspace":false},{"record_seq":2,"generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","parent_context_signature":"b6d44cbebe5ecbaf699fa507c2c268d7748dea144e89de07a767f18cb686176d","branch_seq":1,"target_depth":2,"action_kind":{"action":"deepen","parent_node_seq":1},"estimated_cost_upper_micros":10,"writes_shared_workspace":false}]},"transitions":[{"record_id":"opaque-root","record_seq":1,"generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","parent_context_signature":"8ba8496a2525ae171ffd104d632dede6ef418d9b95962a9d88e2fcdbc8d48d24","action_kind":{"action":"widen","root_slot":1},"next_context_signature":"b6d44cbebe5ecbaf699fa507c2c268d7748dea144e89de07a767f18cb686176d","outcome":{"outcome":"observed","status":{"status":"valid","quality_micros":400000}},"actual_usage":{"input_tokens":1,"output_tokens":1,"cost_micros":1,"latency_millis":1},"source_ids":["train-source-1"],"observation_source_id":"train-source-1"},{"record_id":"opaque-child","record_seq":2,"generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","parent_context_signature":"b6d44cbebe5ecbaf699fa507c2c268d7748dea144e89de07a767f18cb686176d","action_kind":{"action":"deepen","parent_node_seq":1},"next_context_signature":"e5f4a81e15f1cf5a8be63b8b9eb4a9a4b18da704796d165a696235091aad1af3","outcome":{"outcome":"observed","status":{"status":"valid","quality_micros":800000}},"actual_usage":{"input_tokens":1,"output_tokens":1,"cost_micros":1,"latency_millis":1},"source_ids":["train-source-2"],"observation_source_id":"train-source-2"}],"sealed_digest":"17e1b975ff8a9dc37e48f302ec9f983ab2dbb3a512422944a692a3e13a09fb7e"}"#;
const M33_WORLD_SELECT: &str = r#"{"schema_version":"rsia.replay_world.v2","manifest":{"schema_version":"rsia.replay_manifest.v2","world_id":"world-1","cluster_id":"cluster-1","partition":"select","purpose":"development","generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","world_context_signature":"9dfc2d475ba34d340ef0031c734aeb9b01099e0012bdb37692ad081f05a0ae23","baseline_context_signature":"8ba8496a2525ae171ffd104d632dede6ef418d9b95962a9d88e2fcdbc8d48d24","approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","model_digest":"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4","tools_digest":"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281","scorer_digest":"c528cab03ad47ed5fceb01fd4f4557f12d8f2bcb2aa9f6e78087380aab8fde5f","guidance_digest":"f8aac527f9c928fcec7b1d67c73839a57741bf86271b16731e01d75bba683c53","repair_template_digest":"a1a14ff4aab4f1d3efbe2f3fe8e32ec686289ba95e5b2fc3e1f38052d64da522","input_order_digest":"3eeb7e96e59ce40f9cb1a089daba079fd699f6867a30f6634af8570967b2375a","initial_baseline_quality_micros":100000,"baseline_observation_source_id":"baseline-source-1","source_closure":[{"source_id":"source-1","content_digest":"95dd9cc465a3000b6d872074463f4397bea5b2594cb973758e452bf1b77dd151"},{"source_id":"source-2","content_digest":"8d433792d5ae8a60b661ad9d1cb6a2a7ce92d54957e768013c7a1b5d47ae0224"},{"source_id":"baseline-source-1","content_digest":"571f5fe0ddb4eb5a71e531c6f0908e4a1b9b3a712b7724671eec46741bb85d39"}],"revoke_watermark":7,"prefix_coverage":[{"context_signature":"8ba8496a2525ae171ffd104d632dede6ef418d9b95962a9d88e2fcdbc8d48d24","exhausted":false},{"context_signature":"b6d44cbebe5ecbaf699fa507c2c268d7748dea144e89de07a767f18cb686176d","exhausted":false},{"context_signature":"e5f4a81e15f1cf5a8be63b8b9eb4a9a4b18da704796d165a696235091aad1af3","exhausted":true}],"action_catalog":[{"record_seq":1,"generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","parent_context_signature":"8ba8496a2525ae171ffd104d632dede6ef418d9b95962a9d88e2fcdbc8d48d24","branch_seq":1,"target_depth":1,"action_kind":{"action":"widen","root_slot":1},"estimated_cost_upper_micros":10,"writes_shared_workspace":false},{"record_seq":2,"generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","parent_context_signature":"b6d44cbebe5ecbaf699fa507c2c268d7748dea144e89de07a767f18cb686176d","branch_seq":1,"target_depth":2,"action_kind":{"action":"deepen","parent_node_seq":1},"estimated_cost_upper_micros":10,"writes_shared_workspace":false}]},"transitions":[{"record_id":"opaque-root","record_seq":1,"generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","parent_context_signature":"8ba8496a2525ae171ffd104d632dede6ef418d9b95962a9d88e2fcdbc8d48d24","action_kind":{"action":"widen","root_slot":1},"next_context_signature":"b6d44cbebe5ecbaf699fa507c2c268d7748dea144e89de07a767f18cb686176d","outcome":{"outcome":"observed","status":{"status":"valid","quality_micros":400000}},"actual_usage":{"input_tokens":1,"output_tokens":1,"cost_micros":1,"latency_millis":1},"source_ids":["source-1"],"observation_source_id":"source-1"},{"record_id":"opaque-child","record_seq":2,"generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","parent_context_signature":"b6d44cbebe5ecbaf699fa507c2c268d7748dea144e89de07a767f18cb686176d","action_kind":{"action":"deepen","parent_node_seq":1},"next_context_signature":"e5f4a81e15f1cf5a8be63b8b9eb4a9a4b18da704796d165a696235091aad1af3","outcome":{"outcome":"observed","status":{"status":"valid","quality_micros":800000}},"actual_usage":{"input_tokens":1,"output_tokens":1,"cost_micros":1,"latency_millis":1},"source_ids":["source-2"],"observation_source_id":"source-2"}],"sealed_digest":"aa57fc730004aed79e7d80cabbfe8b49ccf713a3cb0eada82875779967bbfce9"}"#;
const M33_POOL: &str = r#"{"schema_version":"rsia.replay_pool.v1","compiler_version":"rsia.replay_pool.compiler.v1","pool_digest":"c64ad9a401dbcb44eeabee0e3f307824c63230d29e63acad8c9a65c686176370","compatibility":{"purpose":"development","generation_signature":"019b9bcefbc83559566668ebbf7ed26c7b1b7f05f00ad7b1e2013f62d29c9066","approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","model_digest":"9372c470eeadd5ecd9c3c74c2b3cb633f8e2f2fad799250a0f70d652b6b825e4","tools_digest":"f9d35d43770d39092a663e665e82ae1d84a9e0da3d0d10c407acada6a40cd281","scorer_digest":"c528cab03ad47ed5fceb01fd4f4557f12d8f2bcb2aa9f6e78087380aab8fde5f","guidance_digest":"f8aac527f9c928fcec7b1d67c73839a57741bf86271b16731e01d75bba683c53","repair_template_digest":"a1a14ff4aab4f1d3efbe2f3fe8e32ec686289ba95e5b2fc3e1f38052d64da522","revoke_watermark":7},"members":[{"world_id":"world-1","world_digest":"aa57fc730004aed79e7d80cabbfe8b49ccf713a3cb0eada82875779967bbfce9","cluster_id":"cluster-1","partition":"select"},{"world_id":"world-train","world_digest":"17e1b975ff8a9dc37e48f302ec9f983ab2dbb3a512422944a692a3e13a09fb7e","cluster_id":"cluster-train","partition":"train"}]}"#;
const M33_REPORT: &str = r#"{"schema_version":"rsia.replay_report.v2","world_id":"world-1","world_digest":"aa57fc730004aed79e7d80cabbfe8b49ccf713a3cb0eada82875779967bbfce9","cluster_id":"cluster-1","partition":"select","policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile_digest":"efb528b6177aa8a982e66a25a3dc4886c42a6f9bd366e2881931102087da2bef","objective_version":"rsia.pareto_attainment.v2","batches":[{"decision_round":1,"action_seqs":[1],"record_seqs":[1],"revealed_quality_micros":[400000]},{"decision_round":2,"action_seqs":[2],"record_seqs":[2],"revealed_quality_micros":[800000]}],"probe_best_quality_micros":[400000,800000],"round_best_quality_micros":[400000,800000],"final_quality_micros":800000,"probes":2,"simulated_rounds":2,"parallel_penalty":{"numerator":2,"denominator":2},"no_probe":false,"attainment_auc_micros":600000,"q_auc_sim_micros":600000,"score_v2_micros":500000,"historical_usage":{"input_tokens":2,"output_tokens":2,"cost_micros":2,"latency_millis":2},"replay_cpu_nanos":551750,"coverage":{"attempted_actions":2,"observed_actions":2,"censored_actions":0,"out_of_support_actions":0,"complete_support":true},"terminal":"budget_exhausted","terminal_reason":"probe budget exhausted","development_only":true,"revealed_prefix":{"schema_version":"rsia.prefix_view.v2","context_signature":"9dfc2d475ba34d340ef0031c734aeb9b01099e0012bdb37692ad081f05a0ae23","approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","initial_baseline_quality_micros":100000,"nodes":[{"node_seq":1,"branch_seq":1,"search_parent_seq":null,"approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","depth":1,"status":{"status":"valid","quality_micros":400000},"best_valid_ancestor_micros":400000,"recent_valid_gains_micros":[300000],"repair_failures_dispatched":0},{"node_seq":2,"branch_seq":1,"search_parent_seq":1,"approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","depth":2,"status":{"status":"valid","quality_micros":800000},"best_valid_ancestor_micros":800000,"recent_valid_gains_micros":[300000,400000],"repair_failures_dispatched":0}],"current_branch_seq":1,"current_branch_focus_actions":2,"decisions_completed":2,"waits":[],"nodes_used":2,"recovery_dispatches_used":0}}"#;
const M33_STORED_REPORT: &str = r#"{"schema_version":"rsia.stored_replay_report.v1","replay_engine_version":"rsia.replay.engine.v2","report_id":"replay-report-b927f09726a8b36f56461a126387d8eb7eb3d5e17f6519746ebd25040c138551","namespace":"ag147-m33-history","purpose":"development","partition":"select","pool_digest":"c64ad9a401dbcb44eeabee0e3f307824c63230d29e63acad8c9a65c686176370","policy":{"schema_version":"rsia.elastic_priority.v1","significant_gain_micros":20000,"stagnation_abs_gain_micros":5000,"stagnation_window":2,"max_focus_actions":2,"fairness_wait_rounds":4},"policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile":{"simulation_version":"rsia.unit_probe_barrier.v1","objective":"pareto_attainment_v2","w_sim":1,"probe_budget":2,"horizon":2,"lambda_work_micros":50000,"lambda_round_micros":50000,"fixed_seed":9,"global_recovery_dispatch_limit":1,"pool_digest":"c64ad9a401dbcb44eeabee0e3f307824c63230d29e63acad8c9a65c686176370","purpose":"development","target_runtime_profile":"simulation-only"},"profile_digest":"efb528b6177aa8a982e66a25a3dc4886c42a6f9bd366e2881931102087da2bef","caps":{"schema_version":"rsia.exploration_caps.v1","w_online":1,"max_nodes":12,"max_depth":4,"max_repair_dispatches_per_episode":1},"caps_digest":"81f3bf209b41d41276ab87a00fa7fd30e9fdf3792ed08b27174c0f27a0cdf67f","member_reports":[{"world_id":"world-1","world_digest":"aa57fc730004aed79e7d80cabbfe8b49ccf713a3cb0eada82875779967bbfce9","report":{"schema_version":"rsia.replay_report.v2","world_id":"world-1","world_digest":"aa57fc730004aed79e7d80cabbfe8b49ccf713a3cb0eada82875779967bbfce9","cluster_id":"cluster-1","partition":"select","policy_digest":"1ad1c74c6632e1edc116c7e430f0ec32d7aec0dbe34617612cdc9009aa30c423","profile_digest":"efb528b6177aa8a982e66a25a3dc4886c42a6f9bd366e2881931102087da2bef","objective_version":"rsia.pareto_attainment.v2","batches":[{"decision_round":1,"action_seqs":[1],"record_seqs":[1],"revealed_quality_micros":[400000]},{"decision_round":2,"action_seqs":[2],"record_seqs":[2],"revealed_quality_micros":[800000]}],"probe_best_quality_micros":[400000,800000],"round_best_quality_micros":[400000,800000],"final_quality_micros":800000,"probes":2,"simulated_rounds":2,"parallel_penalty":{"numerator":2,"denominator":2},"no_probe":false,"attainment_auc_micros":600000,"q_auc_sim_micros":600000,"score_v2_micros":500000,"historical_usage":{"input_tokens":2,"output_tokens":2,"cost_micros":2,"latency_millis":2},"replay_cpu_nanos":551750,"coverage":{"attempted_actions":2,"observed_actions":2,"censored_actions":0,"out_of_support_actions":0,"complete_support":true},"terminal":"budget_exhausted","terminal_reason":"probe budget exhausted","development_only":true,"revealed_prefix":{"schema_version":"rsia.prefix_view.v2","context_signature":"9dfc2d475ba34d340ef0031c734aeb9b01099e0012bdb37692ad081f05a0ae23","approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","initial_baseline_quality_micros":100000,"nodes":[{"node_seq":1,"branch_seq":1,"search_parent_seq":null,"approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","depth":1,"status":{"status":"valid","quality_micros":400000},"best_valid_ancestor_micros":400000,"recent_valid_gains_micros":[300000],"repair_failures_dispatched":0},{"node_seq":2,"branch_seq":1,"search_parent_seq":1,"approved_parent_digest":"a50399b580df41a70a8fe9c128cbf6cc1133701d1435185e4b6112510eb9ae58","depth":2,"status":{"status":"valid","quality_micros":800000},"best_valid_ancestor_micros":800000,"recent_valid_gains_micros":[300000,400000],"repair_failures_dispatched":0}],"current_branch_seq":1,"current_branch_focus_actions":2,"decisions_completed":2,"waits":[],"nodes_used":2,"recovery_dispatches_used":0}}}],"reports_body_digest":"a75bca55ea87f33af1525718dffaf2e69e0aeb40d1cb046b0abf33faef1e3801","semantic_reports_digest":"0753367f7ee2daed09a1cfeb774fc2e89458ef0e3cf3ca44104b7d7fb3e7992a"}"#;

#[test]
fn actual_m33_parent_world_wire_and_sealed_digest_domains_survive() {
    for (raw, expected_sha, world_id, sealed_digest) in [
        (
            M33_WORLD_TRAIN,
            "c3138117c5ffaee541a7c3353529f8bf5dde8dcf276688c29d130659f193058b",
            "world-train",
            "17e1b975ff8a9dc37e48f302ec9f983ab2dbb3a512422944a692a3e13a09fb7e",
        ),
        (
            M33_WORLD_SELECT,
            "4d93c4d7bbe5161589de06e8c50cca6437fc26dd388b4813fa80f408e8e86a6b",
            "world-1",
            "aa57fc730004aed79e7d80cabbfe8b49ccf713a3cb0eada82875779967bbfce9",
        ),
    ] {
        assert_eq!(hash(raw.as_bytes()), expected_sha);
        let world: ReplayWorldV2 = serde_json::from_str(raw).unwrap();
        world.validate_sealed().unwrap();
        assert_eq!(serde_json::to_string(&world).unwrap(), raw);
        assert_eq!(world.manifest.world_id, world_id);
        assert_eq!(world.sealed_digest.as_deref(), Some(sealed_digest));
        let mut resealed = world.clone();
        resealed.sealed_digest = None;
        assert_eq!(resealed.seal().unwrap(), sealed_digest);
        assert_eq!(world.manifest.schema_version, "rsia.replay_manifest.v2");
        assert_eq!(world.schema_version, "rsia.replay_world.v2");
    }
}

#[test]
fn actual_m33_parent_pool_hash_is_unchanged_for_the_projection_compiler() {
    assert_eq!(
        hash(M33_POOL.as_bytes()),
        "69c2b3e9f5be05cab0c2356991091d8645c1280ca781bd71b0c74e7d6c642174"
    );
    let train: ReplayWorldV2 = serde_json::from_str(M33_WORLD_TRAIN).unwrap();
    let select: ReplayWorldV2 = serde_json::from_str(M33_WORLD_SELECT).unwrap();
    let pool: ReplayPoolManifestV1 = serde_json::from_str(M33_POOL).unwrap();
    pool.validate_against(&[train.clone(), select.clone()])
        .unwrap();
    assert_eq!(ReplayPoolManifestV1::build(&[select, train]).unwrap(), pool);
    let projections: Vec<_> = pool
        .members
        .iter()
        .rev()
        .map(|member| (pool.compatibility.clone(), member.clone()))
        .collect();
    let recompiled = ReplayPoolManifestV1::build_from_projections(&projections).unwrap();
    assert_eq!(recompiled, pool);
    assert_eq!(serde_json::to_string(&recompiled).unwrap(), M33_POOL);
    assert_eq!(pool.compiler_version, "rsia.replay_pool.compiler.v1");
    assert_eq!(
        pool.pool_digest,
        "c64ad9a401dbcb44eeabee0e3f307824c63230d29e63acad8c9a65c686176370"
    );
}

#[test]
fn actual_m33_parent_report_ids_wire_and_cpu_are_legacy_only() {
    assert_eq!(
        hash(M33_REPORT.as_bytes()),
        "80e6efa240ca6344524226727cf893e4a0f195e465d95d93151cb1a4b6c49142"
    );
    assert_eq!(
        hash(M33_STORED_REPORT.as_bytes()),
        "1d8393ee75237cce3178bb198fd568d438b69181881c9d0ef8b5c69e54df8327"
    );
    let report: ReplayReportV2 = serde_json::from_str(M33_REPORT).unwrap();
    let stored: StoredReplayReportV1 = serde_json::from_str(M33_STORED_REPORT).unwrap();
    let pool: ReplayPoolManifestV1 = serde_json::from_str(M33_POOL).unwrap();
    assert_eq!(serde_json::to_string(&report).unwrap(), M33_REPORT);
    assert_eq!(serde_json::to_string(&stored).unwrap(), M33_STORED_REPORT);
    stored.validate_static(&pool).unwrap();
    validate_historical_v2_report_without_pool(&stored).unwrap();
    assert_eq!(report.replay_cpu_nanos, 551750);
    assert_eq!(report.probes, 2);
    assert_eq!(report.terminal, ReplayTerminal::BudgetExhausted);
    assert_eq!(stored.member_reports.len(), 1);
    assert_eq!(
        serde_json::to_string(&stored.member_reports[0].report).unwrap(),
        M33_REPORT
    );
    assert_eq!(
        stored.report_id,
        "replay-report-b927f09726a8b36f56461a126387d8eb7eb3d5e17f6519746ebd25040c138551"
    );
    assert_eq!(
        stored.reports_body_digest,
        "a75bca55ea87f33af1525718dffaf2e69e0aeb40d1cb046b0abf33faef1e3801"
    );
    assert_eq!(
        stored.semantic_reports_digest,
        "0753367f7ee2daed09a1cfeb774fc2e89458ef0e3cf3ca44104b7d7fb3e7992a"
    );
    assert_eq!(
        fingerprint(&stored.member_reports).unwrap(),
        stored.reports_body_digest
    );
    assert_eq!(
        replay_report_semantic_digest(&stored.member_reports).unwrap(),
        stored.semantic_reports_digest
    );
    let inputs = (
        &stored.namespace,
        stored.partition,
        &stored.pool_digest,
        &stored.policy_digest,
        &stored.profile_digest,
        &stored.caps_digest,
    );
    assert_eq!(
        replay_report_id_for_version(
            ReplayReportIdentityVersion::LegacyV2,
            inputs.0,
            inputs.1,
            inputs.2,
            inputs.3,
            inputs.4,
            inputs.5
        )
        .unwrap(),
        stored.report_id
    );
    assert_ne!(
        replay_report_id(inputs.0, inputs.1, inputs.2, inputs.3, inputs.4, inputs.5).unwrap(),
        stored.report_id
    );
    assert_eq!(
        stored.replay_engine_version,
        LEGACY_REPLAY_ENGINE_VERSION_V2
    );
    // No current builder/replay is invoked to manufacture a legacy payload.
    // Static readability does not retain a v2 semantic verifier.
    assert!(
        !replay_historical_version(&stored.replay_engine_version)
            .unwrap()
            .retains_semantic_verifier
    );
}
