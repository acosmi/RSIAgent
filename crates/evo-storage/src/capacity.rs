//! Instance measurements read in the same transaction as the admitted intent.
use crate::{Session, internal};
use evo_core::Result;
use evo_core::capacity::CapacityItem;
use serde_json::Value;
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{BTreeMap, BTreeSet};

type Identities = BTreeMap<CapacityItem, BTreeSet<(String, String)>>;

fn add(ids: &mut Identities, item: CapacityItem, namespace: &str, id: &str) {
    ids.entry(item)
        .or_default()
        .insert((namespace.into(), id.into()));
}

fn digest(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// A tag or cleanup event alone cannot free content capacity. The persisted
/// cleanup must have reached this exact node and completed under its tombstone.
pub(crate) async fn lawfully_redacted(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    kind: &str,
    id: &str,
    owner: &str,
    value: &Value,
) -> Result<bool> {
    let fields = if kind == "replay_world" {
        [
            "schema_version",
            "state",
            "original_schema",
            "original_digest",
            "sealed_digest",
            "metadata",
            "historical_transitions",
        ]
    } else {
        [
            "id",
            "schema_version",
            "state",
            "original_kind",
            "original_schema",
            "original_digest",
            "metadata",
        ]
    };
    let Some(object) = value.as_object() else {
        return Ok(false);
    };
    if object.len() != fields.len()
        || fields.iter().any(|field| !object.contains_key(*field))
        || value["schema_version"] != "rsia.redacted.v1"
        || !matches!(value["state"].as_str(), Some("source_revoked" | "revoked"))
        || !digest(&value["original_digest"])
        || !value["original_schema"].is_string()
        || !value["metadata"].is_object()
        || (kind != "replay_world" && value["id"].as_str() != Some(id))
        || (kind == "replay_world" && !value["historical_transitions"].is_array())
    {
        return Ok(false);
    }
    let rows = sqlx::query(
        "SELECT j.source_kind,j.source_id,j.watermark_seq,j.watermark_digest,t.body,e.event_kind,e.details
         FROM revoke_cleanup_jobs j
         JOIN revoke_cleanup_frontier f ON f.namespace=j.namespace AND f.job_id=j.job_id
         JOIN revoke_cleanup_events e ON e.namespace=f.namespace AND e.job_id=f.job_id
              AND e.node_kind=f.node_kind AND e.node_id=f.node_id
         JOIN objects t ON t.namespace=j.namespace AND t.kind='tombstone' AND t.id=j.source_id
         WHERE j.namespace=? AND j.state='complete' AND f.node_kind=? AND f.node_id=? AND f.expanded=1
           AND e.event_kind IN ('content_redacted','e16_content_redacted','world_content_redacted')",
    ).bind(namespace).bind(kind).bind(id).fetch_all(&mut **tx).await.map_err(internal)?;
    for row in rows {
        let raw: String = row.try_get("body").map_err(internal)?;
        let Ok(parsed) = crate::lifecycle::parse_strict_json(&raw) else {
            continue;
        };
        let Ok(tombstone) = serde_json::from_value::<crate::lifecycle::RevokeTombstone>(parsed)
        else {
            continue;
        };
        if tombstone.schema_version != crate::lifecycle::REVOKE_TOMBSTONE_SCHEMA
            || !matches!(tombstone.source_kind.as_str(), "run" | "artifact")
            || tombstone.id != row.try_get::<String, _>("source_id").map_err(internal)?
            || tombstone.source_kind != row.try_get::<String, _>("source_kind").map_err(internal)?
            || tombstone.reason.trim().is_empty()
            || tombstone.reason.len() > 512
            || tombstone.created_at < 0
            || tombstone.watermark_seq == 0
            || tombstone.watermark_seq
                != row.try_get::<i64, _>("watermark_seq").map_err(internal)? as u64
            || tombstone.watermark_digest
                != row
                    .try_get::<String, _>("watermark_digest")
                    .map_err(internal)?
            || !digest(&Value::String(tombstone.watermark_digest))
        {
            continue;
        }
        let event: String = row.try_get("event_kind").map_err(internal)?;
        if event == "content_redacted" {
            let details: String = row.try_get("details").map_err(internal)?;
            let details = crate::lifecycle::parse_strict_json(&details)?;
            if value["original_kind"].as_str() != Some(kind)
                || details["original_digest"] != value["original_digest"]
            {
                continue;
            }
            if value["original_schema"] == "rsia.optimization.stage_fact.v2"
                && details["job_scope_digest"]
                    != serde_json::json!(evo_core::fingerprint(&value["metadata"]["job_scope"])?)
            {
                continue;
            }
        } else if event == "e16_content_redacted" {
            let metadata = value["metadata"].as_object().expect("checked object");
            let original_kind = value["original_schema"]
                .as_str()
                .and_then(crate::lifecycle::e16_schema_kind);
            let details: String = row.try_get("details").map_err(internal)?;
            let details = crate::lifecycle::parse_strict_json(&details)?;
            if kind != "artifact"
                || metadata.len() != 9
                || original_kind.is_none()
                || value["original_kind"].as_str() != original_kind
                || details["record_kind"].as_str() != original_kind
                || value["metadata"]["owner_actor"].as_str() != Some(owner)
                || crate::lifecycle::validate_e16_metadata(
                    namespace,
                    metadata,
                    original_kind.unwrap(),
                )
                .is_err()
                || value["metadata"]["payload"]
                    .as_object()
                    .is_none_or(|payload| {
                        payload.iter().any(|(key, value)| {
                            !crate::lifecycle::E16_RETAINED_PAYLOAD_FIELDS.contains(&key.as_str())
                                || value.is_object()
                                || value.is_array()
                        })
                    })
            {
                continue;
            }
        } else if kind != "replay_world" {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

fn registered_record(schema: &str, namespace: &str, id: &str, owner: &str, value: &Value) -> bool {
    match schema {
        "rsia.e16.staged_asset.v1" | "rsia.e16.import_result.v1" | "rsia.e16.import_result.v2" => {
            let node = crate::lifecycle::TypedObjectRef {
                kind: "artifact".into(),
                id: id.into(),
            };
            value["namespace"].as_str() == Some(namespace)
                && value["owner_actor"].as_str() == Some(owner)
                && crate::lifecycle::validate_e16_envelope(namespace, &node, value).is_ok()
        }
        "rsia.exploration_artifact_envelope.v1"
        | "rsia.curriculum_artifact_envelope.v1"
        | "rsia.replay_economic_artifact_envelope.v1" => {
            let Some(object) = value.as_object() else {
                return false;
            };
            let Some(kind) = value["record_kind"].as_str() else {
                return false;
            };
            if object.len() != 4 || !value["payload"].is_object() {
                return false;
            }
            let payload = &value["payload"];
            let logical = match kind {
                "exploration_node_v1" => Some(format!(
                    "node-{}-{}",
                    payload["world_id"].as_str().unwrap_or(""),
                    payload["node"]["node_seq"].as_u64().unwrap_or(0)
                )),
                "optimization_history_v1" => payload["entry_id"].as_str().map(str::to_owned),
                "meta_trial_v1" => payload["trial_id"].as_str().map(str::to_owned),
                _ => payload["id"].as_str().map(str::to_owned),
            };
            let prefix = match schema {
                "rsia.exploration_artifact_envelope.v1" => "e09",
                "rsia.curriculum_artifact_envelope.v1" => "e12",
                _ => "e11",
            };
            logical.is_some_and(|logical| {
                evo_core::fingerprint(&(kind, logical))
                    .is_ok_and(|digest| format!("{prefix}-{digest}") == id)
            })
        }
        "rsia.practice_attempt_set.v1" | "rsia.practice_attempt_set.v2" => {
            value["namespace"].as_str() == Some(namespace)
                && value["set_id"].is_string()
                && value["control_id"].is_string()
                && digest(&value["request_digest"])
                && value["attempts"].is_array()
        }
        "rsia.host_surface_record.v1" => {
            value["manifest"].is_object()
                && value["extracted"].is_array()
                && digest(&value["manifest_digest"])
        }
        "rsia.registered_external_provider.v1"
        | "rsia.registered_external_provider.v2"
        | "rsia.registered_external_provider.v3"
        | "rsia.registered_resource_instrumentation.v1" => {
            value["namespace"].as_str() == Some(namespace)
                && value["authorized_by"].as_str() == Some(owner)
                && value["host_registration_id"].is_string()
        }
        "rsia.release_candidate.v1" | "rsia.release_candidate.v2" => {
            let bundle = if schema == "rsia.release_candidate.v2" {
                serde_json::from_value::<evo_core::contract::ResolvedBundleV3>(
                    value["bundle"].clone(),
                )
                .is_ok()
            } else {
                serde_json::from_value::<evo_core::contract::ResolvedBundle>(
                    value["bundle"].clone(),
                )
                .is_ok()
            };
            bundle
                && value["proposer_actor"].as_str() == Some(owner)
                && value["sources"].is_array()
                && digest(&value["bundle_digest"])
                && value["bundle_digest"] == value["bundle"]["digest"]
        }
        "rsia.monitoring.development_cycle.v1" | "rsia.monitoring.development_cycle.v2" => {
            value["report_fact_id"].is_string()
                && value["scope_id"].is_string()
                && value["sources"].is_array()
        }
        "rsia.monitoring.consolidation_claim.v1" => {
            value["scope_id"].is_string()
                && value["cycle_ids"].is_array()
                && value["sources"].is_array()
        }
        _ => true,
    }
}

pub(crate) async fn counts(
    tx: &mut Transaction<'_, Sqlite>,
) -> Result<BTreeMap<CapacityItem, u64>> {
    use CapacityItem::*;
    let mut ids = Identities::new();
    let mut events = 0u64;
    let mut nodes = BTreeSet::new();
    let mut intents = Vec::new();
    let mut projections = Vec::new();
    let mut current_candidates = BTreeSet::new();
    let mut jobs = BTreeMap::<(String, String), Value>::new();
    let mut cursor = 0i64;
    loop {
        let rows=sqlx::query("SELECT rowid,namespace,kind,id,owner,body,CASE WHEN json_type(body,'$.schema_version')='text' THEN json_extract(body,'$.schema_version') END AS object_schema,CASE WHEN json_type(body,'$.original_schema')='text' THEN json_extract(body,'$.original_schema') END AS original_schema,CASE WHEN json_type(body,'$.source')='text' THEN json_extract(body,'$.source') END AS source,CAST(COALESCE(json_extract(body,'$.metadata.record_kind'),json_extract(body,'$.record_kind'),'') AS TEXT) AS record_kind FROM objects WHERE rowid>? ORDER BY rowid LIMIT 64")
            .bind(cursor).fetch_all(&mut **tx).await.map_err(internal)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            cursor = row.try_get("rowid").map_err(internal)?;
            let namespace: String = row.try_get("namespace").map_err(internal)?;
            let kind: String = row.try_get("kind").map_err(internal)?;
            let id: String = row.try_get("id").map_err(internal)?;
            let owner: String = row.try_get("owner").map_err(internal)?;
            if !matches!(
                kind.as_str(),
                "run" | "event" | "candidate" | "artifact" | "job"
            ) {
                continue;
            }
            let raw: String = row.try_get("body").map_err(internal)?;
            let stored_schema: Option<String> = row.try_get("object_schema").map_err(internal)?;
            let original_schema: Option<String> =
                row.try_get("original_schema").map_err(internal)?;
            let effective_schema = if stored_schema.as_deref() == Some("rsia.redacted.v1") {
                original_schema.as_deref().unwrap_or("")
            } else {
                stored_schema.as_deref().unwrap_or("")
            };
            if kind == "artifact"
                && !matches!(
                    effective_schema,
                    "rsia.optimization.stage_fact.v1"
                        | "rsia.optimization.stage_fact.v2"
                        | "rsia.e16.import_result.v1"
                        | "rsia.e16.import_result.v2"
                        | "rsia.release_candidate.v1"
                        | "rsia.release_candidate.v2"
                        | "rsia.e16.staged_asset.v1"
                        | "rsia.practice_attempt_set.v1"
                        | "rsia.practice_attempt_set.v2"
                        | "rsia.monitoring.development_cycle.v1"
                        | "rsia.monitoring.development_cycle.v2"
                        | "rsia.monitoring.consolidation_claim.v1"
                        | "rsia.host_surface_record.v1"
                        | "rsia.host_surface_record.v2"
                        | "rsia.registered_external_provider.v1"
                        | "rsia.registered_external_provider.v2"
                        | "rsia.registered_external_provider.v3"
                        | "rsia.registered_resource_instrumentation.v1"
                        | "rsia.meta_trial_artifact_envelope.v1"
                        | "rsia.typed_artifact_envelope.v1"
                        | "rsia.replay_economic_artifact_envelope.v1"
                        | "rsia.exploration_artifact_envelope.v1"
                        | "rsia.curriculum_artifact_envelope.v1"
                )
            {
                let source: Option<String> = row.try_get("source").map_err(internal)?;
                if !effective_schema.is_empty()
                    || source.as_deref() != Some("trusted_host_snapshot")
                {
                    continue;
                }
            }
            if kind == "artifact" {
                let record_kind: String = row.try_get("record_kind").map_err(internal)?;
                let counted = match effective_schema {
                    "rsia.meta_trial_artifact_envelope.v1" | "rsia.typed_artifact_envelope.v1" => {
                        record_kind == "meta_trial_v1"
                    }
                    "rsia.replay_economic_artifact_envelope.v1" => {
                        record_kind == "replay_economic_experiment_v1"
                    }
                    "rsia.exploration_artifact_envelope.v1" => matches!(
                        record_kind.as_str(),
                        "exploration_world_v1"
                            | "optimization_history_v1"
                            | "exploration_node_v1"
                            | "exploration_dispatch_v1"
                            | "meta_trial_v1"
                    ),
                    "rsia.curriculum_artifact_envelope.v1" => matches!(
                        record_kind.as_str(),
                        "development_cycle_receipt_v1"
                            | "coverage_probe_job_v1"
                            | "curriculum_selection_v1"
                            | "learner_state_v2"
                    ),
                    _ => true,
                };
                if !counted {
                    continue;
                }
            }
            let value = if stored_schema.as_deref() == Some("rsia.host_surface_record.v2") {
                evo_core::contract::decode_host_surface_record_v2_json(&raw)?
            } else {
                crate::lifecycle::parse_strict_json(&raw)?
            };
            let redacted = value["schema_version"] == "rsia.redacted.v1";
            let schema = if redacted {
                value["original_schema"].as_str()
            } else {
                value["schema_version"].as_str()
            }
            .unwrap_or("");
            let header = if redacted { &value["metadata"] } else { &value };
            if kind == "artifact"
                && matches!(
                    schema,
                    "rsia.optimization.stage_fact.v1" | "rsia.optimization.stage_fact.v2"
                )
                && header["kind"] == "step_prepared"
            {
                let scope = if schema == "rsia.optimization.stage_fact.v2" {
                    if redacted {
                        header["job_scope"].clone()
                    } else {
                        header["payload"]["job_scope"].clone()
                    }
                } else {
                    serde_json::json!({"job_id":format!("legacy-{}",evo_core::fingerprint(&(namespace.as_str(),&header["episode_id"],&header["request_id"]))?),
                  "groups":[{"group_id":"direct","episode_id":header["episode_id"],"request_id":header["request_id"],"input_digest":header["input_digest"],"step":header["step"],"attempt":header["attempt"]}]})
                };
                let job_id = scope["job_id"]
                    .as_str()
                    .ok_or_else(|| {
                        evo_core::Error::Conflict("optimization job scope is missing".into())
                    })?
                    .to_owned();
                if let Some(previous) = jobs.insert((namespace.clone(), job_id), scope.clone())
                    && previous != scope
                {
                    return Err(evo_core::Error::Conflict(
                        "optimization job scope changed".into(),
                    ));
                }
            }
            if redacted && lawfully_redacted(tx, &namespace, &kind, &id, &owner, &value).await? {
                continue;
            }
            if value
                .get("namespace")
                .is_some_and(|v| v.as_str() != Some(namespace.as_str()))
                || value
                    .get("owner_actor")
                    .is_some_and(|v| v.as_str() != Some(owner.as_str()))
                || value
                    .get("owner")
                    .is_some_and(|v| v.as_str() != Some(owner.as_str()))
            {
                continue;
            }
            let schema = if redacted {
                value["original_schema"].as_str()
            } else {
                value["schema_version"].as_str()
            }
            .unwrap_or("");
            let identity = value["id"]
                .as_str()
                .or_else(|| value["artifact_id"].as_str());
            if identity.is_some_and(|v| v != id) {
                continue;
            }
            if kind == "run" {
                add(&mut ids, Run, &namespace, &id);
                continue;
            }
            if kind == "event" {
                add(&mut ids, Event, &namespace, &id);
                continue;
            }
            if kind == "candidate" {
                if redacted
                    || (serde_json::from_value::<evo_core::Candidate>(value.clone()).is_ok()
                        && value["proposal"]["kind"] == "skill")
                {
                    add(&mut ids, Skill, &namespace, &format!("candidate:{id}"));
                }
                continue;
            }
            if kind == "job"
                && matches!(
                    schema,
                    crate::MANAGEMENT_JOB_V1_SCHEMA | crate::MANAGEMENT_JOB_V2_SCHEMA
                )
                && identity == Some(id.as_str())
            {
                add(&mut ids, ManagementJobRecord, &namespace, &id);
                continue;
            }
            if kind != "artifact" || identity != Some(id.as_str()) {
                continue;
            }
            if !redacted && !registered_record(schema, &namespace, &id, &owner, &value) {
                continue;
            }
            let metadata = if redacted { &value["metadata"] } else { &value };
            let payload = &metadata["payload"];
            match schema {
                "" if value["source"] == "trusted_host_snapshot"
                    && serde_json::from_value::<evo_core::Run>(value.clone()).is_ok() =>
                {
                    add(&mut ids, Run, &namespace, &id)
                }
                "rsia.e16.import_result.v1" | "rsia.e16.import_result.v2" => {
                    events += payload["aggregate_summary"]["total_events"]
                        .as_u64()
                        .ok_or_else(|| {
                            evo_core::Error::Conflict(
                                "existing capacity records contain an invalid event count".into(),
                            )
                        })?;
                }
                "rsia.release_candidate.v1" | "rsia.release_candidate.v2" => {
                    add(&mut ids, Skill, &namespace, &format!("artifact:{id}"));
                    if !redacted {
                        current_candidates.insert((namespace.clone(), id.clone()));
                    }
                }
                "rsia.e16.staged_asset.v1" => {
                    add(&mut ids, StagingPackage, &namespace, &id);
                    if payload["package_kind"] == "skill" {
                        add(&mut ids, Skill, &namespace, &format!("artifact:{id}"));
                        if let Some(candidate) = payload["candidate_ref"].as_str() {
                            projections.push((namespace.clone(), id.clone(), candidate.to_owned()));
                        }
                    }
                }
                "rsia.practice_attempt_set.v1" | "rsia.practice_attempt_set.v2" => {
                    add(&mut ids, PracticeAttemptSet, &namespace, &id)
                }
                "rsia.monitoring.development_cycle.v1" | "rsia.monitoring.development_cycle.v2" => {
                    add(
                        &mut ids,
                        DevelopmentCycle,
                        &namespace,
                        metadata["report_fact_id"].as_str().unwrap_or(&id),
                    );
                }
                "rsia.monitoring.consolidation_claim.v1" => {
                    add(&mut ids, ConsolidationClaim, &namespace, &id)
                }
                "rsia.host_surface_record.v1" | "rsia.host_surface_record.v2" => {
                    add(&mut ids, HostSurface, &namespace, &id)
                }
                "rsia.registered_external_provider.v1"
                | "rsia.registered_external_provider.v2"
                | "rsia.registered_external_provider.v3"
                | "rsia.registered_resource_instrumentation.v1" => {
                    add(&mut ids, ExternalService, &namespace, &id)
                }
                "rsia.meta_trial_artifact_envelope.v1" | "rsia.typed_artifact_envelope.v1"
                    if metadata["record_kind"] == "meta_trial_v1" =>
                {
                    add(&mut ids, MetaTrial, &namespace, &id)
                }
                "rsia.replay_economic_artifact_envelope.v1"
                    if metadata["record_kind"] == "replay_economic_experiment_v1" =>
                {
                    add(&mut ids, ReplayEconomicExperiment, &namespace, &id)
                }
                "rsia.exploration_artifact_envelope.v1" => match metadata["record_kind"]
                    .as_str()
                    .unwrap_or("")
                {
                    "exploration_world_v1" => add(&mut ids, ExplorationWorld, &namespace, &id),
                    "optimization_history_v1" => {
                        add(&mut ids, OptimizationHistory, &namespace, &id)
                    }
                    "exploration_node_v1" => {
                        nodes.insert((
                            namespace.clone(),
                            format!(
                                "node-{}-{}",
                                payload["world_id"].as_str().unwrap_or(""),
                                payload["node"]["node_seq"].as_u64().unwrap_or(0)
                            ),
                        ));
                        add(&mut ids, ExplorationNode, &namespace, &id);
                    }
                    "exploration_dispatch_v1" if payload["state"] == "claimed" => intents.push((
                        namespace.clone(),
                        payload["node_id"]
                            .as_str()
                            .or_else(|| payload["id"].as_str())
                            .unwrap_or(&id)
                            .to_owned(),
                    )),
                    "meta_trial_v1" => add(&mut ids, MetaTrial, &namespace, &id),
                    _ => {}
                },
                "rsia.curriculum_artifact_envelope.v1" => {
                    match metadata["record_kind"].as_str().unwrap_or("") {
                        "development_cycle_receipt_v1" => add(
                            &mut ids,
                            DevelopmentCycle,
                            &namespace,
                            payload["development_observed_fact_id"]
                                .as_str()
                                .unwrap_or(&id),
                        ),
                        "coverage_probe_job_v1" => {
                            add(&mut ids, CurriculumProbeJob, &namespace, &id)
                        }
                        "curriculum_selection_v1" => {
                            add(&mut ids, SelectionRecord, &namespace, &id)
                        }
                        "learner_state_v2" => {
                            if let Some(failures) = payload["failure_records"].as_array() {
                                for failure in failures {
                                    if let Some(fid) = failure["record_id"].as_str() {
                                        add(&mut ids, FailureRecord, &namespace, fid);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    for (namespace, intent) in intents {
        if !nodes.contains(&(namespace.clone(), intent.clone())) {
            add(&mut ids, CapacityItem::ExplorationNode, &namespace, &intent);
        }
    }
    for (namespace, staged, candidate) in projections {
        let edge: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dependencies WHERE namespace=? AND src_kind='artifact' AND src_id=? AND dst_kind='artifact' AND dst_id=?)")
            .bind(&namespace).bind(&candidate).bind(&staged).fetch_one(&mut **tx).await.map_err(internal)?;
        if edge != 0 && current_candidates.contains(&(namespace.clone(), candidate.clone())) {
            ids.entry(Skill)
                .or_default()
                .remove(&(namespace, format!("artifact:{candidate}")));
        }
    }
    let mut result = ids
        .into_iter()
        .map(|(item, ids)| (item, ids.len() as u64))
        .collect::<BTreeMap<_, _>>();
    *result.entry(Event).or_default() += events;
    let mut active = 0;
    for ((namespace, _), scope) in jobs {
        if !optimization_job_closed(tx, &namespace, &scope).await? {
            active += 1;
        }
    }
    result.insert(ConcurrentOptimizationJob, active);

    Ok(result)
}

async fn optimization_job_closed(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    scope: &Value,
) -> Result<bool> {
    let Some(groups) = scope["groups"].as_array() else {
        return Ok(false);
    };
    if !(1..=evo_core::capacity::CapacityItem::SkillGroupPerJob.limit() as usize)
        .contains(&groups.len())
    {
        return Ok(false);
    }
    let current = scope["job_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("optjob-"));
    let mut group_ids = BTreeSet::new();
    let mut episodes = BTreeSet::new();
    let mut requests = BTreeSet::new();
    for group in groups {
        let Some(episode) = group["episode_id"].as_str() else {
            return Ok(false);
        };
        let Some(request) = group["request_id"].as_str() else {
            return Ok(false);
        };
        let Some(input) = group["input_digest"].as_str() else {
            return Ok(false);
        };
        let Some(group_id) = group["group_id"].as_str() else {
            return Ok(false);
        };
        if !group_ids.insert(group_id) || !episodes.insert(episode) || !requests.insert(request) {
            return Ok(false);
        }
        let Some(step) = group["step"].as_u64() else {
            return Ok(false);
        };
        let Some(attempt) = group["attempt"].as_u64() else {
            return Ok(false);
        };
        let prepared_id = if current {
            format!(
                "optstage-{}",
                evo_core::fingerprint(&(
                    "rsia.optimization.stage_fact.v2",
                    namespace,
                    episode,
                    step,
                    attempt,
                    "merge",
                    "step_prepared",
                    request,
                ))?
            )
        } else {
            format!(
                "optstage-{}",
                evo_core::fingerprint(&(
                    namespace,
                    episode,
                    step,
                    attempt,
                    "merge",
                    "step_prepared",
                    request,
                ))?
            )
        };
        let prepared = if current {
            let id = prepared_id.clone();
            let Some(row) = sqlx::query(
                "SELECT owner,body FROM objects WHERE namespace=? AND kind='artifact' AND id=?",
            )
            .bind(namespace)
            .bind(&id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal)?
            else {
                return Ok(false);
            };
            let raw: String = row.try_get("body").map_err(internal)?;
            let value = crate::lifecycle::parse_strict_json(&raw)?;
            let redacted = value["schema_version"] == "rsia.redacted.v1";
            let header = if redacted { &value["metadata"] } else { &value };
            let schema = if redacted {
                &value["original_schema"]
            } else {
                &value["schema_version"]
            };
            let actual_scope = if redacted {
                &header["job_scope"]
            } else {
                &value["payload"]["job_scope"]
            };
            if schema != "rsia.optimization.stage_fact.v2"
                || header["artifact_id"] != id
                || header["namespace"] != namespace
                || header["kind"] != "step_prepared"
                || header["stage"] != "merge"
                || header["episode_id"] != episode
                || header["request_id"] != request
                || header["input_digest"] != input
                || header["step"] != group["step"]
                || header["attempt"] != group["attempt"]
                || actual_scope != scope
            {
                return Ok(false);
            }
            Some((
                id,
                row.try_get::<String, _>("owner").map_err(internal)?,
                value,
            ))
        } else {
            None
        };
        // Recovery facts retain the exact first Prepared as an edge. Episode
        // alone also contains earlier/later steps and cannot establish ownership.
        let rows: Vec<String> = sqlx::query_scalar(r#"SELECT f.body FROM dependencies d
            JOIN objects f ON f.namespace=d.namespace AND f.kind=d.src_kind AND f.id=d.src_id
            WHERE d.namespace=? AND d.src_kind='artifact' AND d.dst_kind='artifact' AND d.dst_id=?
              AND COALESCE(json_extract(f.body,'$.schema_version'), '') IN ('rsia.optimization.stage_fact.v1','rsia.optimization.stage_fact.v2','rsia.redacted.v1')
              AND COALESCE(json_extract(f.body,'$.kind'),json_extract(f.body,'$.metadata.kind')) IN ('dispatch_prepared','dispatch_observed','development_observed')"#)
            .bind(namespace).bind(&prepared_id).fetch_all(&mut **tx).await.map_err(internal)?;
        let mut facts = Vec::with_capacity(rows.len());
        let mut member_requests = BTreeSet::new();
        let mut member_calls = BTreeSet::new();
        let mut member_usage = BTreeSet::new();
        for raw in rows {
            let value = crate::lifecycle::parse_strict_json(&raw)?;
            let redacted = value["schema_version"] == "rsia.redacted.v1";
            let header = if redacted { &value["metadata"] } else { &value };
            if header["namespace"] != namespace
                || header["episode_id"] != episode
                || header["step"] != group["step"]
                || header["attempt"] != group["attempt"]
            {
                return Ok(false);
            }
            let Some(id) = header["request_id"].as_str() else {
                return Ok(false);
            };
            member_requests.insert(id.to_owned());
            let payload = if redacted {
                &header["historical_facts"]
            } else {
                &value["payload"]
            };
            for receipt in [
                &payload["execution_receipt"],
                &payload["dispatch"]["receipt"],
            ] {
                if let Some(id) = receipt["call_id"].as_str() {
                    member_calls.insert(id.to_owned());
                }
                if let Some(id) = receipt["usage_record_id"].as_str() {
                    member_usage.insert(id.to_owned());
                }
            }
            if let Some(ids) = payload["usage_record_ids"].as_array() {
                for id in ids {
                    if let Some(id) = id.as_str() {
                        member_usage.insert(id.to_owned());
                    }
                }
            }
            facts.push(value);
        }
        let requests_json = serde_json::to_string(&member_requests).map_err(internal)?;
        let calls_json = serde_json::to_string(&member_calls).map_err(internal)?;
        let usage_json = serde_json::to_string(&member_usage).map_err(internal)?;
        let accounting = sqlx::query(r#"SELECT COUNT(*) AS calls,COALESCE(SUM(CASE WHEN state IN ('reserved','dispatched','uncertain') OR execution_closed=0
            OR (state='finalized' AND (actual_cost_micros IS NULL OR usage_record_id IS NULL)) THEN 1 ELSE 0 END),0) AS unfinished
            FROM root_budget_calls WHERE namespace=?1 AND (
                call_id IN (SELECT value FROM json_each(?2))
                OR COALESCE(json_extract(request_artifact_body,'$.request_id'),json_extract(request_artifact_body,'$.request.request_id'),json_extract(request_artifact_body,'$.metadata.request_id')) IN (SELECT value FROM json_each(?2))
                OR call_id IN (SELECT value FROM json_each(?3))
                OR usage_record_id IN (SELECT value FROM json_each(?4)))"#)
            .bind(namespace).bind(&requests_json).bind(&calls_json).bind(&usage_json)
            .fetch_one(&mut **tx).await.map_err(internal)?;
        let calls: i64 = accounting.try_get("calls").map_err(internal)?;
        let unfinished: i64 = accounting.try_get("unfinished").map_err(internal)?;
        let terminal=sqlx::query("SELECT id,body FROM objects WHERE namespace=? AND kind='artifact' AND (
          (json_extract(body,'$.schema_version') IN ('rsia.optimization.stage_fact.v1','rsia.optimization.stage_fact.v2') AND json_extract(body,'$.kind')='step_completed' AND json_extract(body,'$.episode_id')=? AND json_extract(body,'$.request_id')=? AND json_extract(body,'$.input_digest')=? AND json_extract(body,'$.step')=? AND json_extract(body,'$.attempt')=?) OR
          (json_extract(body,'$.schema_version')='rsia.redacted.v1' AND json_extract(body,'$.original_schema') IN ('rsia.optimization.stage_fact.v1','rsia.optimization.stage_fact.v2') AND json_extract(body,'$.metadata.kind')='step_completed' AND json_extract(body,'$.metadata.episode_id')=? AND json_extract(body,'$.metadata.request_id')=? AND json_extract(body,'$.metadata.input_digest')=? AND json_extract(body,'$.metadata.step')=? AND json_extract(body,'$.metadata.attempt')=?))")
          .bind(namespace).bind(episode).bind(request).bind(input).bind(group["step"].as_i64()).bind(group["attempt"].as_i64()).bind(episode).bind(request).bind(input).bind(group["step"].as_i64()).bind(group["attempt"].as_i64()).fetch_optional(&mut **tx).await.map_err(internal)?;
        let Some(terminal) = terminal else {
            // Completed cleanup cancels a prepared group only when it never
            // dispatched. A redacted label or an expired lease proves neither.
            let Some((id, owner, value)) = prepared else {
                return Ok(false);
            };
            if value["schema_version"] != "rsia.redacted.v1"
                || !lawfully_redacted(tx, namespace, "artifact", &id, &owner, &value).await?
            {
                return Ok(false);
            }
            if !facts.is_empty() || calls != 0 {
                return Ok(false);
            }
            continue;
        };
        let raw: String = terminal.try_get("body").map_err(internal)?;
        let value = crate::lifecycle::parse_strict_json(&raw)?;
        let outcome = if value["schema_version"] == "rsia.redacted.v1" {
            &value["metadata"]["historical_facts"]
        } else {
            &value["payload"]
        };
        if !matches!(
            outcome["status"].as_str(),
            Some("candidate" | "no_change" | "rejected")
        ) {
            return Ok(false);
        }
        if unfinished != 0 {
            return Ok(false);
        }
        for value in facts {
            let header = if value["schema_version"] == "rsia.redacted.v1" {
                &value["metadata"]
            } else {
                &value
            };
            if header["kind"] != "dispatch_observed" {
                continue;
            }
            let payload = if value["schema_version"] == "rsia.redacted.v1" {
                &value["metadata"]["historical_facts"]
            } else {
                &value["payload"]
            };
            let receipt = if payload["status"] == "completed" {
                &payload["execution_receipt"]
            } else if payload["dispatch"]["state"] == "dispatched" {
                &payload["dispatch"]["receipt"]
            } else {
                continue;
            };
            match receipt["provenance"].as_str() {
                Some("fixture") => continue,
                Some("external_provider") => {}
                _ => return Ok(false),
            }
            let Some(call) = receipt["call_id"].as_str() else {
                return Ok(false);
            };
            let Some(dispatch) = receipt["dispatch_id"].as_str() else {
                return Ok(false);
            };
            let closed:i64=sqlx::query_scalar("SELECT COUNT(*) FROM root_budget_calls WHERE namespace=? AND call_id=? AND dispatch_id=? AND dispatch_group_id=? AND state='finalized' AND execution_closed=1 AND actual_cost_micros IS NOT NULL AND usage_record_id=? AND provider_request_id=?")
              .bind(namespace).bind(call).bind(dispatch).bind(episode).bind(receipt["usage_record_id"].as_str()).bind(receipt["provider_request_id"].as_str()).fetch_one(&mut **tx).await.map_err(internal)?;
            if closed != 1 {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

impl Session {
    /// Existing derived proposals remain spent for their original claim.
    pub async fn capacity_proposals_for_claim(
        &mut self,
        ctx: &evo_core::Context,
        claim_id: &str,
    ) -> Result<u64> {
        let used:i64=sqlx::query_scalar("SELECT COUNT(*) FROM objects WHERE namespace=? AND kind='artifact' AND (
          (json_extract(body,'$.schema_version') IN ('rsia.monitoring.consolidation_proposal.v1','rsia.monitoring.consolidation_proposal.v2') AND json_extract(body,'$.claim_id')=?) OR
          (json_extract(body,'$.schema_version')='rsia.redacted.v1' AND json_extract(body,'$.original_schema') IN ('rsia.monitoring.consolidation_proposal.v1','rsia.monitoring.consolidation_proposal.v2') AND json_extract(body,'$.metadata.claim_id')=?)))")
          .bind(ctx.namespace()).bind(claim_id).bind(claim_id).fetch_one(&mut *self.tx).await.map_err(internal)?;
        u64::try_from(used).map_err(|_| evo_core::Error::Internal)
    }

    pub async fn capacity_cycle_exists(
        &mut self,
        ctx: &evo_core::Context,
        report_id: &str,
    ) -> Result<bool> {
        let found: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM objects WHERE namespace=? AND kind='artifact' AND (
          (json_extract(body,'$.schema_version') IN ('rsia.monitoring.development_cycle.v1','rsia.monitoring.development_cycle.v2') AND json_extract(body,'$.report_fact_id')=?) OR
          (json_extract(body,'$.schema_version')='rsia.curriculum_artifact_envelope.v1' AND json_extract(body,'$.record_kind')='development_cycle_receipt_v1' AND json_extract(body,'$.payload.development_observed_fact_id')=?)))")
          .bind(ctx.namespace()).bind(report_id).bind(report_id).fetch_one(&mut *self.tx).await.map_err(internal)?;
        Ok(found != 0)
    }

    pub async fn capacity_failure_ids(
        &mut self,
        ctx: &evo_core::Context,
    ) -> Result<BTreeSet<String>> {
        let ids: Vec<String> = sqlx::query_scalar("SELECT DISTINCT json_extract(f.value,'$.record_id') FROM objects o,json_each(o.body,'$.payload.failure_records') f
          WHERE o.namespace=? AND o.kind='artifact' AND json_extract(o.body,'$.schema_version')='rsia.curriculum_artifact_envelope.v1'
          AND json_extract(o.body,'$.record_kind')='learner_state_v2' AND json_type(f.value,'$.record_id')='text'")
          .bind(ctx.namespace()).fetch_all(&mut *self.tx).await.map_err(internal)?;
        Ok(ids.into_iter().collect())
    }

    /// Aggregate counts only; this grants no cross-namespace content access.
    pub async fn capacity_used(&mut self, item: CapacityItem) -> Result<u64> {
        match item {
            CapacityItem::ActiveLease => {
                crate::count_active_leases(&mut self.tx, evo_core::now()?).await
            }
            CapacityItem::ConcurrentManagementDispatch => Ok(crate::count_management_job_capacity(
                &mut self.tx,
                evo_core::now()?,
            )
            .await?
            .concurrent_dispatches),
            CapacityItem::ReplayWorld => {
                let rows = sqlx::query("SELECT namespace,id,manifest FROM replay_worlds")
                    .fetch_all(&mut *self.tx)
                    .await
                    .map_err(internal)?;
                let mut used = 0;
                for row in rows {
                    let ns: String = row.try_get("namespace").map_err(internal)?;
                    let id: String = row.try_get("id").map_err(internal)?;
                    let raw: String = row.try_get("manifest").map_err(internal)?;
                    let value = crate::lifecycle::parse_strict_json(&raw)?;
                    if !lawfully_redacted(&mut self.tx, &ns, "replay_world", &id, "", &value)
                        .await?
                    {
                        used += 1;
                    }
                }
                Ok(used)
            }
            _ => Ok(counts(&mut self.tx).await?.get(&item).copied().unwrap_or(0)),
        }
    }

    /// The producer supplies the actual missing identities, after idempotency
    /// and before its first persistent or outbound side effect.
    pub async fn admit_capacity(&mut self, intent: &[(CapacityItem, u64)]) -> Result<()> {
        if intent.iter().all(|(_, delta)| *delta == 0) {
            return Ok(());
        }
        for &(item, delta) in intent {
            if delta == 0 {
                continue;
            }
            match item {
                CapacityItem::InflightPrepare => {
                    return Err(evo_core::Error::Invalid(
                        "prepare admission uses the Store RAII guard".into(),
                    ));
                }
                CapacityItem::ProposalPerClaim
                | CapacityItem::StagingBindingPerClaim
                | CapacityItem::SkillGroupPerJob
                | CapacityItem::ResetReceiptPerNode
                | CapacityItem::RootStagePlanVersionPerRoot => {
                    return Err(evo_core::Error::Invalid(
                        "owner-scoped admission requires the actual owner intent".into(),
                    ));
                }
                CapacityItem::RepairTemplate
                | CapacityItem::DirectionHint
                | CapacityItem::TraceExport
                | CapacityItem::ImproverRelease => {
                    return Err(evo_core::Error::Invalid(
                        "capacity item has no registered producer on this path".into(),
                    ));
                }
                _ => {}
            }
        }
        let counts = counts(&mut self.tx).await?;
        for &(item, delta) in intent {
            if delta != 0 {
                let used = match item {
                    CapacityItem::ActiveLease
                    | CapacityItem::ConcurrentManagementDispatch
                    | CapacityItem::ReplayWorld => self.capacity_used(item).await?,
                    _ => counts.get(&item).copied().unwrap_or(0),
                };
                item.admit(used, delta)?;
            }
        }
        Ok(())
    }
}

/// Process-local prepare admission shared by every clone/service of this Store.
#[derive(Debug)]
#[must_use = "dropping the guard releases the prepare slot"]
pub struct InflightPrepareGuard {
    counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
}
impl Drop for InflightPrepareGuard {
    fn drop(&mut self) {
        self.counter
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
impl crate::Store {
    pub fn inflight_prepare(&self) -> u64 {
        self.inflight_prepare
            .load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn begin_inflight_prepare(&self) -> Result<InflightPrepareGuard> {
        use std::sync::atomic::Ordering;
        let mut used = self.inflight_prepare();
        loop {
            CapacityItem::InflightPrepare.admit(used, 1)?;
            match self.inflight_prepare.compare_exchange_weak(
                used,
                used + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(InflightPrepareGuard {
                        counter: self.inflight_prepare.clone(),
                    });
                }
                Err(observed) => used = observed,
            }
        }
    }
}
