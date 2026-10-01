//! The revocation gate of whatever is about to depend on a run or an artifact: a
//! source whose revocation committed refuses new access, derivation, export and
//! publication at once (plan §11, §11.3, §11.5, E08), while the cleanup that follows
//! reaches what depends on it one page at a time. So for a while after
//! `begin_revoke` the pool, learner state, input artifact, grant or staged package a
//! new dependent would stand on is still live, and only a node further up carries the
//! tombstone. Every entry point that creates or reads such a dependent therefore
//! judges what it depends on, and the upstream closure of it, in the session that
//! goes on to write, before anything is written.
//!
//! Three entry points run it, and share the rule:
//! - the submit check of a management request (`dispatch`, AG-032 and AG-044), which
//!   refuses with a `Conflict` that names the node;
//! - the grant of a source selection (`evidence::store_source_selection`), the same;
//! - the verification of the sources of an E16 package or seed install
//!   (`packages::verify_sources`, which every stage, read, export, handoff and reset
//!   of one goes through), which keeps its own answer, `Forbidden`, and its own
//!   digest check.
//!
//! The judgement is [`judge_dependency`] (each dependency, by its own kind) and then
//! [`judge_upstream`] (everything above them); they return what they found, so that
//! a caller answers in its own words. [`ensure_dependencies_live`] runs both and
//! answers with the `Conflict` of the submit check.

use evo_core::{Context, Error, Result};
use evo_storage::Session;
use serde_json::Value;

/// Schema of the tombstone the revocation cleanup leaves in place of a record whose
/// source was revoked (plan §11.5, E08).
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";

/// Nodes the upstream closure of a new request's dependencies may hold before the
/// request is refused for its size (plan §11: a derivation beyond the scale is
/// refused, never silently cut short; `capacity` has the same rule). The closure of
/// a real request is a few dozen nodes.
pub(crate) const MAX_UPSTREAM_CLOSURE_NODES: usize = 10_000;

/// How a run or an artifact was found revoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cause {
    /// The node carries a tombstone `begin_revoke` wrote for its own kind of source.
    Revoked,
    /// The node carries a tombstone `begin_revoke` never writes (it does not decode,
    /// or its source kind is neither `run` nor `artifact`). It cannot be matched to
    /// anything, so the node is treated as revoked: fail closed.
    Unreadable,
    /// The node is an artifact whose body the cleanup already replaced with an
    /// `rsia.redacted.v1` tombstone.
    Redacted,
}

/// The first run or artifact judged revoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Revoked {
    kind: String,
    id: String,
    cause: Cause,
    /// Found in the upstream closure of the dependencies, and not among the
    /// dependencies themselves.
    upstream: bool,
}

impl Revoked {
    fn new(kind: &str, id: &str, cause: Cause, upstream: bool) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
            cause,
            upstream,
        }
    }

    /// The refusal as the submit check words it: a `Conflict` that names the node
    /// (`kind id`) and says why, in the words of `subject`, the thing that is
    /// refused (a "management request", a "source grant").
    pub(crate) fn conflict(&self, subject: &str) -> Error {
        let Self {
            kind,
            id,
            cause,
            upstream,
        } = self;
        let lead = if *upstream {
            format!("{subject}'s dependency closure holds {kind} {id}, ")
        } else {
            format!("{subject} depends on {kind} {id}, ")
        };
        let reason = match cause {
            Cause::Revoked => "whose source was revoked",
            Cause::Unreadable => {
                "whose revocation tombstone cannot be read; the source is treated as revoked"
            }
            Cause::Redacted => "which was redacted because its source was revoked",
        };
        Error::Conflict(format!("{lead}{reason}"))
    }
}

/// Whether the dependency `(kind, id)` itself is revoked: the first stage of the
/// check, which reads in the session that goes on to write.
///
/// A dependency is revoked when it is a tombstoned source (`begin_revoke` writes a
/// tombstone under the source id, and the cleanup then deletes a run), or when the
/// cleanup already replaced it with an `rsia.redacted.v1` tombstone.
///
/// A tombstone is keyed by the id of its source alone, and a source is a run or an
/// artifact, so a run and an artifact that share an id share one tombstone. The
/// tombstone records which of the two it was written for (`source_kind`) and it
/// decides a dependency only when that is the dependency's own kind: a revoked run
/// does not refuse a live artifact that carries its id, nor the reverse. A tombstone
/// that cannot be read as one `begin_revoke` writes (it does not decode, or its source
/// kind is neither `run` nor `artifact`) cannot be matched to anything and fails
/// closed: the dependency is refused. A dependency of any other kind is not judged by
/// a tombstone, only by the redacted-body check.
///
/// A dependency that does not exist (and is not tombstoned) is not refused here: the
/// operation itself fails on it, as before.
pub(crate) async fn judge_dependency(
    ctx: &Context,
    session: &mut Session,
    kind: &str,
    id: &str,
) -> Result<Option<Revoked>> {
    if matches!(kind, "run" | "artifact")
        && let Some(body) = session.get::<Value>(ctx, "tombstone", id).await?
    {
        match tombstone_source_kind(body) {
            Some(source_kind) if source_kind == kind => {
                return Ok(Some(Revoked::new(kind, id, Cause::Revoked, false)));
            }
            // The tombstone belongs to a source of the other kind that shares this
            // id; this dependency is a different object.
            Some(_) => {}
            None => return Ok(Some(Revoked::new(kind, id, Cause::Unreadable, false))),
        }
    }
    // The cleanup deletes a revoked run instead of redacting it, and a run body can be
    // large: for a run only the tombstone matters.
    if kind == "run" {
        return Ok(None);
    }
    if session
        .get::<Value>(ctx, kind, id)
        .await?
        .as_ref()
        .is_some_and(is_redacted)
    {
        return Ok(Some(Revoked::new(kind, id, Cause::Redacted, false)));
    }
    Ok(None)
}

/// Whether anything above the dependencies is revoked: the second stage of the
/// check, the runs and artifacts every dependency depends on, directly or
/// transitively.
///
/// The cleanup reaches the nodes below a revoked source one page at a time, so for a
/// while after `begin_revoke` the pool, learner state or input artifact a new
/// dependent stands on is still live, and only a node further up carries the
/// tombstone. Each run or artifact of the closure is judged as a dependency is in the
/// first stage, by the same rule, and the finding names that node: a tombstone of its
/// own kind, a tombstone that cannot be read, or (for an artifact) a body the cleanup
/// already redacted. A closure of more than [`MAX_UPSTREAM_CLOSURE_NODES`] nodes is
/// the `Conflict` of `Session::upstream_closure`, not a finding: judging part of it
/// would take a revoked source nobody looked at for a live one.
pub(crate) async fn judge_upstream(
    ctx: &Context,
    session: &mut Session,
    dependencies: &[(String, String)],
) -> Result<Option<Revoked>> {
    let closure = session
        .upstream_closure(ctx, dependencies, MAX_UPSTREAM_CLOSURE_NODES)
        .await?;
    for node in closure {
        let (kind, id) = (node.kind, node.id);
        if let Some(body) = node.tombstone {
            match tombstone_source_kind(body) {
                Some(source_kind) if source_kind == kind => {
                    return Ok(Some(Revoked::new(&kind, &id, Cause::Revoked, true)));
                }
                // The tombstone belongs to a source of the other kind that shares
                // this id; this node is a different object.
                Some(_) => {}
                None => return Ok(Some(Revoked::new(&kind, &id, Cause::Unreadable, true))),
            }
        }
        if node.redacted {
            return Ok(Some(Revoked::new(&kind, &id, Cause::Redacted, true)));
        }
    }
    Ok(None)
}

/// `Conflict` naming the first thing a new `subject` depends on whose source was
/// revoked, in two stages. Both read in the session that goes on to write, before
/// anything is written, so a refusal leaves no private input, job, grant, edge,
/// idempotency row or audit record behind.
///
/// 1. The dependencies themselves, in the order given ([`judge_dependency`]).
/// 2. The upstream closure of the dependencies that passed the first stage
///    ([`judge_upstream`]).
///
/// The words are those of the submit check, led by `subject`: "management request
/// depends on run r, whose source was revoked".
pub(crate) async fn ensure_dependencies_live(
    ctx: &Context,
    session: &mut Session,
    dependencies: &[(String, String)],
    subject: &str,
) -> Result<()> {
    for (kind, id) in dependencies {
        if let Some(revoked) = judge_dependency(ctx, session, kind, id).await? {
            return Err(revoked.conflict(subject));
        }
    }
    ensure_upstream_closure_live(ctx, session, dependencies, subject).await
}

/// The second stage of [`ensure_dependencies_live`]: the runs and artifacts above the
/// dependencies, judged as the dependencies are.
async fn ensure_upstream_closure_live(
    ctx: &Context,
    session: &mut Session,
    dependencies: &[(String, String)],
    subject: &str,
) -> Result<()> {
    match judge_upstream(ctx, session, dependencies).await? {
        Some(revoked) => Err(revoked.conflict(subject)),
        None => Ok(()),
    }
}

/// The kind of source (`run` or `artifact`) a stored tombstone says it was written
/// for; `None` for a body `begin_revoke` never writes (it does not decode, or its
/// source kind is neither).
pub(crate) fn tombstone_source_kind(body: Value) -> Option<String> {
    serde_json::from_value::<evo_storage::lifecycle::RevokeTombstone>(body)
        .ok()
        .map(|tombstone| tombstone.source_kind)
        .filter(|source_kind| matches!(source_kind.as_str(), "run" | "artifact"))
}

fn is_redacted(body: &Value) -> bool {
    body.get("schema_version").and_then(Value::as_str) == Some(REDACTED_SCHEMA)
}
