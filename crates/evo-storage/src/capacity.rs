//! Instance measurements read in the same transaction as the admitted intent.
//!
//! Every persisted row of a counted class occupies its slot. Only a completed
//! lawful cleanup frees one: a forged tag, a malformed record or a body that
//! does not parse keeps occupying it, and never fails the whole instance.
use crate::{Session, internal};
use evo_core::Result;
use evo_core::capacity::CapacityItem;
use serde::Deserialize;
use serde_json::Value;
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{BTreeMap, BTreeSet};

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

/// The schema a row was written under, also after its content was redacted.
const SCHEMA: &str = "(CASE WHEN json_extract(body,'$.schema_version')='rsia.redacted.v1' \
     THEN json_extract(body,'$.original_schema') ELSE json_extract(body,'$.schema_version') END)";
/// The record kind of a typed envelope, also after its content was redacted.
const RECORD_KIND: &str =
    "COALESCE(json_extract(body,'$.metadata.record_kind'),json_extract(body,'$.record_kind'))";
const EXPLORATION: &str = "'rsia.exploration_artifact_envelope.v1'";
const CURRICULUM: &str = "'rsia.curriculum_artifact_envelope.v1'";

/// One persisted row of a counted class.
struct Occupant {
    namespace: String,
    id: String,
    /// `None` when the body does not parse strictly. Such a row still occupies
    /// its slot instead of failing every admission of the instance.
    value: Option<Value>,
}

impl Occupant {
    /// The original header: the body itself, or the metadata a redaction kept.
    fn header(&self) -> &Value {
        static UNREADABLE: Value = Value::Null;
        match &self.value {
            Some(value) if value["schema_version"] == "rsia.redacted.v1" => &value["metadata"],
            Some(value) => value,
            None => &UNREADABLE,
        }
    }
}

/// Every row `filter` selects occupies capacity until a lawful cleanup has
/// redacted it. Only the selected rows are read and parsed.
async fn occupants(tx: &mut Transaction<'_, Sqlite>, filter: &str) -> Result<Vec<Occupant>> {
    let rows = sqlx::query(&format!(
        "SELECT namespace,kind,id,owner,body FROM objects WHERE {filter}"
    ))
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    let mut occupants = Vec::with_capacity(rows.len());
    for row in rows {
        let namespace: String = row.try_get("namespace").map_err(internal)?;
        let kind: String = row.try_get("kind").map_err(internal)?;
        let id: String = row.try_get("id").map_err(internal)?;
        let owner: String = row.try_get("owner").map_err(internal)?;
        let raw: String = row.try_get("body").map_err(internal)?;
        let value = crate::lifecycle::parse_strict_json(&raw).ok();
        if let Some(value) = &value
            && value["schema_version"] == "rsia.redacted.v1"
            && lawfully_redacted(tx, &namespace, &kind, &id, &owner, value).await?
        {
            continue;
        }
        occupants.push(Occupant {
            namespace,
            id,
            value,
        });
    }
    Ok(occupants)
}

fn distinct<'a>(ids: impl Iterator<Item = (&'a str, &'a str)>) -> u64 {
    ids.collect::<BTreeSet<_>>().len() as u64
}

/// The filter of an item each of whose rows is one identity.
fn row_class(item: CapacityItem) -> Option<String> {
    use CapacityItem::*;
    let artifact = |schemas: &str| format!("kind='artifact' AND {SCHEMA} IN ({schemas})");
    let envelope = |schemas: &str, kind: &str| {
        format!("kind='artifact' AND {SCHEMA} IN ({schemas}) AND {RECORD_KIND}='{kind}'")
    };
    Some(match item {
        Run => "kind='run' OR (kind='artifact' AND json_extract(body,'$.schema_version') IS NULL \
                AND json_extract(body,'$.source')='trusted_host_snapshot')"
            .into(),
        StagingPackage => artifact("'rsia.e16.staged_asset.v1'"),
        ManagementJobRecord => {
            format!(
                "kind='job' AND {SCHEMA} IN ('rsia.management_job.v1','rsia.management_job.v2')"
            )
        }
        ConsolidationClaim => artifact("'rsia.monitoring.consolidation_claim.v1'"),
        PracticeAttemptSet => {
            artifact("'rsia.practice_attempt_set.v1','rsia.practice_attempt_set.v2'")
        }
        HostSurface => artifact("'rsia.host_surface_record.v1','rsia.host_surface_record.v2'"),
        ExternalService => artifact(
            "'rsia.registered_external_provider.v1','rsia.registered_external_provider.v2',\
             'rsia.registered_external_provider.v3','rsia.registered_resource_instrumentation.v1'",
        ),
        MetaTrial => envelope(
            "'rsia.meta_trial_artifact_envelope.v1','rsia.typed_artifact_envelope.v1',\
             'rsia.exploration_artifact_envelope.v1'",
            "meta_trial_v1",
        ),
        ExplorationWorld => envelope(EXPLORATION, "exploration_world_v1"),
        OptimizationHistory => envelope(EXPLORATION, "optimization_history_v1"),
        CurriculumProbeJob => envelope(CURRICULUM, "coverage_probe_job_v1"),
        SelectionRecord => envelope(CURRICULUM, "curriculum_selection_v1"),
        ReplayEconomicExperiment => envelope(
            "'rsia.replay_economic_artifact_envelope.v1'",
            "replay_economic_experiment_v1",
        ),
        _ => return None,
    })
}

/// Event objects plus the events every import result version declares. A
/// result whose count is unreadable occupies one event.
async fn events(tx: &mut Transaction<'_, Sqlite>) -> Result<u64> {
    let objects = occupants(tx, "kind='event'").await?.len() as u64;
    let results = occupants(
        tx,
        &format!(
            "kind='artifact' AND {SCHEMA} IN ('rsia.e16.import_result.v1','rsia.e16.import_result.v2')"
        ),
    )
    .await?;
    Ok(results.iter().fold(objects, |sum, result| {
        let declared = result.header()["payload"]["aggregate_summary"]["total_events"].as_u64();
        sum.saturating_add(declared.unwrap_or(1))
    }))
}

/// Skill births: Skill candidates (never Improver ones), release candidates
/// and staged Skill packages. A staged projection proven by its edge from a
/// Current release candidate is the same Skill.
async fn skills(tx: &mut Transaction<'_, Sqlite>) -> Result<u64> {
    let mut ids = BTreeSet::new();
    for row in occupants(
        tx,
        "kind='candidate' AND (json_extract(body,'$.proposal.kind')='skill' OR json_extract(body,'$.schema_version')='rsia.redacted.v1')",
    )
    .await?
    {
        ids.insert((row.namespace, format!("candidate:{}", row.id)));
    }
    let mut current = BTreeSet::new();
    for row in occupants(
        tx,
        &format!(
            "kind='artifact' AND {SCHEMA} IN ('rsia.release_candidate.v1','rsia.release_candidate.v2')"
        ),
    )
    .await?
    {
        if row.value.as_ref().is_some_and(|v| v["schema_version"] != "rsia.redacted.v1") {
            current.insert((row.namespace.clone(), row.id.clone()));
        }
        ids.insert((row.namespace, format!("artifact:{}", row.id)));
    }
    let staged = occupants(
        tx,
        &format!(
            "kind='artifact' AND {SCHEMA}='rsia.e16.staged_asset.v1' AND COALESCE(\
             json_extract(body,'$.metadata.payload.package_kind'),\
             json_extract(body,'$.payload.package_kind'))='skill'"
        ),
    )
    .await?;
    for row in staged {
        ids.insert((row.namespace.clone(), format!("artifact:{}", row.id)));
        let Some(candidate) = row.header()["payload"]["candidate_ref"].as_str() else {
            continue;
        };
        let edge: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dependencies WHERE namespace=? AND src_kind='artifact' AND src_id=? AND dst_kind='artifact' AND dst_id=?)")
            .bind(&row.namespace).bind(candidate).bind(&row.id).fetch_one(&mut **tx).await.map_err(internal)?;
        if edge != 0 && current.contains(&(row.namespace.clone(), candidate.to_owned())) {
            ids.remove(&(row.namespace.clone(), format!("artifact:{candidate}")));
        }
    }
    Ok(ids.len() as u64)
}

/// Materialized nodes plus claimed dispatches whose node is not written yet.
async fn exploration_nodes(tx: &mut Transaction<'_, Sqlite>) -> Result<u64> {
    let rows = occupants(
        tx,
        &format!(
            "kind='artifact' AND {SCHEMA}={EXPLORATION} \
             AND {RECORD_KIND} IN ('exploration_node_v1','exploration_dispatch_v1')"
        ),
    )
    .await?;
    let mut ids = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    let mut intents = Vec::new();
    for row in &rows {
        let header = row.header();
        let payload = &header["payload"];
        if header["record_kind"] != "exploration_dispatch_v1" {
            let node = format!(
                "node-{}-{}",
                payload["world_id"].as_str().unwrap_or(""),
                payload["node"]["node_seq"].as_u64().unwrap_or(0)
            );
            nodes.insert((row.namespace.as_str(), node));
            ids.insert((row.namespace.as_str(), row.id.clone()));
        } else if payload["state"] == "claimed" {
            let node = payload["node_id"]
                .as_str()
                .or_else(|| payload["id"].as_str());
            intents.push((row.namespace.as_str(), node.unwrap_or(&row.id).to_owned()));
        }
    }
    for intent in intents {
        if !nodes.contains(&intent) {
            ids.insert(intent);
        }
    }
    Ok(ids.len() as u64)
}

/// Both projections of one development cycle name its observed report.
async fn development_cycles(tx: &mut Transaction<'_, Sqlite>) -> Result<u64> {
    let rows = occupants(
        tx,
        &format!(
            "kind='artifact' AND ({SCHEMA} IN ('rsia.monitoring.development_cycle.v1',\
             'rsia.monitoring.development_cycle.v2') OR ({SCHEMA}={CURRICULUM} \
             AND {RECORD_KIND}='development_cycle_receipt_v1'))"
        ),
    )
    .await?;
    Ok(distinct(rows.iter().map(|row| {
        let header = row.header();
        let report = header["report_fact_id"]
            .as_str()
            .or_else(|| header["payload"]["development_observed_fact_id"].as_str());
        (row.namespace.as_str(), report.unwrap_or(&row.id))
    })))
}

/// Failure records are embedded in learner states and identified once.
async fn failure_records(tx: &mut Transaction<'_, Sqlite>) -> Result<u64> {
    let rows = occupants(
        tx,
        &format!("kind='artifact' AND {SCHEMA}={CURRICULUM} AND {RECORD_KIND}='learner_state_v2'"),
    )
    .await?;
    let mut ids = BTreeSet::new();
    for row in &rows {
        if row.value.is_none() {
            ids.insert((row.namespace.as_str(), row.id.as_str()));
        }
        for failure in row.header()["payload"]["failure_records"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(id) = failure["record_id"].as_str() {
                ids.insert((row.namespace.as_str(), id));
            }
        }
    }
    Ok(ids.len() as u64)
}

/// Replay worlds live in their own table; a sealed manifest may be redacted.
async fn replay_worlds(tx: &mut Transaction<'_, Sqlite>) -> Result<u64> {
    let rows = sqlx::query("SELECT namespace,id,manifest FROM replay_worlds")
        .fetch_all(&mut **tx)
        .await
        .map_err(internal)?;
    let mut used = 0;
    for row in rows {
        let namespace: String = row.try_get("namespace").map_err(internal)?;
        let id: String = row.try_get("id").map_err(internal)?;
        let raw: String = row.try_get("manifest").map_err(internal)?;
        let lawful = match crate::lifecycle::parse_strict_json(&raw) {
            Ok(value) => lawfully_redacted(tx, &namespace, "replay_world", &id, "", &value).await?,
            Err(_) => false,
        };
        used += u64::from(!lawful);
    }
    Ok(used)
}

/// One declared group of the server scope a current first claim persists.
#[derive(Deserialize)]
struct GroupScope {
    episode_id: String,
    request_id: String,
    step: u64,
    attempt: u64,
    input_digest: String,
}

/// Jobs whose server scope is not closed yet. A legacy v1 claim has no server
/// scope: it occupies the slot only while a live lease still runs for it.
async fn active_optimization_jobs(tx: &mut Transaction<'_, Sqlite>) -> Result<u64> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(&format!(
        "SELECT namespace,id,body FROM objects WHERE kind='artifact' AND {SCHEMA} IN \
         ('rsia.optimization.stage_fact.v1','rsia.optimization.stage_fact.v2') AND \
         COALESCE(json_extract(body,'$.metadata.kind'),json_extract(body,'$.kind'))='step_prepared'"
    ))
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    let now = evo_core::now()?;
    let mut active = 0;
    let mut jobs = BTreeMap::<(String, String), Option<Value>>::new();
    for (namespace, id, raw) in rows {
        let Ok(value) = crate::lifecycle::parse_strict_json(&raw) else {
            jobs.insert((namespace, id), None);
            continue;
        };
        let redacted = value["schema_version"] == "rsia.redacted.v1";
        let header = if redacted { &value["metadata"] } else { &value };
        let schema = if redacted {
            &value["original_schema"]
        } else {
            &value["schema_version"]
        };
        if schema == "rsia.optimization.stage_fact.v1" {
            if let (Some(episode), Some(step), Some(attempt), Some(request)) = (
                header["episode_id"].as_str(),
                header["step"].as_u64(),
                header["attempt"].as_u64(),
                header["request_id"].as_str(),
            ) && ledger(tx, &namespace, &id, (episode, step, attempt, request), now)
                .await?
                .is_some_and(|ledger| ledger.live != 0)
            {
                active += 1;
            }
            continue;
        }
        let scope = if redacted {
            &header["job_scope"]
        } else {
            &value["payload"]["job_scope"]
        };
        let key = (
            namespace,
            scope["job_id"].as_str().map_or(id, str::to_owned),
        );
        match jobs.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Some(scope.clone()));
            }
            // Members of one job persist one scope; a disagreeing one keeps it open.
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if entry.get().as_ref() != Some(scope) {
                    entry.insert(None);
                }
            }
        }
    }
    for ((namespace, _), scope) in jobs {
        let groups = scope.as_ref().and_then(|scope| {
            serde_json::from_value::<Vec<GroupScope>>(scope["groups"].clone()).ok()
        });
        let (Some(scope), Some(groups)) = (scope, groups) else {
            active += 1;
            continue;
        };
        for group in &groups {
            if !group_closed(tx, &namespace, &scope, group, now).await? {
                active += 1;
                break;
            }
        }
    }
    Ok(active)
}

/// A group is closed by any terminal fact once its paid execution is
/// reconciled, or by a completed cleanup that cancelled it before dispatch.
/// A step run for an exploration dispatch or a consolidation claim (whose id
/// is the step's episode) also ends with that owner's terminal record: the
/// coordinator never resumes a step it settled.
async fn group_closed(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    scope: &Value,
    group: &GroupScope,
    now: i64,
) -> Result<bool> {
    let id = format!(
        "optstage-{}",
        evo_core::fingerprint(&(
            "rsia.optimization.stage_fact.v2",
            namespace,
            &group.episode_id,
            group.step,
            group.attempt,
            "merge",
            "step_prepared",
            &group.request_id,
        ))?
    );
    let Some((owner, raw)) = sqlx::query_as::<_, (String, String)>(
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
    let Ok(prepared) = crate::lifecycle::parse_strict_json(&raw) else {
        return Ok(false);
    };
    let redacted = prepared["schema_version"] == "rsia.redacted.v1";
    let (schema, header, persisted_scope) = if redacted {
        let metadata = &prepared["metadata"];
        (
            &prepared["original_schema"],
            metadata,
            &metadata["job_scope"],
        )
    } else {
        (
            &prepared["schema_version"],
            &prepared,
            &prepared["payload"]["job_scope"],
        )
    };
    if schema != "rsia.optimization.stage_fact.v2"
        || header["input_digest"] != group.input_digest.as_str()
        || persisted_scope != scope
    {
        return Ok(false);
    }
    let key = (
        group.episode_id.as_str(),
        group.step,
        group.attempt,
        group.request_id.as_str(),
    );
    let Some(ledger) = ledger(tx, namespace, &id, key, now).await? else {
        return Ok(false);
    };
    let terminal: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM objects WHERE namespace=? AND kind='artifact' AND (
          (json_extract(body,'$.schema_version') IN ('rsia.optimization.stage_fact.v1','rsia.optimization.stage_fact.v2') AND json_extract(body,'$.kind')='step_completed' AND json_extract(body,'$.episode_id')=?2 AND json_extract(body,'$.request_id')=?3 AND json_extract(body,'$.input_digest')=?4 AND json_extract(body,'$.step')=?5 AND json_extract(body,'$.attempt')=?6) OR
          (json_extract(body,'$.schema_version')='rsia.redacted.v1' AND json_extract(body,'$.original_schema') IN ('rsia.optimization.stage_fact.v1','rsia.optimization.stage_fact.v2') AND json_extract(body,'$.metadata.kind')='step_completed' AND json_extract(body,'$.metadata.episode_id')=?2 AND json_extract(body,'$.metadata.request_id')=?3 AND json_extract(body,'$.metadata.input_digest')=?4 AND json_extract(body,'$.metadata.step')=?5 AND json_extract(body,'$.metadata.attempt')=?6) OR
          (json_extract(body,'$.schema_version')='rsia.exploration_artifact_envelope.v1' AND json_extract(body,'$.record_kind')='exploration_dispatch_v1' AND json_extract(body,'$.payload.world_id')=?2 AND json_extract(body,'$.payload.idempotency_key')=?3 AND json_extract(body,'$.payload.state') IN ('observed','uncertain')) OR
          (json_extract(body,'$.schema_version')='rsia.monitoring.consolidation_run.v1' AND json_extract(body,'$.claim_id')=?2)))")
        .bind(namespace).bind(&group.episode_id).bind(&group.request_id).bind(&group.input_digest)
        .bind(group.step as i64).bind(group.attempt as i64)
        .fetch_one(&mut **tx).await.map_err(internal)?;
    if terminal == 0 {
        // A redacted label or an expired lease proves no cancellation.
        return Ok(redacted
            && ledger.facts.is_empty()
            && ledger.calls == 0
            && lawfully_redacted(tx, namespace, "artifact", &id, &owner, &prepared).await?);
    }
    Ok(ledger.unfinished == 0
        && receipts_closed(tx, namespace, &group.episode_id, &ledger.facts).await?)
}

/// The facts recorded against one first claim and the paid calls they name.
struct Ledger {
    facts: Vec<Value>,
    calls: i64,
    unfinished: i64,
    live: i64,
}

/// Recovery facts keep the exact first claim as an edge; an episode alone
/// also holds other steps and cannot establish ownership. `None` when a
/// fact on that edge names another step.
async fn ledger(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    prepared: &str,
    (episode, step, attempt, request): (&str, u64, u64, &str),
    now: i64,
) -> Result<Option<Ledger>> {
    let rows: Vec<String> = sqlx::query_scalar(r#"SELECT f.body FROM dependencies d
        JOIN objects f ON f.namespace=d.namespace AND f.kind=d.src_kind AND f.id=d.src_id
        WHERE d.namespace=? AND d.src_kind='artifact' AND d.dst_kind='artifact' AND d.dst_id=?
          AND COALESCE(json_extract(f.body,'$.schema_version'), '') IN ('rsia.optimization.stage_fact.v1','rsia.optimization.stage_fact.v2','rsia.redacted.v1')
          AND COALESCE(json_extract(f.body,'$.kind'),json_extract(f.body,'$.metadata.kind')) IN ('dispatch_prepared','dispatch_observed','development_observed')"#)
        .bind(namespace).bind(prepared).fetch_all(&mut **tx).await.map_err(internal)?;
    let mut facts = Vec::with_capacity(rows.len());
    let mut requests = BTreeSet::from([request.to_owned()]);
    let mut calls = BTreeSet::new();
    let mut usage = BTreeSet::new();
    for raw in rows {
        let Ok(value) = crate::lifecycle::parse_strict_json(&raw) else {
            return Ok(None);
        };
        let redacted = value["schema_version"] == "rsia.redacted.v1";
        let header = if redacted { &value["metadata"] } else { &value };
        let Some(request) = header["request_id"].as_str() else {
            return Ok(None);
        };
        if header["namespace"] != namespace
            || header["episode_id"] != episode
            || header["step"] != step
            || header["attempt"] != attempt
        {
            return Ok(None);
        }
        requests.insert(request.to_owned());
        let payload = if redacted {
            &header["historical_facts"]
        } else {
            &value["payload"]
        };
        for receipt in [
            &payload["execution_receipt"],
            &payload["dispatch"]["receipt"],
        ] {
            calls.extend(receipt["call_id"].as_str().map(str::to_owned));
            usage.extend(receipt["usage_record_id"].as_str().map(str::to_owned));
        }
        for id in payload["usage_record_ids"].as_array().into_iter().flatten() {
            usage.extend(id.as_str().map(str::to_owned));
        }
        facts.push(value);
    }
    let json = |ids: &BTreeSet<String>| serde_json::to_string(ids).map_err(internal);
    let accounting = sqlx::query(r#"SELECT COUNT(*) AS calls,
            COALESCE(SUM(CASE WHEN state IN ('reserved','dispatched','uncertain') OR execution_closed=0
                OR (state='finalized' AND (actual_cost_micros IS NULL OR usage_record_id IS NULL)) THEN 1 ELSE 0 END),0) AS unfinished,
            COALESCE(SUM(CASE WHEN lease_until>?5 AND (state IN ('reserved','dispatched','uncertain')
                OR (state='finalized' AND execution_closed=0)) THEN 1 ELSE 0 END),0) AS live
            FROM root_budget_calls WHERE namespace=?1 AND (
                call_id IN (SELECT value FROM json_each(?2))
                OR COALESCE(json_extract(request_artifact_body,'$.request_id'),json_extract(request_artifact_body,'$.request.request_id'),json_extract(request_artifact_body,'$.metadata.request_id')) IN (SELECT value FROM json_each(?2))
                OR call_id IN (SELECT value FROM json_each(?3))
                OR usage_record_id IN (SELECT value FROM json_each(?4)))"#)
        .bind(namespace).bind(json(&requests)?).bind(json(&calls)?).bind(json(&usage)?).bind(now)
        .fetch_one(&mut **tx).await.map_err(internal)?;
    Ok(Some(Ledger {
        facts,
        calls: accounting.try_get("calls").map_err(internal)?,
        unfinished: accounting.try_get("unfinished").map_err(internal)?,
        live: accounting.try_get("live").map_err(internal)?,
    }))
}

/// Every external provider dispatch a group observed is finalized, charged
/// and execution-closed in the root ledger. Fixture dispatches carry no bill.
async fn receipts_closed(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    episode: &str,
    facts: &[Value],
) -> Result<bool> {
    for value in facts {
        let (header, payload) = if value["schema_version"] == "rsia.redacted.v1" {
            (&value["metadata"], &value["metadata"]["historical_facts"])
        } else {
            (value, &value["payload"])
        };
        if header["kind"] != "dispatch_observed" {
            continue;
        }
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
        let (Some(call), Some(dispatch)) =
            (receipt["call_id"].as_str(), receipt["dispatch_id"].as_str())
        else {
            return Ok(false);
        };
        let closed:i64=sqlx::query_scalar("SELECT COUNT(*) FROM root_budget_calls WHERE namespace=? AND call_id=? AND dispatch_id=? AND dispatch_group_id=? AND state='finalized' AND execution_closed=1 AND actual_cost_micros IS NOT NULL AND usage_record_id=? AND provider_request_id=?")
          .bind(namespace).bind(call).bind(dispatch).bind(episode).bind(receipt["usage_record_id"].as_str()).bind(receipt["provider_request_id"].as_str()).fetch_one(&mut **tx).await.map_err(internal)?;
        if closed != 1 {
            return Ok(false);
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
          (json_extract(body,'$.schema_version')='rsia.redacted.v1' AND json_extract(body,'$.original_schema') IN ('rsia.monitoring.consolidation_proposal.v1','rsia.monitoring.consolidation_proposal.v2') AND json_extract(body,'$.metadata.claim_id')=?))")
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
    /// Each item reads only its own class of rows.
    pub async fn capacity_used(&mut self, item: CapacityItem) -> Result<u64> {
        use CapacityItem::*;
        if let Some(filter) = row_class(item) {
            let rows = occupants(&mut self.tx, &filter).await?;
            return Ok(distinct(
                rows.iter()
                    .map(|row| (row.namespace.as_str(), row.id.as_str())),
            ));
        }
        match item {
            ActiveLease => crate::count_active_leases(&mut self.tx, evo_core::now()?).await,
            ConcurrentManagementDispatch => Ok(crate::count_management_job_capacity(
                &mut self.tx,
                evo_core::now()?,
            )
            .await?
            .concurrent_dispatches),
            ReplayWorld => replay_worlds(&mut self.tx).await,
            Event => events(&mut self.tx).await,
            Skill => skills(&mut self.tx).await,
            ExplorationNode => exploration_nodes(&mut self.tx).await,
            DevelopmentCycle => development_cycles(&mut self.tx).await,
            FailureRecord => failure_records(&mut self.tx).await,
            ConcurrentOptimizationJob => active_optimization_jobs(&mut self.tx).await,
            // Trace export records keep a redacted history slot, so this uses
            // the same cross-namespace counter as the producer's own write
            // gate instead of the occupants() path, which frees lawful
            // redactions.
            TraceExport => crate::count_trace_exports_all_namespaces(&mut self.tx).await,
            // Prepare slots live in the Store guard; owner-scoped items are
            // admitted against their owner; the rest have no producer yet.
            _ => Ok(0),
        }
    }

    /// The producer supplies the actual missing identities, after idempotency
    /// and before its first persistent or outbound side effect.
    pub async fn admit_capacity(&mut self, intent: &[(CapacityItem, u64)]) -> Result<()> {
        for &(item, delta) in intent {
            if delta != 0 {
                let used = self.capacity_used(item).await?;
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
