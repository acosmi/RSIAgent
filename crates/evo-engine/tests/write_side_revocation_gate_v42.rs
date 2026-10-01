//! AG-046 (E08, plan §11, §11.3 and §11.5; V017, V069, V075): the entry points the
//! submit-time gate of AG-044 left out refuse a revoked source too, and write
//! nothing when they do. Real SQLite store, no model, no provider, zero monetary
//! cost.
//!
//! `begin_revoke` writes the tombstone and the watermark in one transaction, so a
//! source is revoked from the moment it commits (plan §11: a revocation refuses new
//! access at once). The cleanup that follows reaches what depends on the source one
//! page at a time. Until it does, and for an artifact whose own body the cleanup has
//! already replaced, three entry points let a new object come into being over the
//! revoked source:
//!
//! - the grant of a source selection (`store_source_selection`) did not look at the
//!   tombstone of any of its runs at all: it stored the grant artifact and an edge
//!   to every run, in any state of the cleanup, `Complete` included;
//! - the staged package (`PersistentPackageStore::stage_package`) and the seed
//!   install (`PersistentSeedStore::install`) ran `verify_sources` over the
//!   references of the request, which looked at a tombstone under the id of each
//!   source whatever its kind, and at the digest of the source, but never at what the
//!   source itself depends on or at whether the cleanup had replaced its body. What
//!   stopped them was a later check of the storage layer, at blob publication, over
//!   the dependency closure of the new envelope (a tombstone under any id of it): it
//!   ran after the first transaction had committed the `Prepared` envelope, its
//!   edges and an audit row, so the call failed with `Forbidden` and left them
//!   behind, and it never refused a closure whose nodes the cleanup had redacted;
//! - every read of what exists (`read_staged`, `read_install`, and through them the
//!   handoff, the export and the seed reset) runs `verify_sources` and nothing else:
//!   an existing package over a revoked closure was read, exported and reset.
//!
//! All of them now run the judgement of the submit check (`revocation_gate`): each
//! dependency is judged by its own kind (a tombstone written for that kind, a
//! tombstone that cannot be read, a body the cleanup already redacted), and then the
//! upstream closure of the dependencies is judged node by node by the same rule,
//! with a bound of 10 000 nodes beyond which the answer is a refusal and not a
//! truncated walk. The grant refuses with a `Conflict` that names the node, as the
//! submit check does. `verify_sources` keeps its own answer: `Forbidden` for a
//! revoked source (a tombstone, an unreadable tombstone, a redacted body, a revoked
//! node of the closure) and `Conflict` for a source whose digest changed; a closure
//! over the bound is the `Conflict` of `Session::upstream_closure`. The refusal comes
//! before the first write.
//!
//! Not covered here (and not claimed): writes after the cleanup is `Complete` other
//! than these; physical residue; whether a granted run exists or is trustworthy (the
//! consumer, `StoreOptimizationJournal::verify_sources`, still decides that);
//! production callers (there are none for any of the three). The storage-layer check
//! at blob publication is unchanged and keeps its own rule, a tombstone under any id
//! of the closure whatever its kind: where it refuses what the gate accepts, the
//! writer still fails late, after its first commit (the `known_boundary_` tests pin
//! the places where that shows).
//!
//! The fixtures below are copied from `tests/submit_upstream_gate_v42.rs` (the
//! synthetic graph and the `curriculum.step` request), `tests/packages_v41.rs` (the
//! package request), `tests/seeds_v41.rs` (the install request) and
//! `tests/revocation_source_ids_v42.rs` (the trace authority and the import
//! registration); test crates cannot import each other, and the originals are
//! untouched.

use evo_core::evidence::{ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::dispatch::ManagementDispatcher;
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::import::{
    IMPORT_REGISTRATION_SCHEMA, ImportRegistrationRequest, ImportRetentionScope, ImportSourceSpec,
    PersistentImportService, SourceFormat,
};
use evo_engine::packages::{
    AssetPackageManifest, E16SourceRef, PACKAGE_SCHEMA_V1, PRIVACY_DISCLAIMER, PackageDependency,
    PackageEntry, PackageKind, PersistentPackageStore, RedactionReport, RedactionStatus,
    StagePackageRequest, StagedAssetState, package_dependency_ref_id,
};
use evo_engine::seeds::{InstallSeedRequest, PersistentSeedStore};
use evo_storage::Store;
use evo_storage::lifecycle::{
    CleanupState, CleanupStatus, LifecycleStore, REVOKE_TOMBSTONE_SCHEMA, RevokeTombstone,
    TypedObjectRef,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

use common::*;
use e16_fx::*;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

mod common {
    use super::*;

    pub const REDACTED: &str = "rsia.redacted.v1";

    /// The bound of the upstream closure the gate walks (plan §11: a derivation
    /// beyond the scale is refused, never silently cut short).
    pub const CLOSURE_BOUND: usize = 10_000;

    /// Every kind the `objects` table accepts: a snapshot of the store reads them
    /// all, so a refused call cannot hide a write under a kind nobody looked at.
    const OBJECT_KINDS: [&str; 15] = [
        "run",
        "feedback",
        "event",
        "candidate",
        "release",
        "pointer",
        "receipt",
        "dataset",
        "evaluation",
        "budget",
        "reservation",
        "job",
        "improvement",
        "artifact",
        "tombstone",
    ];

    pub fn host() -> Context {
        Context::new("n", "host", Role::Host).unwrap()
    }

    pub fn admin() -> Context {
        Context::new("n", "admin", Role::Admin).unwrap()
    }

    /// A store in a temporary directory, with the revoke watermark of namespace
    /// `n` bumped once: every E16 operation reads it.
    pub struct Fx {
        pub dir: tempfile::TempDir,
        pub store: Store,
    }

    impl Fx {
        pub async fn open(name: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join(name)).await.unwrap();
            let fx = Self { dir, store };
            fx.bump(&admin(), "ag046-initial-watermark").await;
            fx
        }

        pub async fn bump(&self, ctx: &Context, label: &str) {
            let mut session = self.store.session().await.unwrap();
            session
                .bump_watermark(ctx, &hash(label.as_bytes()))
                .await
                .unwrap();
            session.commit().await.unwrap();
        }

        /// The stored body of one object, straight from the store, so the
        /// assertions do not depend on any consumer's read path.
        pub async fn raw(&self, ctx: &Context, kind: &str, id: &str) -> Option<Value> {
            let mut session = self.store.session().await.unwrap();
            let value = session.get(ctx, kind, id).await.unwrap();
            session.commit().await.unwrap();
            value
        }

        pub async fn put(&self, ctx: &Context, kind: &str, id: &str, body: &Value) {
            let mut session = self.store.session().await.unwrap();
            session.put(ctx, kind, id, "admin", body).await.unwrap();
            session.commit().await.unwrap();
        }

        /// Edges `(src kind, src id, dst kind, dst id)`.
        pub async fn edges(&self, ctx: &Context, edges: &[(&str, &str, &str, &str)]) {
            let mut session = self.store.session().await.unwrap();
            for (src_kind, src_id, dst_kind, dst_id) in edges {
                session
                    .put_edge(ctx, src_kind, src_id, dst_kind, dst_id)
                    .await
                    .unwrap();
            }
            session.commit().await.unwrap();
        }

        /// Every `artifact` body whose schema is `schema`.
        pub async fn artifacts_of(&self, ctx: &Context, schema: &str) -> Vec<Value> {
            let mut session = self.store.session().await.unwrap();
            let all: Vec<Value> = session.list(ctx, "artifact").await.unwrap();
            session.commit().await.unwrap();
            all.into_iter()
                .filter(|body| body["schema_version"] == schema)
                .collect()
        }

        /// Every file under `blobs/` and `exports/` of the store, relative to the
        /// store's directory.
        pub fn files(&self) -> Vec<String> {
            fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    return;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(root, &path, out);
                    } else {
                        out.push(
                            path.strip_prefix(root)
                                .unwrap()
                                .to_string_lossy()
                                .into_owned(),
                        );
                    }
                }
            }
            let mut out = Vec::new();
            for directory in ["blobs", "exports"] {
                walk(self.dir.path(), &self.dir.path().join(directory), &mut out);
            }
            out.sort();
            out
        }
    }

    pub fn live(id: &str) -> Value {
        json!({"id": id, "schema_version": "fixture.live.v1"})
    }

    pub fn redacted(id: &str) -> Value {
        json!({"id": id, "schema_version": REDACTED})
    }

    /// A tombstone as `begin_revoke` writes it, for a source of `source_kind`.
    pub fn tombstone(id: &str, source_kind: &str) -> Value {
        serde_json::to_value(RevokeTombstone {
            id: id.into(),
            schema_version: REVOKE_TOMBSTONE_SCHEMA.into(),
            source_kind: source_kind.into(),
            reason: "test".into(),
            watermark_seq: 1,
            watermark_digest: "a".repeat(64),
            created_at: 1,
        })
        .unwrap()
    }

    /// A tombstone body `begin_revoke` never writes.
    pub fn corrupt(id: &str) -> Value {
        json!({"id": id})
    }

    /// A Host-issued trace authority (copied from `tests/revocation_source_ids_v42.rs`).
    pub fn authority(id: &str) -> StoredTraceAuthority {
        StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: id.into(),
                body: id.as_bytes().to_vec(),
                parent_family: "family-a".into(),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: OptimizationTrace {
                run_id: id.into(),
                parent_family: "family-a".into(),
                source_digest: hash(id.as_bytes()),
                purpose: Purpose::Development,
                outcome: TraceOutcome::Success,
                diagnosis: None,
                excerpt: id.into(),
                seed: 1,
            },
            excerpt_start: 0,
            excerpt_end: id.len(),
        }
    }

    pub async fn store_run(fx: &Fx, id: &str) {
        store_trace_authority(&fx.store, &host(), &authority(id))
            .await
            .unwrap();
    }

    pub fn selection(runs: &[&str]) -> SourceSelection {
        SourceSelection {
            roots: vec![],
            run_ids: runs.iter().map(|run| (*run).into()).collect(),
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        }
    }

    /// Id of the grant artifact of a selection; it depends on every run of it.
    pub fn grant_id(selection: &SourceSelection) -> String {
        format!("optgrant-{}", fingerprint(selection).unwrap())
    }

    pub async fn begin(fx: &Fx, ctx: &Context, kind: &str, id: &str) -> CleanupStatus {
        LifecycleStore::begin_revoke(
            ctx,
            &fx.store,
            TypedObjectRef {
                kind: kind.into(),
                id: id.into(),
            },
            "source revoked",
            500,
        )
        .await
        .unwrap()
    }

    /// One cleanup step of `limit` edges; `now` is the step's clock.
    pub async fn step(
        fx: &Fx,
        ctx: &Context,
        status: &CleanupStatus,
        limit: usize,
        now: i64,
    ) -> CleanupStatus {
        LifecycleStore::cleanup_step(ctx, &fx.store, &status.job_id, limit, now)
            .await
            .unwrap()
    }

    pub fn finished(status: &CleanupStatus) -> bool {
        matches!(status.state, CleanupState::Complete | CleanupState::Failed)
    }

    /// What a refused call must leave untouched: the bodies of every object of the
    /// namespace, every edge into the named nodes, the audit chain, the revoke
    /// watermark and the files under `blobs/` and `exports/`. (None of these calls
    /// writes an idempotency row: they have no request-key cache.)
    #[derive(Debug, PartialEq)]
    pub struct Shape {
        objects: Vec<(&'static str, Vec<Value>)>,
        dependents: Vec<Vec<(String, String)>>,
        audit: usize,
        watermark: Option<(i64, String)>,
        files: Vec<String>,
    }

    impl Shape {
        /// What differs from `after`, in words (empty when nothing does).
        pub fn changes(&self, after: &Shape) -> String {
            let mut parts = Vec::new();
            for ((kind, before), (_, after)) in self.objects.iter().zip(&after.objects) {
                if before != after {
                    parts.push(format!(
                        "{kind} objects {} -> {}",
                        before.len(),
                        after.len()
                    ));
                }
            }
            for (index, (before, after)) in
                self.dependents.iter().zip(&after.dependents).enumerate()
            {
                if before != after {
                    parts.push(format!(
                        "edges into node {index} {} -> {}",
                        before.len(),
                        after.len()
                    ));
                }
            }
            if self.audit != after.audit {
                parts.push(format!("audit rows {} -> {}", self.audit, after.audit));
            }
            if self.watermark != after.watermark {
                parts.push("revoke watermark moved".into());
            }
            if self.files != after.files {
                parts.push(format!(
                    "files {} -> {}",
                    self.files.len(),
                    after.files.len()
                ));
            }
            parts.join(", ")
        }
    }

    pub async fn shape(fx: &Fx, ctx: &Context, nodes: &[(&str, &str)]) -> Shape {
        let mut session = fx.store.session().await.unwrap();
        let mut objects = Vec::new();
        for kind in OBJECT_KINDS {
            let bodies: Vec<Value> = session.list(ctx, kind).await.unwrap();
            objects.push((kind, bodies));
        }
        let mut dependents = Vec::new();
        for (kind, id) in nodes {
            let mut found = session.dependents(ctx, kind, id).await.unwrap();
            found.sort();
            dependents.push(found);
        }
        let watermark = session.watermark(ctx).await.unwrap();
        session.commit().await.unwrap();
        let audit = fx.store.verify_audit(ctx).await.unwrap();
        Shape {
            objects,
            dependents,
            audit,
            watermark,
            files: fx.files(),
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture: the E16 package and install requests (copied from tests/packages_v41.rs
// and tests/seeds_v41.rs)
// ---------------------------------------------------------------------------

mod e16_fx {
    use super::*;

    pub fn manifest() -> AssetPackageManifest {
        let content1 = b"# Test Skill\nInstructions here\n";
        let content2 = b"{\"param\": 123}\n";
        AssetPackageManifest {
            schema_version: PACKAGE_SCHEMA_V1.into(),
            kind: PackageKind::Skill,
            publisher: "org.rsia".into(),
            asset_id: "test_asset".into(),
            name: "Test Asset".into(),
            version: "1.0.0".into(),
            description: "A test package for verification".into(),
            license: "Apache-2.0".into(),
            entries: vec![
                PackageEntry {
                    path: "skill.md".into(),
                    digest_sha256: hash(content1),
                    size_bytes: content1.len() as u64,
                    compressed_bytes: content1.len() as u64,
                },
                PackageEntry {
                    path: "config.json".into(),
                    digest_sha256: hash(content2),
                    size_bytes: content2.len() as u64,
                    compressed_bytes: content2.len() as u64,
                },
            ],
            dependencies: vec![],
            redaction_report: RedactionReport {
                scanned_at: 1000,
                findings_count: 0,
                status: RedactionStatus::Clean,
                disclaimer: PRIVACY_DISCLAIMER.into(),
                findings: vec![],
            },
            foreign_metadata: None,
        }
    }

    pub fn files() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            (
                "skill.md".into(),
                b"# Test Skill\nInstructions here\n".to_vec(),
            ),
            ("config.json".into(), b"{\"param\": 123}\n".to_vec()),
        ])
    }

    pub fn stage_request_for(
        key: &str,
        manifest: AssetPackageManifest,
        sources: Vec<E16SourceRef>,
    ) -> StagePackageRequest {
        StagePackageRequest {
            request_key: key.into(),
            manifest,
            files: files(),
            source_refs: sources,
            baseline_digest: hash(b"baseline"),
            local_digest: hash(b"local"),
            upstream_digest: Some(hash(b"upstream")),
            environment_digest: environment(),
            compiler_version: "compiler-v1".into(),
            candidate_material: None,
        }
    }

    pub fn stage_request(key: &str, sources: Vec<E16SourceRef>) -> StagePackageRequest {
        stage_request_for(key, manifest(), sources)
    }

    pub fn install_request(
        key: &str,
        asset: &str,
        sources: Vec<E16SourceRef>,
    ) -> InstallSeedRequest {
        InstallSeedRequest {
            request_key: key.into(),
            publisher: "org.rsia".into(),
            asset_id: asset.into(),
            kind: "skill".into(),
            baseline_bytes: b"baseline seed".to_vec(),
            local_bytes: b"local seed".to_vec(),
            upstream_bytes: Some(b"upstream seed".to_vec()),
            environment_digest: environment(),
            source_refs: sources,
        }
    }

    pub fn environment() -> String {
        hash(b"environment")
    }

    /// A reference to the object of this kind and id as it is stored now.
    pub async fn source_ref(fx: &Fx, ctx: &Context, kind: &str, id: &str) -> E16SourceRef {
        let body = fx.raw(ctx, kind, id).await.unwrap();
        E16SourceRef {
            kind: kind.into(),
            id: id.into(),
            digest: fingerprint(&body).unwrap(),
        }
    }
}

// ---------------------------------------------------------------------------
// One table for every entry point. The grant names a run (`run-y`); the others name
// an artifact (`src-art`): the two writers in the request, the two reads through
// the package or the install that was stored over it before the case began. Each
// case hangs nodes, edges and tombstones below that root and says what every entry
// point must answer: the same verdict, because they judge by one rule.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Grant,
    Stage,
    Install,
    ReadStaged,
    ReadInstall,
}

const ENTRIES: [Entry; 5] = [
    Entry::Grant,
    Entry::Stage,
    Entry::Install,
    Entry::ReadStaged,
    Entry::ReadInstall,
];

/// The entry points the gate decides alone. The grant has no other check and the
/// two reads look at nothing but `verify_sources`; `stage_package` and `install`
/// also pass the storage check at blob publication, which refuses by the id of a
/// tombstone whatever its kind (`known_boundary_` below).
const GATE_ONLY: &[Entry] = &[Entry::Grant, Entry::ReadStaged, Entry::ReadInstall];

/// Entry points that name an artifact.
const ARTIFACT_ROOTS: &[Entry] = &[
    Entry::Stage,
    Entry::Install,
    Entry::ReadStaged,
    Entry::ReadInstall,
];

impl Entry {
    /// The node the entry point's source reference names.
    fn root(self) -> (&'static str, &'static str) {
        match self {
            Entry::Grant => ("run", "run-y"),
            _ => ("artifact", "src-art"),
        }
    }
}

/// A store and, for the two reads, the id of the package or install that exists.
struct Setup {
    fx: Fx,
    existing: Option<String>,
}

/// The store a case starts from: the root exists and is live, and the reads have
/// something stored over it.
async fn prepare(entry: Entry, name: &str) -> Setup {
    let fx = Fx::open(name).await;
    let admin = admin();
    let mut existing = None;
    if entry == Entry::Grant {
        store_run(&fx, "run-y").await;
    } else {
        fx.put(&admin, "artifact", "src-art", &live("src-art"))
            .await;
        let sources = vec![source_ref(&fx, &admin, "artifact", "src-art").await];
        match entry {
            Entry::ReadStaged => {
                existing = Some(
                    PersistentPackageStore::stage_package(
                        &admin,
                        &fx.store,
                        stage_request("existing-stage", sources),
                    )
                    .await
                    .unwrap()
                    .id,
                );
            }
            Entry::ReadInstall => {
                existing = Some(
                    PersistentSeedStore::install(
                        &admin,
                        &fx.store,
                        install_request("existing-install", "existing-seed", sources),
                    )
                    .await
                    .unwrap()
                    .id,
                );
            }
            _ => {}
        }
    }
    Setup { fx, existing }
}

/// One call of the entry point over its root, in `namespace`.
async fn attempt(entry: Entry, setup: &Setup, namespace: &str) -> Result<(), Error> {
    let fx = &setup.fx;
    let host = Context::new(namespace, "host", Role::Host).unwrap();
    let admin = Context::new(namespace, "admin", Role::Admin).unwrap();
    let (kind, id) = entry.root();
    match entry {
        Entry::Grant => store_source_selection(&fx.store, &host, &selection(&[id])).await,
        Entry::Stage => {
            let sources = vec![source_ref(fx, &admin, kind, id).await];
            PersistentPackageStore::stage_package(
                &admin,
                &fx.store,
                stage_request("case-stage", sources),
            )
            .await
            .map(|_| ())
        }
        Entry::Install => {
            let sources = vec![source_ref(fx, &admin, kind, id).await];
            PersistentSeedStore::install(
                &admin,
                &fx.store,
                install_request("case-install", "case-seed", sources),
            )
            .await
            .map(|_| ())
        }
        Entry::ReadStaged => PersistentPackageStore::read_staged(
            &admin,
            &fx.store,
            setup.existing.as_deref().unwrap(),
        )
        .await
        .map(|_| ()),
        Entry::ReadInstall => {
            PersistentSeedStore::read_install(&admin, &fx.store, setup.existing.as_deref().unwrap())
                .await
                .map(|_| ())
        }
    }
}

#[derive(Debug)]
enum Verdict {
    Accepted,
    /// A revoked node: the grant answers with a `Conflict` that holds every one of
    /// these parts, every other entry point with `Forbidden`.
    Revoked(&'static [&'static str]),
    /// A closure over the bound: a `Conflict` from every entry point that names the
    /// bound and does not name the last node a truncated walk would never reach.
    TooBig,
}

/// Runs the entry point once and compares the answer with the verdict; returns what
/// differs (nothing when it matches), so that a table reports every row it fails and
/// not only the first. A refusal must also have written nothing.
async fn judge(entry: Entry, setup: &Setup, name: &str, verdict: &Verdict) -> Vec<String> {
    let fx = &setup.fx;
    let admin = admin();
    let nodes = [entry.root()];
    let before = shape(fx, &admin, &nodes).await;
    let result = attempt(entry, setup, "n").await;
    let mut problems = Vec::new();
    match (verdict, result) {
        (Verdict::Accepted, Ok(())) => return problems,
        (Verdict::Accepted, Err(error)) => {
            problems.push(format!(
                "{entry:?} / {name}: refused, expected accepted: {error:?}"
            ));
        }
        (Verdict::Revoked(parts), Err(Error::Conflict(message))) if entry == Entry::Grant => {
            for part in parts.iter() {
                if !message.contains(part) {
                    problems.push(format!("{entry:?} / {name}: `{message}` lacks `{part}`"));
                }
            }
        }
        (Verdict::Revoked(_), Err(Error::Forbidden)) if entry != Entry::Grant => {}
        (Verdict::Revoked(_), Err(other)) => {
            let expected = if entry == Entry::Grant {
                "a naming Conflict"
            } else {
                "Forbidden"
            };
            problems.push(format!(
                "{entry:?} / {name}: expected {expected}, got {other:?}"
            ));
        }
        (Verdict::Revoked(_) | Verdict::TooBig, Ok(())) => {
            problems.push(format!("{entry:?} / {name}: accepted, expected a refusal"));
        }
        (Verdict::TooBig, Err(Error::Conflict(message))) => {
            if !message.contains("closure") || !message.contains("10000") {
                problems.push(format!(
                    "{entry:?} / {name}: `{message}` does not name the bound"
                ));
            }
            if message.contains("run tail") || message.contains("artifact tail") {
                problems.push(format!(
                    "{entry:?} / {name}: `{message}` names a node a truncated walk never judged"
                ));
            }
        }
        (Verdict::TooBig, Err(other)) => {
            problems.push(format!(
                "{entry:?} / {name}: expected a Conflict, got {other:?}"
            ));
        }
    }
    let changes = before.changes(&shape(fx, &admin, &nodes).await);
    if !changes.is_empty() {
        problems.push(format!(
            "{entry:?} / {name}: a refused call wrote ({changes})"
        ));
    }
    problems
}

/// What sits under the root's own id in the `tombstone` kind.
#[derive(Clone, Copy)]
enum OnRoot {
    /// A tombstone written for the root's own kind.
    Own,
    /// A tombstone written for the other of the two source kinds: it belongs to
    /// another object that shares the id.
    Other,
    /// A body `begin_revoke` never writes.
    Corrupt,
    /// A tombstone as `begin_revoke` writes it, for a kind it never writes one for.
    UnknownKind,
}

impl OnRoot {
    fn body(self, entry: Entry) -> Value {
        let (kind, id) = entry.root();
        match self {
            OnRoot::Own => tombstone(id, kind),
            OnRoot::Other => tombstone(id, if kind == "run" { "artifact" } else { "run" }),
            OnRoot::Corrupt => corrupt(id),
            OnRoot::UnknownKind => tombstone(id, "release"),
        }
    }
}

struct Case {
    name: &'static str,
    /// Objects stored besides the root: (kind, body).
    objects: Vec<(&'static str, Value)>,
    /// Edges (src kind, src id, dst kind, dst id); the src kind `ROOT` stands for
    /// the entry point's root.
    edges: Vec<(&'static str, &'static str, &'static str, &'static str)>,
    /// Tombstones, each keyed by the id of its source alone.
    tombstones: Vec<Value>,
    on_root: Option<OnRoot>,
    /// The stored body of the root is already redacted (an artifact root only).
    root_redacted: bool,
    entries: &'static [Entry],
    verdict: Verdict,
}

impl Case {
    fn new(name: &'static str, verdict: Verdict) -> Self {
        Self {
            name,
            objects: vec![],
            edges: vec![],
            tombstones: vec![],
            on_root: None,
            root_redacted: false,
            entries: &ENTRIES,
            verdict,
        }
    }

    fn object(mut self, kind: &'static str, body: Value) -> Self {
        self.objects.push((kind, body));
        self
    }

    fn edge(
        mut self,
        src_kind: &'static str,
        src_id: &'static str,
        dst_kind: &'static str,
        dst_id: &'static str,
    ) -> Self {
        self.edges.push((src_kind, src_id, dst_kind, dst_id));
        self
    }

    /// An edge from the root.
    fn below(self, dst_kind: &'static str, dst_id: &'static str) -> Self {
        self.edge("ROOT", "", dst_kind, dst_id)
    }

    fn tombstone(mut self, body: Value) -> Self {
        self.tombstones.push(body);
        self
    }

    fn on_root(mut self, on_root: OnRoot) -> Self {
        self.on_root = Some(on_root);
        self
    }

    fn root_redacted(mut self) -> Self {
        self.root_redacted = true;
        self.entries = ARTIFACT_ROOTS;
        self
    }

    fn only(mut self, entries: &'static [Entry]) -> Self {
        self.entries = entries;
        self
    }
}

async fn apply(entry: Entry, fx: &Fx, case: &Case) {
    let admin = admin();
    let (root_kind, root_id) = entry.root();
    for (kind, body) in &case.objects {
        fx.put(&admin, kind, body["id"].as_str().unwrap(), body)
            .await;
    }
    let mut session = fx.store.session().await.unwrap();
    for (src_kind, src_id, dst_kind, dst_id) in &case.edges {
        let (src_kind, src_id) = if *src_kind == "ROOT" {
            (root_kind, root_id)
        } else {
            (*src_kind, *src_id)
        };
        session
            .put_edge(&admin, src_kind, src_id, dst_kind, dst_id)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    if case.root_redacted {
        fx.put(&admin, root_kind, root_id, &redacted(root_id)).await;
    }
    if let Some(on_root) = case.on_root {
        fx.put(&admin, "tombstone", root_id, &on_root.body(entry))
            .await;
    }
    for body in &case.tombstones {
        fx.put(&admin, "tombstone", body["id"].as_str().unwrap(), body)
            .await;
    }
}

fn cases() -> Vec<Case> {
    use Verdict::{Accepted, Revoked};
    vec![
        // The root itself, judged by its own kind (stage one of the check).
        Case::new(
            "the root's own tombstone",
            Revoked(&["run run-y", "revoked"]),
        )
        .on_root(OnRoot::Own),
        Case::new(
            "a tombstone of the other kind under the root's id",
            Accepted,
        )
        .on_root(OnRoot::Other)
        .only(GATE_ONLY),
        Case::new(
            "a root whose tombstone does not decode",
            Revoked(&["run run-y", "cannot be read"]),
        )
        .on_root(OnRoot::Corrupt),
        Case::new(
            "a root whose tombstone names an unknown source kind",
            Revoked(&["run run-y", "cannot be read"]),
        )
        .on_root(OnRoot::UnknownKind),
        // Only an artifact is judged by its body, and a run never is.
        Case::new(
            "a root artifact the cleanup already redacted",
            Revoked(&["artifact src-art", "redacted"]),
        )
        .root_redacted(),
        // The upstream closure (stage two), node by node, by the node's own kind.
        Case::new(
            "a revoked run above the root",
            Revoked(&["run run-r1", "revoked"]),
        )
        .object("run", live("run-r1"))
        .below("run", "run-r1")
        .tombstone(tombstone("run-r1", "run")),
        Case::new(
            "a revoked run the cleanup already deleted",
            Revoked(&["run run-r2", "revoked"]),
        )
        .below("run", "run-r2")
        .tombstone(tombstone("run-r2", "run")),
        Case::new(
            "a revoked import source artifact above the root",
            Revoked(&["artifact art-a1", "revoked"]),
        )
        .object("artifact", live("art-a1"))
        .below("artifact", "art-a1")
        .tombstone(tombstone("art-a1", "artifact")),
        // The walk goes through nodes of every kind, not only runs and artifacts.
        Case::new(
            "a revoked run behind a live artifact, a release and a candidate",
            Revoked(&["run run-r3", "revoked"]),
        )
        .object("artifact", live("art-m1"))
        .object("release", live("rel-1"))
        .object("candidate", live("cand-1"))
        .object("run", live("run-r3"))
        .below("artifact", "art-m1")
        .edge("artifact", "art-m1", "release", "rel-1")
        .edge("release", "rel-1", "candidate", "cand-1")
        .edge("candidate", "cand-1", "run", "run-r3")
        .tombstone(tombstone("run-r3", "run")),
        Case::new(
            "a cycle that leads to a revoked run",
            Revoked(&["run run-r4", "revoked"]),
        )
        .object("run", live("run-r4"))
        .below("artifact", "art-c1")
        .edge("artifact", "art-c1", "artifact", "art-c2")
        .edge("artifact", "art-c2", "artifact", "art-c1")
        .edge("artifact", "art-c2", "run", "run-r4")
        .tombstone(tombstone("run-r4", "run")),
        Case::new(
            "a diamond that leads to a revoked run",
            Revoked(&["run run-r5", "revoked"]),
        )
        .object("run", live("run-r5"))
        .below("artifact", "art-d1")
        .below("artifact", "art-d2")
        .edge("artifact", "art-d1", "run", "run-r5")
        .edge("artifact", "art-d2", "run", "run-r5")
        .tombstone(tombstone("run-r5", "run")),
        // A tombstone is keyed by the id alone and records the kind of the source it
        // was written for: one written for the other kind is another object's.
        Case::new("a run whose id a revoked artifact shares", Accepted)
            .object("run", live("shared-1"))
            .below("run", "shared-1")
            .tombstone(tombstone("shared-1", "artifact"))
            .only(GATE_ONLY),
        Case::new("an artifact whose id a revoked run shares", Accepted)
            .object("artifact", live("shared-2"))
            .below("artifact", "shared-2")
            .tombstone(tombstone("shared-2", "run"))
            .only(GATE_ONLY),
        Case::new(
            "a run and an artifact of one id, the run revoked",
            Revoked(&["run shared-3", "revoked"]),
        )
        .object("run", live("shared-3"))
        .object("artifact", live("shared-3"))
        .below("artifact", "shared-3")
        .below("run", "shared-3")
        .tombstone(tombstone("shared-3", "run")),
        // A tombstone that is not one `begin_revoke` writes cannot be matched to
        // anything: the node is treated as revoked.
        Case::new(
            "a run whose tombstone does not decode",
            Revoked(&["run run-x1", "cannot be read"]),
        )
        .object("run", live("run-x1"))
        .below("run", "run-x1")
        .tombstone(corrupt("run-x1")),
        Case::new(
            "an artifact whose tombstone does not decode",
            Revoked(&["artifact art-x2", "cannot be read"]),
        )
        .object("artifact", live("art-x2"))
        .below("artifact", "art-x2")
        .tombstone(corrupt("art-x2")),
        Case::new(
            "an artifact whose tombstone names an unknown source kind",
            Revoked(&["artifact art-x3", "cannot be read"]),
        )
        .object("artifact", live("art-x3"))
        .below("artifact", "art-x3")
        .tombstone(tombstone("art-x3", "release")),
        // Only runs and artifacts are judged by a tombstone, and only artifacts by
        // their body.
        Case::new(
            "a release whose id carries a tombstone that does not decode",
            Accepted,
        )
        .object("release", live("rel-2"))
        .below("release", "rel-2")
        .tombstone(corrupt("rel-2"))
        .only(GATE_ONLY),
        Case::new(
            "an artifact the cleanup redacted, without a tombstone of its own",
            Revoked(&["artifact art-w1", "redacted"]),
        )
        .object("artifact", redacted("art-w1"))
        .below("artifact", "art-w1"),
        Case::new("a run whose stored body says redacted", Accepted)
            .object("run", redacted("run-q1"))
            .below("run", "run-q1"),
        Case::new("live runs and artifacts only", Accepted)
            .object("artifact", live("art-l1"))
            .object("run", live("run-l2"))
            .object("run", live("run-l3"))
            .below("artifact", "art-l1")
            .edge("artifact", "art-l1", "run", "run-l2")
            .below("run", "run-l3"),
    ]
}

#[tokio::test]
async fn every_entry_point_judges_a_source_by_one_rule() {
    let mut problems = Vec::new();
    for case in cases() {
        for entry in ENTRIES {
            if !case.entries.contains(&entry) {
                continue;
            }
            let setup = prepare(entry, "rule.sqlite3").await;
            apply(entry, &setup.fx, &case).await;
            problems.extend(judge(entry, &setup, case.name, &case.verdict).await);
        }
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

// ---------------------------------------------------------------------------
// The bound: a closure over it is refused, never truncated and then judged live
// ---------------------------------------------------------------------------

/// `length` nodes below the root, one behind the other: the live artifacts
/// `link-1` .. `link-{length - 1}` and a last node `tail` of kind `tail_kind`. The
/// closure of the root is the root and this chain, `length + 1` nodes.
async fn put_chain(fx: &Fx, root: (&str, &str), length: usize, tail_kind: &str) {
    let admin = admin();
    let mut session = fx.store.session().await.unwrap();
    let mut previous = (root.0.to_string(), root.1.to_string());
    for index in 1..length {
        let id = format!("link-{index}");
        session
            .put_edge(&admin, &previous.0, &previous.1, "artifact", &id)
            .await
            .unwrap();
        previous = ("artifact".into(), id);
    }
    session
        .put_edge(&admin, &previous.0, &previous.1, tail_kind, "tail")
        .await
        .unwrap();
    session.commit().await.unwrap();
}

#[tokio::test]
async fn a_closure_over_the_bound_is_refused_by_every_entry_point_not_truncated() {
    // (nodes in the closure, kind and revocation of the last node, verdict, entry
    // points). The closure is the root and a chain below it; its last node is the
    // one a truncated walk would never reach. A closure at exactly the bound is
    // accepted only where the gate decides alone: `stage_package` and `install`
    // would go on to the storage check at blob publication, which has a bound and a
    // requirement of its own (every node of the closure is a stored object).
    let cases: [(usize, &str, bool, Verdict, &[Entry]); 5] = [
        (CLOSURE_BOUND, "run", false, Verdict::Accepted, GATE_ONLY),
        (
            CLOSURE_BOUND,
            "run",
            true,
            Verdict::Revoked(&["run tail", "revoked"]),
            &ENTRIES,
        ),
        (CLOSURE_BOUND + 1, "run", false, Verdict::TooBig, &ENTRIES),
        (CLOSURE_BOUND + 1, "run", true, Verdict::TooBig, &ENTRIES),
        (
            CLOSURE_BOUND + 50,
            "artifact",
            true,
            Verdict::TooBig,
            &ENTRIES,
        ),
    ];
    let mut problems = Vec::new();
    for (size, tail_kind, revoked, verdict, entries) in &cases {
        for entry in entries.iter().copied() {
            let name = format!("{size} nodes, {tail_kind} tail, revoked {revoked}");
            let setup = prepare(entry, "bound.sqlite3").await;
            put_chain(&setup.fx, entry.root(), size - 1, tail_kind).await;
            if *revoked {
                setup
                    .fx
                    .put(&admin(), "tombstone", "tail", &tombstone("tail", tail_kind))
                    .await;
            }
            problems.extend(judge(entry, &setup, &name, verdict).await);
        }
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

// ---------------------------------------------------------------------------
// A grant over a revoked run, in every state of the cleanup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_grant_over_a_revoked_run_is_refused_in_every_state_of_the_cleanup() {
    let fx = Fx::open("grant-states.sqlite3").await;
    let admin = admin();
    store_run(&fx, "run-x").await;
    store_run(&fx, "run-z").await;

    // The control: a grant over a live run is accepted, with the artifact, the
    // edge to the run and the audit row it always wrote.
    let live_selection = selection(&["run-z"]);
    store_source_selection(&fx.store, &host(), &live_selection)
        .await
        .unwrap();
    assert!(
        fx.raw(&admin, "artifact", &grant_id(&live_selection))
            .await
            .is_some()
    );

    let mut status = begin(&fx, &admin, "run", "run-x").await;
    let nodes = [("run", "run-x"), ("run", "run-z")];
    let expected = "source grant depends on run run-x, whose source was revoked";
    let mut seen: Vec<CleanupState> = Vec::new();
    let mut problems = Vec::new();
    for clock in 501..901 {
        // One attempt per state, the first time the cleanup is seen in it. (Several
        // attempts in a row would keep reopening a cleanup that an accepting
        // writer feeds with a new dependent every time.)
        if !seen.contains(&status.state) {
            seen.push(status.state);
            let state = status.state;
            for (what, attempted) in [
                ("the revoked run alone", selection(&["run-x"])),
                (
                    "the revoked run after a live one",
                    selection(&["run-z", "run-x"]),
                ),
            ] {
                let before = shape(&fx, &admin, &nodes).await;
                match store_source_selection(&fx.store, &host(), &attempted).await {
                    Err(Error::Conflict(message)) => {
                        if message != expected {
                            problems.push(format!("{state:?} / {what}: `{message}`"));
                        }
                        let changes = before.changes(&shape(&fx, &admin, &nodes).await);
                        if !changes.is_empty() {
                            problems.push(format!(
                                "{state:?} / {what}: a refused grant wrote ({changes})"
                            ));
                        }
                        if fx
                            .raw(&admin, "artifact", &grant_id(&attempted))
                            .await
                            .is_some()
                        {
                            problems.push(format!("{state:?} / {what}: the grant artifact exists"));
                        }
                    }
                    other => problems.push(format!(
                        "{state:?} / {what}: expected a Conflict, got {other:?}"
                    )),
                }
            }
        }
        if finished(&status) {
            break;
        }
        status = step(&fx, &admin, &status, 1, clock).await;
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    for state in [
        CleanupState::Pending,
        CleanupState::Running,
        CleanupState::Complete,
    ] {
        assert!(seen.contains(&state), "no attempt in {state:?}: {seen:?}");
    }

    // The refusals did not disturb the cleanup, and a grant over the live run is
    // still accepted after the revocation, in the state `Complete`.
    store_source_selection(&fx.store, &host(), &live_selection)
        .await
        .unwrap();
    let other = selection(&["run-z", "run-w"]);
    store_source_selection(&fx.store, &host(), &other)
        .await
        .unwrap();
    assert!(
        fx.raw(&admin, "artifact", &grant_id(&other))
            .await
            .is_some()
    );
}

// ---------------------------------------------------------------------------
// The grant's upstream closure holds a revoked import source or a revoked run
// ---------------------------------------------------------------------------

const HISTORY: &[u8] = br#"{"role":"user","content":"private imported material"}"#;

/// A registration whose source reads `history.jsonl` of `dir`, shaped like
/// `registration` in `tests/revocation_source_ids_v42.rs`.
fn registration(dir: &Path, request_key: &str, logical_ids: &[&str]) -> ImportRegistrationRequest {
    let path = dir.join("history.jsonl");
    ImportRegistrationRequest {
        schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
        request_key: request_key.into(),
        roots: vec![dir.to_string_lossy().into_owned()],
        purpose: Purpose::Development,
        allow_model_excerpts: false,
        outbound_authorized: false,
        retention_scope: ImportRetentionScope::LocalPrivate,
        sources: logical_ids
            .iter()
            .map(|logical| ImportSourceSpec {
                source_id: (*logical).into(),
                path: path.to_string_lossy().into_owned(),
                reader: SourceFormat::ClaudeFixture,
                expected_digest: hash(HISTORY),
            })
            .collect(),
    }
}

#[tokio::test]
async fn a_grant_over_a_run_above_which_an_import_source_or_a_run_was_revoked_is_refused() {
    let fx = Fx::open("grant-closure.sqlite3").await;
    let admin = admin();
    let history = tempfile::tempdir().unwrap();
    tokio::fs::write(history.path().join("history.jsonl"), HISTORY)
        .await
        .unwrap();
    // A real E16 import source (an artifact of the `rsia.e16.import_source.v1`
    // schema, the only artifact `begin_revoke` accepts as a source).
    let registered = PersistentImportService::new(fx.store.clone())
        .register(
            &admin,
            registration(history.path(), "grant-import", &["source-one"]),
        )
        .await
        .unwrap();
    let import_source = registered.payload.import_source_ids[0].clone();
    store_run(&fx, "run-y").await;
    store_run(&fx, "run-top").await;
    // `run-y` is not revoked itself; the import source and `run-top` are above it.
    fx.edges(
        &admin,
        &[
            ("run", "run-y", "artifact", import_source.as_str()),
            ("run", "run-y", "run", "run-top"),
        ],
    )
    .await;

    // The control: with nothing revoked the grant is accepted. It is a different
    // selection from the ones refused below, so nothing it wrote is in the way.
    store_source_selection(&fx.store, &host(), &selection(&["run-y", "run-other"]))
        .await
        .unwrap();

    let nodes = [
        ("run", "run-y"),
        ("run", "run-top"),
        ("artifact", import_source.as_str()),
    ];
    let attempted = selection(&["run-y"]);

    // The import source is revoked (the real `begin_revoke`: tombstone, watermark,
    // a Pending cleanup).
    let cleanup = begin(&fx, &admin, "artifact", &import_source).await;
    assert_eq!(cleanup.state, CleanupState::Pending);
    let before = shape(&fx, &admin, &nodes).await;
    let message = match store_source_selection(&fx.store, &host(), &attempted).await {
        Err(Error::Conflict(message)) => message,
        other => {
            panic!("a grant above a revoked import source: expected a Conflict, got {other:?}")
        }
    };
    assert_eq!(
        message,
        format!(
            "source grant's dependency closure holds artifact {import_source}, whose source was revoked"
        )
    );
    let changes = before.changes(&shape(&fx, &admin, &nodes).await);
    assert!(changes.is_empty(), "a refused grant wrote ({changes})");
    assert!(
        fx.raw(&admin, "artifact", &grant_id(&attempted))
            .await
            .is_none()
    );

    // A run above it is revoked as well: the first node of the closure, in the
    // order of kind and id, that fails names the refusal (an artifact sorts first).
    let cleanup = begin(&fx, &admin, "run", "run-top").await;
    assert_eq!(cleanup.state, CleanupState::Pending);
    let before = shape(&fx, &admin, &nodes).await;
    let message = match store_source_selection(&fx.store, &host(), &attempted).await {
        Err(Error::Conflict(message)) => message,
        other => panic!("a grant above a revoked run: expected a Conflict, got {other:?}"),
    };
    assert!(message.contains("revoked"), "{message}");
    assert!(
        message.contains(&format!("artifact {import_source}")) || message.contains("run run-top"),
        "{message}"
    );
    let changes = before.changes(&shape(&fx, &admin, &nodes).await);
    assert!(changes.is_empty(), "a refused grant wrote ({changes})");

    // The run alone (with the import source out of the way, in another store) is
    // named as well.
    let second = Fx::open("grant-closure-run.sqlite3").await;
    store_run(&second, "run-y").await;
    store_run(&second, "run-top").await;
    second
        .edges(&admin, &[("run", "run-y", "run", "run-top")])
        .await;
    begin(&second, &admin, "run", "run-top").await;
    let before = shape(&second, &admin, &nodes).await;
    let message = match store_source_selection(&second.store, &host(), &attempted).await {
        Err(Error::Conflict(message)) => message,
        result => panic!("a grant above a revoked run: expected a Conflict, got {result:?}"),
    };
    assert_eq!(
        message,
        "source grant's dependency closure holds run run-top, whose source was revoked"
    );
    let changes = before.changes(&shape(&second, &admin, &nodes).await);
    assert!(changes.is_empty(), "a refused grant wrote ({changes})");
}

// ---------------------------------------------------------------------------
// A package staged, or a seed installed, over a source derived from a revoked
// run: refused in every state of the cleanup, and nothing is written
// ---------------------------------------------------------------------------

/// One attempt of each E16 writer over the derived artifact, plus a control over an
/// unrelated live artifact, all in the current state of the cleanup. Returns what
/// differs from the expectation.
async fn derived_attempts(fx: &Fx, label: &str, derived: &str) -> Vec<String> {
    let admin = admin();
    let nodes = [
        ("run", "run-r"),
        ("artifact", derived),
        ("artifact", "unrelated"),
    ];
    let mut problems = Vec::new();

    // The control first: a live source that depends on nothing revoked is not
    // refused by the gate in any state.
    let unrelated = source_ref(fx, &admin, "artifact", "unrelated").await;
    if let Err(error) = PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request(&format!("control-stage-{label}"), vec![unrelated.clone()]),
    )
    .await
    {
        problems.push(format!(
            "{label}: stage over an unrelated live source: {error:?}"
        ));
    }
    if let Err(error) = PersistentSeedStore::install(
        &admin,
        &fx.store,
        install_request(
            &format!("control-install-{label}"),
            &format!("control-seed-{label}"),
            vec![unrelated],
        ),
    )
    .await
    {
        problems.push(format!(
            "{label}: install over an unrelated live source: {error:?}"
        ));
    }

    // The source as it is stored now: before the cleanup reaches it, live; after
    // it, the redacted body, whose digest a caller that reads and hashes the
    // object takes.
    let source = source_ref(fx, &admin, "artifact", derived).await;
    let before = shape(fx, &admin, &nodes).await;
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request(&format!("stage-{label}"), vec![source.clone()]),
    )
    .await
    .map(|_| ());
    let installed = PersistentSeedStore::install(
        &admin,
        &fx.store,
        install_request(
            &format!("install-{label}"),
            &format!("seed-{label}"),
            vec![source],
        ),
    )
    .await
    .map(|_| ());
    for (writer, result) in [("stage", staged), ("install", installed)] {
        match result {
            Err(Error::Forbidden) => {}
            other => problems.push(format!(
                "{label}: {writer} over a source derived from a revoked run: expected Forbidden, got {other:?}"
            )),
        }
    }
    let changes = before.changes(&shape(fx, &admin, &nodes).await);
    if !changes.is_empty() {
        problems.push(format!("{label}: a refused writer wrote ({changes})"));
    }
    problems
}

#[tokio::test]
async fn a_staged_package_or_an_install_over_a_source_derived_from_a_revoked_run_is_refused() {
    let fx = Fx::open("derived.sqlite3").await;
    let admin = admin();
    store_run(&fx, "run-r").await;
    fx.put(&admin, "artifact", "unrelated", &live("unrelated"))
        .await;
    // The derived source: a package staged over the run. It depends on the run
    // through the edge `put_envelope` writes, and is an artifact of the E16
    // staged-asset schema, which the cleanup redacts.
    let run = source_ref(&fx, &admin, "run", "run-r").await;
    let derived = PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request("derived-stage", vec![run]),
    )
    .await
    .unwrap()
    .id;

    // Before the revocation both writers accept a package, and an install, over
    // the derived source.
    let source = source_ref(&fx, &admin, "artifact", &derived).await;
    PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request("live-stage", vec![source.clone()]),
    )
    .await
    .unwrap();
    PersistentSeedStore::install(
        &admin,
        &fx.store,
        install_request("live-install", "live-seed", vec![source]),
    )
    .await
    .unwrap();

    let mut status = begin(&fx, &admin, "run", "run-r").await;
    let mut seen: Vec<CleanupState> = Vec::new();
    let mut problems = Vec::new();
    for clock in 501..901 {
        if !seen.contains(&status.state) {
            seen.push(status.state);
            let label = format!("{:?}", status.state).to_lowercase();
            problems.extend(derived_attempts(&fx, &label, &derived).await);
        }
        if finished(&status) {
            break;
        }
        status = step(&fx, &admin, &status, 1, clock).await;
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    for state in [
        CleanupState::Pending,
        CleanupState::Running,
        CleanupState::Complete,
    ] {
        assert!(seen.contains(&state), "no attempt in {state:?}: {seen:?}");
    }
    // By now the cleanup replaced the derived source: the digest the last attempt
    // took was the digest of the redacted body.
    assert_eq!(
        fx.raw(&admin, "artifact", &derived).await.unwrap()["schema_version"],
        REDACTED
    );
}

// ---------------------------------------------------------------------------
// What already exists cannot be read, exported or reset over a revoked closure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn existing_staged_packages_installs_and_exports_are_closed_over_a_revoked_closure() {
    let fx = Fx::open("existing.sqlite3").await;
    let admin = admin();
    store_run(&fx, "run-r").await;
    let run = source_ref(&fx, &admin, "run", "run-r").await;
    let derived = PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request("derived-stage", vec![run]),
    )
    .await
    .unwrap();
    let source = source_ref(&fx, &admin, "artifact", &derived.id).await;
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request("ok-stage", vec![source.clone()]),
    )
    .await
    .unwrap();
    let installed = PersistentSeedStore::install(
        &admin,
        &fx.store,
        install_request("ok-install", "ok-seed", vec![source.clone()]),
    )
    .await
    .unwrap();
    let attempt_id = PersistentPackageStore::prepare_export(
        &admin,
        &fx.store,
        "ok-export",
        &staged.id,
        "local_namespace",
        None,
    )
    .await
    .unwrap()
    .id;

    // The control: everything is readable while nothing is revoked.
    PersistentPackageStore::read_staged(&admin, &fx.store, &staged.id)
        .await
        .unwrap();
    PersistentSeedStore::read_install(&admin, &fx.store, &installed.id)
        .await
        .unwrap();

    // The run is revoked WITHOUT a watermark move (a tombstone written by hand, as
    // the fixtures of AG-044 do): with the watermark moved, every read below is
    // already refused as stale, before the sources are looked at. The package, the
    // install and the export attempt do not name the run: the derived artifact
    // sits between them and it.
    fx.put(&admin, "tombstone", "run-r", &tombstone("run-r", "run"))
        .await;
    let nodes = [
        ("run", "run-r"),
        ("artifact", derived.id.as_str()),
        ("artifact", staged.id.as_str()),
        ("artifact", installed.id.as_str()),
    ];
    let before = shape(&fx, &admin, &nodes).await;
    let export_directory = fx
        .store
        .local_export_directory(&admin, &attempt_id)
        .unwrap();

    let mut problems = Vec::new();
    let mut expect_forbidden = |what: &str, result: Result<(), Error>| match result {
        Err(Error::Forbidden) => {}
        other => problems.push(format!("{what}: expected Forbidden, got {other:?}")),
    };
    expect_forbidden(
        "read_staged",
        PersistentPackageStore::read_staged(&admin, &fx.store, &staged.id)
            .await
            .map(|_| ()),
    );
    expect_forbidden(
        "read_install",
        PersistentSeedStore::read_install(&admin, &fx.store, &installed.id)
            .await
            .map(|_| ()),
    );
    expect_forbidden(
        "record_candidate_handoff",
        PersistentPackageStore::record_candidate_handoff(&admin, &fx.store, &staged.id, "cand-x")
            .await
            .map(|_| ()),
    );
    expect_forbidden(
        "prepare_export",
        PersistentPackageStore::prepare_export(
            &admin,
            &fx.store,
            "late-export",
            &staged.id,
            "local_namespace",
            None,
        )
        .await
        .map(|_| ()),
    );
    expect_forbidden(
        "complete_export",
        PersistentPackageStore::complete_export(&admin, &fx.store, &attempt_id)
            .await
            .map(|_| ()),
    );
    expect_forbidden(
        "stage_reset_to_baseline",
        PersistentSeedStore::stage_reset_to_baseline(
            &admin,
            &fx.store,
            &installed.id,
            "late-reset",
            &environment(),
            "compiler-v1",
        )
        .await
        .map(|_| ()),
    );
    expect_forbidden(
        "stage_package, the request of the existing package again",
        PersistentPackageStore::stage_package(
            &admin,
            &fx.store,
            stage_request("ok-stage", vec![source.clone()]),
        )
        .await
        .map(|_| ()),
    );
    expect_forbidden(
        "install, the request of the existing install again",
        PersistentSeedStore::install(
            &admin,
            &fx.store,
            install_request("ok-install", "ok-seed", vec![source]),
        )
        .await
        .map(|_| ()),
    );
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
    let changes = before.changes(&shape(&fx, &admin, &nodes).await);
    assert!(
        changes.is_empty(),
        "a refused operation wrote to the store ({changes})"
    );
    assert!(
        !tokio::fs::try_exists(&export_directory).await.unwrap(),
        "a refused export wrote its output"
    );
}

#[tokio::test]
async fn after_a_real_revocation_the_existing_package_and_install_are_closed_by_the_watermark() {
    // The control that bounds the test above: with `begin_revoke` (the watermark
    // moves) the read of what exists was already closed, before the sources are
    // looked at. This is the behaviour that stays as it was, in every state: first
    // as a stale envelope, then, once the cleanup has redacted it, as an object that
    // no longer reads as an envelope.
    let fx = Fx::open("existing-real.sqlite3").await;
    let admin = admin();
    store_run(&fx, "run-r").await;
    let run = source_ref(&fx, &admin, "run", "run-r").await;
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request("real-stage", vec![run.clone()]),
    )
    .await
    .unwrap();
    let installed = PersistentSeedStore::install(
        &admin,
        &fx.store,
        install_request("real-install", "real-seed", vec![run]),
    )
    .await
    .unwrap();
    let mut status = begin(&fx, &admin, "run", "run-r").await;
    let mut states: Vec<CleanupState> = Vec::new();
    for clock in 501..901 {
        for (what, result) in [
            (
                "read_staged",
                PersistentPackageStore::read_staged(&admin, &fx.store, &staged.id)
                    .await
                    .map(|_| ()),
            ),
            (
                "read_install",
                PersistentSeedStore::read_install(&admin, &fx.store, &installed.id)
                    .await
                    .map(|_| ()),
            ),
        ] {
            match (status.state, result) {
                // Until the cleanup reaches the envelope: refused as stale.
                (CleanupState::Pending, Err(Error::Conflict(message)))
                    if message.contains("stale or invalid") => {}
                (CleanupState::Pending, other) => {
                    panic!("Pending / {what}: expected the watermark to refuse, got {other:?}")
                }
                (_, Err(_)) => {}
                (state, Ok(())) => panic!("{state:?} / {what}: read a revoked package"),
            }
        }
        if !states.contains(&status.state) {
            states.push(status.state);
        }
        if finished(&status) {
            break;
        }
        status = step(&fx, &admin, &status, 1, clock).await;
    }
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert!(states.contains(&CleanupState::Pending) && states.contains(&CleanupState::Complete));
}

// ---------------------------------------------------------------------------
// Exactness: by namespace, and by kind for a source that is a run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_source_revoked_in_another_namespace_refuses_nothing_here() {
    let fx = Fx::open("namespaces.sqlite3").await;
    let m_admin = Context::new("m", "admin", Role::Admin).unwrap();
    let m_host = Context::new("m", "host", Role::Host).unwrap();
    fx.bump(&m_admin, "ag046-m-watermark").await;
    // The same ids in both namespaces: the run `run-y` and, above it, the artifact
    // `src-art` that a package or an install names. The run is revoked in `m` only.
    store_trace_authority(&fx.store, &host(), &authority("run-y"))
        .await
        .unwrap();
    store_trace_authority(&fx.store, &m_host, &authority("run-y"))
        .await
        .unwrap();
    for ctx in [admin(), m_admin.clone()] {
        fx.put(&ctx, "artifact", "src-art", &live("src-art")).await;
        fx.edges(&ctx, &[("artifact", "src-art", "run", "run-y")])
            .await;
    }
    begin(&fx, &m_admin, "run", "run-y").await;
    let setup = Setup { fx, existing: None };

    let mut problems = Vec::new();
    for entry in [Entry::Grant, Entry::Stage, Entry::Install] {
        // In `m` the source is revoked: the grant names the run, the two E16
        // writers reach it through `src-art`. Nothing is written.
        let nodes = [entry.root()];
        let before = shape(&setup.fx, &m_admin, &nodes).await;
        match (entry, attempt(entry, &setup, "m").await) {
            (Entry::Grant, Err(Error::Conflict(message))) if message.contains("run run-y") => {}
            (Entry::Stage | Entry::Install, Err(Error::Forbidden)) => {}
            (entry, other) => problems.push(format!("m / {entry:?}: {other:?}")),
        }
        let changes = before.changes(&shape(&setup.fx, &m_admin, &nodes).await);
        if !changes.is_empty() {
            problems.push(format!("m / {entry:?}: a refused call wrote ({changes})"));
        }
        // In `n` the same ids are live, and nothing there was revoked.
        if let Err(error) = attempt(entry, &setup, "n").await {
            problems.push(format!(
                "n / {entry:?}: refused by a revocation of `m`: {error:?}"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[tokio::test]
async fn an_e16_source_that_is_a_run_is_judged_by_its_own_kind() {
    // The reference names a run, not an artifact: what is under its id in the
    // `tombstone` kind decides only when it was written for a run. Read through the
    // package or the install that was stored over the run while nothing was revoked:
    // a read looks at nothing but the gate. (A new `stage_package` or `install` over
    // such a run still fails later, in the storage check: `known_boundary_` below.)
    let mut problems = Vec::new();
    for (name, on_run, accepted) in [
        ("a tombstone of the run's own kind", OnRoot::Own, false),
        (
            "a tombstone of the artifact kind that shares the id",
            OnRoot::Other,
            true,
        ),
        ("a tombstone that does not decode", OnRoot::Corrupt, false),
        (
            "a tombstone of an unknown source kind",
            OnRoot::UnknownKind,
            false,
        ),
    ] {
        for entry in [Entry::ReadStaged, Entry::ReadInstall] {
            let fx = Fx::open("run-ref.sqlite3").await;
            let admin = admin();
            store_run(&fx, "run-y").await;
            let run = source_ref(&fx, &admin, "run", "run-y").await;
            let id = if entry == Entry::ReadStaged {
                PersistentPackageStore::stage_package(
                    &admin,
                    &fx.store,
                    stage_request("run-ref-stage", vec![run]),
                )
                .await
                .unwrap()
                .id
            } else {
                PersistentSeedStore::install(
                    &admin,
                    &fx.store,
                    install_request("run-ref-install", "run-ref-seed", vec![run]),
                )
                .await
                .unwrap()
                .id
            };
            // The tombstone comes after the run and the package are stored (a run
            // cannot be stored over a tombstone), without a watermark move.
            fx.put(&admin, "tombstone", "run-y", &on_run.body(Entry::Grant))
                .await;
            let result = if entry == Entry::ReadStaged {
                PersistentPackageStore::read_staged(&admin, &fx.store, &id)
                    .await
                    .map(|_| ())
            } else {
                PersistentSeedStore::read_install(&admin, &fx.store, &id)
                    .await
                    .map(|_| ())
            };
            match (accepted, result) {
                (true, Ok(())) | (false, Err(Error::Forbidden)) => {}
                (_, other) => problems.push(format!("{entry:?} / {name}: {other:?}")),
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

// ---------------------------------------------------------------------------
// The submit gate keeps its answers, byte for byte
// ---------------------------------------------------------------------------

mod synthetic {
    use super::*;
    use evo_engine::curriculum::{curriculum_profile_storage_id, curriculum_state_storage_id};

    pub const PROFILE_ID: &str = "synthetic-profile";
    pub const STATE_ID: &str = "synthetic-state";

    pub fn profile() -> String {
        curriculum_profile_storage_id(PROFILE_ID).unwrap()
    }

    pub fn state() -> String {
        curriculum_state_storage_id(STATE_ID).unwrap()
    }

    pub fn request(request_key: &str) -> Value {
        json!({
            "schema_version": "rsia.management.curriculum_step.v1",
            "request_key": request_key,
            "profile_id": PROFILE_ID,
            "state_id": STATE_ID,
            "root_budget_limit_micros": 1_000u64,
        })
    }

    /// A store whose request dependencies, the profile and the learner state, are
    /// live and have nothing below them yet.
    pub async fn open(name: &str) -> Fx {
        let fx = Fx::open(name).await;
        fx.put(&admin(), "artifact", &profile(), &live(&profile()))
            .await;
        fx.put(&admin(), "artifact", &state(), &live(&state()))
            .await;
        fx
    }
}

#[tokio::test]
async fn the_submit_gate_answers_with_the_same_messages_as_before_it_was_shared() {
    use synthetic::{profile, state};
    let (profile, state) = (profile(), state());
    let prefix = "management request";
    // (name, objects, edges below the state, tombstones, expected message). The
    // messages are the ones the submit check wrote before the judgement was lifted
    // out of `dispatch.rs`; this test passes on the code before and after the move.
    type Pin = (
        &'static str,
        Vec<(&'static str, String, Value)>,
        Vec<(&'static str, &'static str)>,
        Vec<Value>,
        String,
    );
    let cases: Vec<Pin> = vec![
        (
            "a direct dependency whose source was revoked",
            vec![],
            vec![],
            vec![tombstone(&profile, "artifact")],
            format!("{prefix} depends on artifact {profile}, whose source was revoked"),
        ),
        (
            "a direct dependency whose tombstone does not decode",
            vec![],
            vec![],
            vec![corrupt(&profile)],
            format!(
                "{prefix} depends on artifact {profile}, whose revocation tombstone cannot be read; the source is treated as revoked"
            ),
        ),
        (
            "a direct dependency the cleanup redacted",
            vec![("artifact", profile.clone(), redacted(&profile))],
            vec![],
            vec![],
            format!(
                "{prefix} depends on artifact {profile}, which was redacted because its source was revoked"
            ),
        ),
        (
            "a revoked run in the closure",
            vec![("run", "run-r1".into(), live("run-r1"))],
            vec![("run", "run-r1")],
            vec![tombstone("run-r1", "run")],
            format!("{prefix}'s dependency closure holds run run-r1, whose source was revoked"),
        ),
        (
            "an artifact of the closure whose tombstone does not decode",
            vec![("artifact", "art-x2".into(), live("art-x2"))],
            vec![("artifact", "art-x2")],
            vec![corrupt("art-x2")],
            format!(
                "{prefix}'s dependency closure holds artifact art-x2, whose revocation tombstone cannot be read; the source is treated as revoked"
            ),
        ),
        (
            "a redacted artifact in the closure",
            vec![("artifact", "art-w1".into(), redacted("art-w1"))],
            vec![("artifact", "art-w1")],
            vec![],
            format!(
                "{prefix}'s dependency closure holds artifact art-w1, which was redacted because its source was revoked"
            ),
        ),
    ];
    let mut problems = Vec::new();
    for (name, objects, edges, tombstones, expected) in cases {
        let fx = synthetic::open("pins.sqlite3").await;
        let admin = admin();
        for (kind, id, body) in &objects {
            fx.put(&admin, kind, id, body).await;
        }
        for (kind, id) in &edges {
            fx.edges(&admin, &[("artifact", state.as_str(), kind, id)])
                .await;
        }
        for body in &tombstones {
            fx.put(&admin, "tombstone", body["id"].as_str().unwrap(), body)
                .await;
        }
        let dispatcher = ManagementDispatcher::new(fx.store.clone(), vec![admin.clone()]).unwrap();
        match dispatcher
            .submit(&admin, "curriculum.step", synthetic::request("pin-key"))
            .await
        {
            Err(Error::Conflict(message)) if message == expected => {}
            other => problems.push(format!("{name}: expected `{expected}`, got {other:?}")),
        }
    }
    // A closure over the bound: the message of `Session::upstream_closure`.
    let fx = synthetic::open("pins-bound.sqlite3").await;
    let admin = admin();
    put_chain(&fx, ("artifact", state.as_str()), CLOSURE_BOUND - 1, "run").await;
    let dispatcher = ManagementDispatcher::new(fx.store.clone(), vec![admin.clone()]).unwrap();
    match dispatcher
        .submit(&admin, "curriculum.step", synthetic::request("pin-bound"))
        .await
    {
        Err(Error::Conflict(message))
            if message
                == "upstream dependency closure exceeds 10000 nodes; it is refused, not truncated" =>
            {}
        other => problems.push(format!("a closure over the bound: {other:?}")),
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

// ---------------------------------------------------------------------------
// The repeated request of a package that has a manifest dependency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_repeated_request_of_a_package_is_judged_over_its_manifest_dependency() {
    // Request sources keep their first check. The union with resolved manifest
    // dependencies is also checked before either a new write or an existing lookup.
    let admin = admin();
    let dependency = PackageDependency {
        publisher: "org.rsia".into(),
        asset_id: "dep-asset".into(),
        kind: "skill".into(),
        version_req: "1".into(),
    };
    let dependency_id = package_dependency_ref_id(&dependency).unwrap();
    let mut with_dependency = manifest();
    with_dependency.dependencies.push(dependency);

    let fx = Fx::open("repeat-dependency.sqlite3").await;
    store_run(&fx, "run-r").await;
    fx.put(&admin, "artifact", "src-art", &live("src-art"))
        .await;
    fx.put(&admin, "artifact", &dependency_id, &live(&dependency_id))
        .await;
    fx.edges(
        &admin,
        &[("artifact", dependency_id.as_str(), "run", "run-r")],
    )
    .await;
    let source = source_ref(&fx, &admin, "artifact", "src-art").await;
    let request = stage_request_for("repeat-stage", with_dependency, vec![source]);
    let staged = PersistentPackageStore::stage_package(&admin, &fx.store, request.clone())
        .await
        .unwrap();

    // The run below the dependency is revoked without a watermark move. The sources
    // of the request do not depend on it, so their check passes; the union with the
    // manifest dependency refuses before the existing envelope is considered.
    fx.put(&admin, "tombstone", "run-r", &tombstone("run-r", "run"))
        .await;
    let nodes = [("artifact", "src-art"), ("artifact", staged.id.as_str())];
    let before = shape(&fx, &admin, &nodes).await;
    let result = PersistentPackageStore::stage_package(&admin, &fx.store, request).await;
    assert!(
        matches!(result, Err(Error::Forbidden)),
        "the request of a package whose dependency is revoked, again: {result:?}"
    );
    let changes = before.changes(&shape(&fx, &admin, &nodes).await);
    assert!(changes.is_empty(), "a refused request wrote ({changes})");
}

// ---------------------------------------------------------------------------
// Manifest dependencies are judged together with request sources before any write
// ---------------------------------------------------------------------------

fn manifest_dependency(asset: &str) -> PackageDependency {
    PackageDependency {
        publisher: "org.rsia".into(),
        asset_id: asset.into(),
        kind: "skill".into(),
        version_req: "1".into(),
    }
}

fn manifest_with(dependencies: Vec<PackageDependency>) -> AssetPackageManifest {
    let mut package = manifest();
    package.dependencies = dependencies;
    package
}

/// The existing full store shape, plus blob/export content digests, so an unchanged
/// path cannot conceal a rewrite of its bytes.
async fn manifest_shape(
    fx: &Fx,
    ctx: &Context,
    nodes: &[(&str, &str)],
) -> (Shape, Vec<(String, String)>) {
    let stored = shape(fx, ctx, nodes).await;
    let contents = fx
        .files()
        .into_iter()
        .map(|path| {
            let digest = hash(&std::fs::read(fx.dir.path().join(&path)).unwrap());
            (path, digest)
        })
        .collect();
    (stored, contents)
}

enum ManifestRefusal {
    Forbidden,
    NotFound,
    Conflict(&'static str),
}

async fn refused_manifest_stage(
    fx: &Fx,
    ctx: &Context,
    request: StagePackageRequest,
    nodes: &[(&str, &str)],
    label: &str,
    expected: ManifestRefusal,
) -> Vec<String> {
    let before = manifest_shape(fx, ctx, nodes).await;
    let result = PersistentPackageStore::stage_package(ctx, &fx.store, request).await;
    let mut problems = Vec::new();
    match (&expected, &result) {
        (ManifestRefusal::Forbidden, Err(Error::Forbidden))
        | (ManifestRefusal::NotFound, Err(Error::NotFound)) => {}
        (ManifestRefusal::Conflict(expected), Err(Error::Conflict(actual)))
            if *expected == actual.as_str() => {}
        _ => problems.push(format!("{label}: unexpected answer: {result:?}")),
    }
    let after = manifest_shape(fx, ctx, nodes).await;
    let changes = before.0.changes(&after.0);
    if !changes.is_empty() {
        problems.push(format!("{label}: a refused stage wrote ({changes})"));
    }
    if before.1 != after.1 {
        problems.push(format!("{label}: blob/export contents changed"));
    }
    problems
}

fn assert_manifest_problems(problems: Vec<String>) {
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// The `Prepared` staged packages of the store.
async fn prepared_staged(fx: &Fx) -> usize {
    fx.artifacts_of(&admin(), "rsia.e16.staged_asset.v1")
        .await
        .iter()
        .filter(|body| body["payload"]["state"] == "prepared")
        .count()
}

#[tokio::test]
async fn manifest_dependencies_are_judged_before_the_first_package_write() {
    let admin = admin();
    let dependency = manifest_dependency("dep-asset");
    let dependency_id = package_dependency_ref_id(&dependency).unwrap();
    let with_dependency = manifest_with(vec![dependency]);
    let cases = [
        "upstream-run-tombstone",
        "dependency-redacted",
        "dependency-artifact-tombstone",
        "upstream-artifact-redacted",
        "dependency-unreadable-tombstone",
        "upstream-unreadable-tombstone",
    ];
    let mut problems = Vec::new();
    for case in cases {
        let fx = Fx::open("manifest-refusal.sqlite3").await;
        fx.put(&admin, "artifact", "src-art", &live("src-art"))
            .await;
        fx.put(&admin, "artifact", &dependency_id, &live(&dependency_id))
            .await;
        match case {
            "upstream-run-tombstone" => {
                store_run(&fx, "run-r").await;
                fx.edges(
                    &admin,
                    &[("artifact", dependency_id.as_str(), "run", "run-r")],
                )
                .await;
                fx.put(&admin, "tombstone", "run-r", &tombstone("run-r", "run"))
                    .await;
            }
            "dependency-redacted" => {
                fx.put(
                    &admin,
                    "artifact",
                    &dependency_id,
                    &redacted(&dependency_id),
                )
                .await;
            }
            "dependency-artifact-tombstone" => {
                fx.put(
                    &admin,
                    "tombstone",
                    &dependency_id,
                    &tombstone(&dependency_id, "artifact"),
                )
                .await;
            }
            "upstream-artifact-redacted" | "upstream-unreadable-tombstone" => {
                let body = if case == "upstream-artifact-redacted" {
                    redacted("upstream-art")
                } else {
                    live("upstream-art")
                };
                fx.put(&admin, "artifact", "upstream-art", &body).await;
                fx.edges(
                    &admin,
                    &[(
                        "artifact",
                        dependency_id.as_str(),
                        "artifact",
                        "upstream-art",
                    )],
                )
                .await;
                if case == "upstream-unreadable-tombstone" {
                    fx.put(
                        &admin,
                        "tombstone",
                        "upstream-art",
                        &corrupt("upstream-art"),
                    )
                    .await;
                }
            }
            "dependency-unreadable-tombstone" => {
                fx.put(
                    &admin,
                    "tombstone",
                    &dependency_id,
                    &corrupt(&dependency_id),
                )
                .await;
            }
            _ => unreachable!(),
        }
        let source = source_ref(&fx, &admin, "artifact", "src-art").await;
        let nodes = [
            ("artifact", "src-art"),
            ("artifact", dependency_id.as_str()),
            ("run", "run-r"),
            ("artifact", "upstream-art"),
        ];
        problems.extend(
            refused_manifest_stage(
                &fx,
                &admin,
                stage_request_for("manifest-stage", with_dependency.clone(), vec![source]),
                &nodes,
                case,
                ManifestRefusal::Forbidden,
            )
            .await,
        );
    }
    assert_manifest_problems(problems);
}

/// Give the manifest's storage ref a real staged-asset body and a real content
/// blob, so the lifecycle cleanup can redact it rather than fail on a fixture schema.
async fn store_staged_manifest_dependency(
    fx: &Fx,
    dependency: &PackageDependency,
    source: E16SourceRef,
) -> String {
    let admin = admin();
    let mut envelope = PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request("manifest-dependency-body", vec![source.clone()]),
    )
    .await
    .unwrap();
    let id = package_dependency_ref_id(dependency).unwrap();
    envelope.id = id.clone();
    fx.put(
        &admin,
        "artifact",
        &id,
        &serde_json::to_value(envelope).unwrap(),
    )
    .await;
    fx.edges(
        &admin,
        &[(
            "artifact",
            id.as_str(),
            source.kind.as_str(),
            source.id.as_str(),
        )],
    )
    .await;
    id
}

#[tokio::test]
async fn manifest_dependencies_refuse_real_revocation_and_repeated_requests_without_revival() {
    let admin = admin();
    let mut problems = Vec::new();
    for source_kind in ["run", "artifact"] {
        for already_staged in [false, true] {
            let fx = Fx::open("manifest-real-revoke.sqlite3").await;
            let source_id = if source_kind == "run" {
                store_run(&fx, "manifest-run").await;
                "manifest-run".to_string()
            } else {
                tokio::fs::write(fx.dir.path().join("history.jsonl"), HISTORY)
                    .await
                    .unwrap();
                let registered = PersistentImportService::new(fx.store.clone())
                    .register(
                        &admin,
                        registration(fx.dir.path(), "manifest-import", &["source-one"]),
                    )
                    .await
                    .unwrap();
                registered.payload.import_source_ids[0].clone()
            };
            let dependency = manifest_dependency("real-dependency");
            let source = source_ref(&fx, &admin, source_kind, &source_id).await;
            let dependency_id = store_staged_manifest_dependency(&fx, &dependency, source).await;
            fx.put(&admin, "artifact", "independent", &live("independent"))
                .await;
            let independent = source_ref(&fx, &admin, "artifact", "independent").await;
            let request = stage_request_for(
                "manifest-real-stage",
                manifest_with(vec![dependency]),
                vec![independent],
            );
            let existing = if already_staged {
                let staged =
                    PersistentPackageStore::stage_package(&admin, &fx.store, request.clone())
                        .await
                        .unwrap();
                assert_eq!(staged.payload.state, StagedAssetState::Staged);
                Some(staged.id)
            } else {
                None
            };
            let mut status = begin(&fx, &admin, source_kind, &source_id).await;
            assert_eq!(status.state, CleanupState::Pending);
            for phase in [CleanupState::Pending, CleanupState::Complete] {
                if phase == CleanupState::Complete {
                    for clock in 501..601 {
                        if finished(&status) {
                            break;
                        }
                        status = step(&fx, &admin, &status, 100, clock).await;
                    }
                    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
                    assert_eq!(
                        fx.raw(&admin, "artifact", &dependency_id).await.unwrap()["schema_version"],
                        REDACTED
                    );
                    if let Some(id) = &existing {
                        assert_eq!(
                            fx.raw(&admin, "artifact", id).await.unwrap()["schema_version"],
                            REDACTED
                        );
                    }
                }
                let mut nodes = vec![
                    (source_kind, source_id.as_str()),
                    ("artifact", dependency_id.as_str()),
                    ("artifact", "independent"),
                ];
                if let Some(id) = &existing {
                    nodes.push(("artifact", id.as_str()));
                }
                for repeat in 1..=2 {
                    let label = format!(
                        "{source_kind}, staged={already_staged}, {phase:?}, repeat={repeat}"
                    );
                    problems.extend(
                        refused_manifest_stage(
                            &fx,
                            &admin,
                            request.clone(),
                            &nodes,
                            &label,
                            ManifestRefusal::Forbidden,
                        )
                        .await,
                    );
                }
            }
        }
    }
    assert_manifest_problems(problems);
}

#[tokio::test]
async fn manifest_dependencies_refuse_mixed_live_and_revoked_refs_in_either_order() {
    let admin = admin();
    let mut problems = Vec::new();
    for reversed in [false, true] {
        let fx = Fx::open("manifest-mixed.sqlite3").await;
        let live_dependency = manifest_dependency("live-dependency");
        let revoked_dependency = manifest_dependency("revoked-dependency");
        let live_id = package_dependency_ref_id(&live_dependency).unwrap();
        let revoked_id = package_dependency_ref_id(&revoked_dependency).unwrap();
        for id in [
            live_id.as_str(),
            revoked_id.as_str(),
            "request-a",
            "request-b",
        ] {
            fx.put(&admin, "artifact", id, &live(id)).await;
        }
        store_run(&fx, "revoked-upstream").await;
        fx.edges(
            &admin,
            &[("artifact", revoked_id.as_str(), "run", "revoked-upstream")],
        )
        .await;
        fx.put(
            &admin,
            "tombstone",
            "revoked-upstream",
            &tombstone("revoked-upstream", "run"),
        )
        .await;
        let mut dependencies = vec![live_dependency, revoked_dependency];
        let mut sources = vec![
            source_ref(&fx, &admin, "artifact", "request-a").await,
            source_ref(&fx, &admin, "artifact", "request-b").await,
        ];
        if reversed {
            dependencies.reverse();
            sources.reverse();
        }
        let nodes = [
            ("artifact", live_id.as_str()),
            ("artifact", revoked_id.as_str()),
            ("artifact", "request-a"),
            ("artifact", "request-b"),
            ("run", "revoked-upstream"),
        ];
        problems.extend(
            refused_manifest_stage(
                &fx,
                &admin,
                stage_request_for("manifest-mixed-stage", manifest_with(dependencies), sources),
                &nodes,
                &format!("mixed refs, reversed={reversed}"),
                ManifestRefusal::Forbidden,
            )
            .await,
        );
    }
    assert_manifest_problems(problems);
}

#[tokio::test]
async fn manifest_dependencies_are_artifacts_in_the_request_namespace() {
    let fx = Fx::open("manifest-namespace.sqlite3").await;
    let admin = admin();
    let other = Context::new("m", "admin", Role::Admin).unwrap();
    fx.bump(&other, "manifest-other-watermark").await;
    let dependency = manifest_dependency("namespace-dependency");
    let id = package_dependency_ref_id(&dependency).unwrap();
    fx.put(&admin, "artifact", &id, &live(&id)).await;
    fx.put(&other, "artifact", &id, &redacted(&id)).await;
    fx.put(&other, "tombstone", &id, &tombstone(&id, "artifact"))
        .await;
    let nodes = [("artifact", id.as_str())];
    let before_other = manifest_shape(&fx, &other, &nodes).await;
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request_for(
            "manifest-namespace-stage",
            manifest_with(vec![dependency]),
            vec![],
        ),
    )
    .await
    .unwrap();
    assert_eq!(staged.payload.state, StagedAssetState::Staged);
    assert_eq!(
        staged.source_refs,
        vec![source_ref(&fx, &admin, "artifact", &id).await]
    );
    // The file snapshot spans namespaces: this successful n stage adds its n blob,
    // while every m body, edge, audit row and watermark stays unchanged.
    let after_other = manifest_shape(&fx, &other, &nodes).await;
    assert_eq!(before_other.0.changes(&after_other.0), "files 0 -> 1");
    assert!(before_other.1.is_empty());
    assert_eq!(after_other.1.len(), 1);
    assert!(
        after_other.1[0]
            .0
            .starts_with(&format!("blobs/{}/", hash(b"n")))
    );
}

#[tokio::test]
async fn manifest_dependencies_keep_request_source_errors_before_the_union_gate() {
    let admin = admin();
    let mut problems = Vec::new();
    for case in [
        "wrong-digest",
        "missing-request",
        "duplicate-request",
        "duplicate-manifest-ref",
    ] {
        let fx = Fx::open("manifest-request-errors.sqlite3").await;
        let dependency = manifest_dependency("request-error-dependency");
        let id = package_dependency_ref_id(&dependency).unwrap();
        fx.put(&admin, "artifact", &id, &live(&id)).await;
        fx.put(
            &admin,
            "artifact",
            "request-source",
            &live("request-source"),
        )
        .await;
        let source = source_ref(&fx, &admin, "artifact", "request-source").await;
        let (sources, expected) = match case {
            "wrong-digest" => {
                fx.put(&admin, "tombstone", &id, &tombstone(&id, "artifact"))
                    .await;
                let mut changed = source;
                changed.digest = "a".repeat(64);
                (
                    vec![changed],
                    ManifestRefusal::Conflict("source changed: artifact:request-source"),
                )
            }
            "missing-request" => {
                fx.put(&admin, "tombstone", &id, &tombstone(&id, "artifact"))
                    .await;
                (
                    vec![E16SourceRef {
                        kind: "artifact".into(),
                        id: "missing-source".into(),
                        digest: hash(b"missing"),
                    }],
                    ManifestRefusal::NotFound,
                )
            }
            "duplicate-request" => (
                vec![source.clone(), source],
                ManifestRefusal::Conflict("duplicate E16 source ref"),
            ),
            "duplicate-manifest-ref" => (
                vec![source_ref(&fx, &admin, "artifact", &id).await],
                ManifestRefusal::Conflict("duplicate E16 source ref"),
            ),
            _ => unreachable!(),
        };
        let nodes = [
            ("artifact", id.as_str()),
            ("artifact", "request-source"),
            ("artifact", "missing-source"),
        ];
        problems.extend(
            refused_manifest_stage(
                &fx,
                &admin,
                stage_request_for(
                    "manifest-errors-stage",
                    manifest_with(vec![dependency]),
                    sources,
                ),
                &nodes,
                case,
                expected,
            )
            .await,
        );
    }
    assert_manifest_problems(problems);
}

#[tokio::test]
async fn missing_manifest_dependencies_still_quarantine_without_a_fabricated_source_ref() {
    let admin = admin();
    for with_resolved in [false, true] {
        let fx = Fx::open("manifest-missing.sqlite3").await;
        fx.put(
            &admin,
            "artifact",
            "request-source",
            &live("request-source"),
        )
        .await;
        let source = source_ref(&fx, &admin, "artifact", "request-source").await;
        let missing = manifest_dependency("missing-dependency");
        let missing_id = package_dependency_ref_id(&missing).unwrap();
        let mut dependencies = Vec::new();
        let mut expected_sources = vec![source.clone()];
        if with_resolved {
            let resolved = manifest_dependency("resolved-dependency");
            let resolved_id = package_dependency_ref_id(&resolved).unwrap();
            fx.put(&admin, "artifact", &resolved_id, &live(&resolved_id))
                .await;
            expected_sources.push(source_ref(&fx, &admin, "artifact", &resolved_id).await);
            dependencies.push(resolved);
        }
        dependencies.push(missing);
        expected_sources.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
        let request = stage_request_for(
            "manifest-missing-stage",
            manifest_with(dependencies),
            vec![source],
        );
        let staged = PersistentPackageStore::stage_package(&admin, &fx.store, request.clone())
            .await
            .unwrap();
        assert_eq!(staged.payload.state, StagedAssetState::Quarantined);
        assert_eq!(
            staged.payload.quarantine_reason.as_deref(),
            Some("unresolved_dependency:org.rsia:missing-dependency")
        );
        assert_eq!(staged.source_refs, expected_sources);
        assert!(fx.raw(&admin, "artifact", &missing_id).await.is_none());
        let nodes = [
            ("artifact", "request-source"),
            ("artifact", missing_id.as_str()),
        ];
        let before = manifest_shape(&fx, &admin, &nodes).await;
        let repeated = PersistentPackageStore::stage_package(&admin, &fx.store, request)
            .await
            .unwrap();
        assert_eq!(repeated.id, staged.id);
        assert_eq!(repeated.payload.state, StagedAssetState::Quarantined);
        assert_eq!(before, manifest_shape(&fx, &admin, &nodes).await);
    }
}

/// Two disjoint, fully stored stars. Each root's closure fits the bound; the
/// requested total includes both artifact roots and all their live candidate leaves.
/// Non-source kinds count toward the closure too, without consuming the run cap.
async fn store_manifest_joint_closure(
    fx: &Fx,
    dependency_id: &str,
    total: usize,
) -> Vec<(String, String)> {
    let admin = admin();
    let mut session = fx.store.session().await.unwrap();
    let mut nodes = Vec::new();
    let request_size = CLOSURE_BOUND / 2;
    for (root, prefix, size) in [
        ("joint-request", "request-leaf", request_size),
        (dependency_id, "manifest-leaf", total - request_size),
    ] {
        session
            .put(&admin, "artifact", root, admin.actor(), &live(root))
            .await
            .unwrap();
        nodes.push(("artifact".into(), root.into()));
        for index in 1..size {
            let id = format!("{prefix}-{index}");
            session
                .put(&admin, "candidate", &id, admin.actor(), &live(&id))
                .await
                .unwrap();
            session
                .put_edge(&admin, "artifact", root, "candidate", &id)
                .await
                .unwrap();
            nodes.push(("candidate".into(), id));
        }
        assert_eq!(
            session
                .upstream_closure(&admin, &[("artifact", root)], CLOSURE_BOUND)
                .await
                .unwrap()
                .len(),
            1 // Only source kinds are returned; all kinds are counted by the walk.
        );
        assert!(matches!(
            session
                .upstream_closure(&admin, &[("artifact", root)], size - 1)
                .await,
            Err(Error::Conflict(_))
        ));
    }
    assert_eq!(nodes.len(), total);
    session.commit().await.unwrap();
    nodes
}

#[tokio::test]
async fn manifest_and_request_closures_over_the_joint_bound_refuse_before_any_write() {
    let fx = Fx::open("manifest-joint-over-bound.sqlite3").await;
    let admin = admin();
    let dependency = manifest_dependency("joint-dependency");
    let id = package_dependency_ref_id(&dependency).unwrap();
    let nodes = store_manifest_joint_closure(&fx, &id, CLOSURE_BOUND + 1).await;
    let nodes = nodes
        .iter()
        .map(|(kind, id)| (kind.as_str(), id.as_str()))
        .collect::<Vec<_>>();
    let source = source_ref(&fx, &admin, "artifact", "joint-request").await;
    let problems = refused_manifest_stage(
        &fx,
        &admin,
        stage_request_for(
            "manifest-joint-stage",
            manifest_with(vec![dependency]),
            vec![source],
        ),
        &nodes,
        "5000 request nodes + 5001 manifest nodes",
        ManifestRefusal::Conflict(
            "upstream dependency closure exceeds 10000 nodes; it is refused, not truncated",
        ),
    )
    .await;
    assert_manifest_problems(problems);
}

#[tokio::test]
async fn manifest_and_request_closures_at_the_joint_bound_can_stage() {
    let fx = Fx::open("manifest-joint-at-bound.sqlite3").await;
    let admin = admin();
    let dependency = manifest_dependency("joint-dependency");
    let id = package_dependency_ref_id(&dependency).unwrap();
    store_manifest_joint_closure(&fx, &id, CLOSURE_BOUND).await;
    let source = source_ref(&fx, &admin, "artifact", "joint-request").await;
    let staged = PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request_for(
            "manifest-joint-stage",
            manifest_with(vec![dependency]),
            vec![source],
        ),
    )
    .await
    .unwrap();
    assert_eq!(staged.payload.state, StagedAssetState::Staged);
    assert_eq!(staged.source_refs.len(), 2);
}

#[tokio::test]
async fn known_boundary_a_manifest_dependency_with_another_kind_tombstone_still_fails_late() {
    let fx = Fx::open("manifest-other-kind.sqlite3").await;
    let admin = admin();
    let dependency = manifest_dependency("other-kind-dependency");
    let id = package_dependency_ref_id(&dependency).unwrap();
    fx.put(&admin, "artifact", &id, &live(&id)).await;
    fx.put(
        &admin,
        "artifact",
        "request-source",
        &live("request-source"),
    )
    .await;
    fx.put(&admin, "tombstone", &id, &tombstone(&id, "run"))
        .await;
    let source = source_ref(&fx, &admin, "artifact", "request-source").await;
    let nodes = [("artifact", id.as_str()), ("artifact", "request-source")];
    let before = manifest_shape(&fx, &admin, &nodes).await;
    let result = PersistentPackageStore::stage_package(
        &admin,
        &fx.store,
        stage_request_for(
            "manifest-other-kind-stage",
            manifest_with(vec![dependency]),
            vec![source],
        ),
    )
    .await;
    assert!(matches!(result, Err(Error::Forbidden)), "{result:?}");
    let after = manifest_shape(&fx, &admin, &nodes).await;
    assert_eq!(prepared_staged(&fx).await, 1);
    assert_eq!(
        before.0.changes(&after.0),
        "artifact objects 2 -> 3, edges into node 0 0 -> 1, edges into node 1 0 -> 1, audit rows 0 -> 1"
    );
    assert_eq!(before.1, after.1);
}

// ---------------------------------------------------------------------------
// Known boundary: storage's id-only check can still refuse another kind late
// ---------------------------------------------------------------------------

#[tokio::test]
async fn known_boundary_a_tombstone_of_the_other_kind_still_stops_a_new_stage_or_install_late() {
    // The gate no longer takes a tombstone of the other kind for a refusal (the
    // grant and the two reads above), so a new `stage_package` or `install` over
    // such a source gets through the first transaction. The storage check at blob
    // publication then refuses it by the id alone, whatever the kind the tombstone
    // was written for: `Forbidden`, after the `Prepared` envelope was committed. The
    // answer is the same as before this change; the refusal comes later, and leaves
    // the envelope behind (before, the id-only tombstone check of `verify_sources`
    // refused it first). In production no such pair exists: the creators of a run
    // and of an import source refuse each other's ids (AG-036). Making the storage
    // check kind-exact is the storage change this card leaves out.
    let admin = admin();
    for entry in [Entry::Stage, Entry::Install] {
        let fx = Fx::open("boundary-kind.sqlite3").await;
        fx.put(&admin, "artifact", "src-art", &live("src-art"))
            .await;
        let source = source_ref(&fx, &admin, "artifact", "src-art").await;
        fx.put(&admin, "tombstone", "src-art", &tombstone("src-art", "run"))
            .await;
        let result = match entry {
            Entry::Stage => PersistentPackageStore::stage_package(
                &admin,
                &fx.store,
                stage_request("boundary-stage", vec![source]),
            )
            .await
            .map(|_| ()),
            _ => PersistentSeedStore::install(
                &admin,
                &fx.store,
                install_request("boundary-install", "boundary-seed", vec![source]),
            )
            .await
            .map(|_| ()),
        };
        assert!(
            matches!(result, Err(Error::Forbidden)),
            "{entry:?}: {result:?}"
        );
        let left = if entry == Entry::Stage {
            prepared_staged(&fx).await
        } else {
            fx.artifacts_of(&admin, "rsia.e16.seed_install.v1")
                .await
                .iter()
                .filter(|body| body["payload"]["status"] == "prepared")
                .count()
        };
        assert_eq!(left, 1, "{entry:?}");
    }
}
