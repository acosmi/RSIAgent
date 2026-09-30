//! E16.5 startup recovery quarantine and deployment gate for `rsia serve` / `rsia mcp`.
//!
//! A data directory that `scripts/restore_backup.py` produced (it holds a
//! `restore.json` receipt) must not be mounted until it is proven to be the
//! restored state of an independent, operator-designated current control plane:
//! a restore must not resurrect revoked history or permissions, rewind consumed
//! query/exposure/budget facts, or reopen early-stopped tickets (E16.5, §11.3 to
//! §11.5, E08). The gate is a pure read-only decision; it never opens the
//! database through `Store::open` and every failure path writes nothing.
//!
//! State machine (data directory `D` = parent of the `--data` file):
//!
//! | `D/restore.json` | anchor | `D/restore.admission.json` | outcome |
//! |---|---|---|---|
//! | absent | none | (ignored) | [`RecoveryPosture::Normal`] |
//! | absent | given | (ignored) | `startup_rejected` |
//! | present | none | absent | quarantine (an anchor is required) |
//! | present | given | absent | verify, then [`RecoveryPosture::RestoredVerified`] or quarantine |
//! | present | none | valid, bound to this receipt | [`RecoveryPosture::AdmittedPreviously`] |
//! | present | given | valid, bound to this receipt | `startup_rejected` |
//! | present | any | present but unparsable or bound elsewhere | quarantine |
//!
//! The anchor is only ever the operator's explicit `--trusted-revocations-db`.
//! `path_local_only` inside `restore.json` lives in the directory being verified
//! and is therefore never trusted: it is only quoted in a diagnostic hint when
//! the anchor the operator names is a different file than the restore used.
//!
//! An admission is written by [`GateDecision::admit`] only after the caller has
//! bootstrapped the store and before anything recovers or serves. A crash before
//! the admission lands simply re-runs the verification on the next start.
//!
//! Code execution is never enabled here: [`deployment_config`] takes no
//! code-execution, sandbox or network input and the isolation facts come from
//! [`IsolationPolicy::reference_host`], the only production construction.

use crate::capacity::{
    DeploymentSecurityConfig, validate_code_execution_gate, validate_deployment_security,
};
use crate::executor::IsolationPolicy;
use evo_core::{Error, hash};
use evo_storage::lifecycle::{
    ControlPlaneFacts, WatermarkFact, parse_strict_json, read_control_plane_facts,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const RESTORE_RECEIPT_FILE: &str = "restore.json";
pub const RESTORE_ADMISSION_FILE: &str = "restore.admission.json";
pub const RESTORE_RECEIPT_SCHEMA: &str = "rsia.restore_receipt.v2";
pub const RESTORE_ADMISSION_SCHEMA: &str = "rsia.restore_admission.v1";
pub const RECEIPT_SCOPE: &str = "local_only_not_for_export";
/// A receipt or admission record is a few hundred bytes; anything larger is refused unread.
pub const MAX_RECEIPT_BYTES: u64 = 64 * 1024;
/// Name of the data-directory lock `rsia` takes; an anchor directory that holds it is live.
pub const DATA_LOCK_FILE: &str = ".rsia.lock";

const BACKUP_MANIFEST_FILE: &str = "backup-manifest.json";
const ANCHOR_WITHOUT_RESTORE: &str = "--trusted-revocations-db is only accepted for a restored data directory that has not been admitted";
/// How many differing keys a mismatch reason names.
const DIFF_SAMPLE: usize = 3;

/// Why the gate stopped a startup. The `Display` text is the operator-facing
/// contract the smoke test and the gate tests assert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupGateError {
    /// `startup_rejected: <reason>` — the invocation itself is wrong.
    Rejected(String),
    /// `recovery_quarantine: <reason>; not mounting` — the directory is isolated.
    Quarantine(String),
    /// `recovery_admission_failed: <reason>; not serving` — verified, but the
    /// admission record could not be made durable.
    Admission(String),
}

impl fmt::Display for StartupGateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(reason) => write!(f, "startup_rejected: {reason}"),
            Self::Quarantine(reason) => write!(f, "recovery_quarantine: {reason}; not mounting"),
            Self::Admission(reason) => {
                write!(f, "recovery_admission_failed: {reason}; not serving")
            }
        }
    }
}

impl std::error::Error for StartupGateError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPosture {
    /// No restore receipt: an ordinary data directory.
    Normal,
    /// A restored directory whose admission record is bound to its receipt.
    AdmittedPreviously,
    /// A restored directory verified against the operator's anchor just now.
    RestoredVerified,
}

impl RecoveryPosture {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::AdmittedPreviously => "admitted_previously",
            Self::RestoredVerified => "restored_verified",
        }
    }
}

/// The deployment configuration a CLI startup derives. Its signature has no
/// code-execution, sandbox or network input on purpose: `allow_code_execution`
/// is constantly false, so no flag, environment variable or configuration
/// boolean can request code execution through startup.
pub fn deployment_config(trusted_revocations_db: Option<&Path>) -> DeploymentSecurityConfig {
    DeploymentSecurityConfig {
        sandbox_enabled: false,
        allow_code_execution: false,
        allow_external_network: false,
        trusted_revocations_db_path: trusted_revocations_db
            .map(|path| path.to_string_lossy().into_owned()),
    }
}

/// The data directory of a `--data` SQLite file: its parent, with the empty
/// parent of a bare file name read as the current directory.
pub fn data_directory(data: &Path) -> PathBuf {
    match data.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// `D/restore.admission.json`. Field order is the on-disk order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionRecord {
    pub schema_version: String,
    /// sha256 of the exact `restore.json` bytes this admission covers.
    pub receipt_sha256: String,
    /// Canonical path of the anchor that was verified (diagnostic only).
    pub anchor_path_local_only: String,
    /// See [`facts_digest`].
    pub verified_facts_digest: String,
    /// Sorted namespaces whose facts were compared.
    pub namespaces: Vec<String>,
    /// Unix seconds.
    pub admitted_at: i64,
    pub scope: String,
}

/// What a verified, not yet admitted directory will record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAdmission {
    pub receipt_sha256: String,
    pub anchor_path_local_only: String,
    pub verified_facts_digest: String,
    pub namespaces: Vec<String>,
}

/// The outcome of [`StartupGate::evaluate`]. Nothing has been written yet.
#[derive(Debug, Clone)]
pub struct GateDecision {
    posture: RecoveryPosture,
    isolation: IsolationPolicy,
    data_dir: PathBuf,
    pending: Option<PendingAdmission>,
}

impl GateDecision {
    pub fn posture(&self) -> RecoveryPosture {
        self.posture
    }

    pub fn isolation(&self) -> &IsolationPolicy {
        &self.isolation
    }

    /// Present only for [`RecoveryPosture::RestoredVerified`].
    pub fn pending_admission(&self) -> Option<&PendingAdmission> {
        self.pending.as_ref()
    }

    /// The one-line posture every startup prints to stderr (stdout is the MCP
    /// JSON-RPC channel). Both isolation facts come from the isolation policy,
    /// not from literals.
    pub fn startup_line(&self) -> String {
        format!(
            "rsia startup: code_execution={} sandbox={} recovery={}",
            self.isolation.code_execution,
            if self.isolation.sandbox_available {
                "available"
            } else {
                "unavailable"
            },
            self.posture.label()
        )
    }

    /// Make the verification durable. Only a [`RecoveryPosture::RestoredVerified`]
    /// decision writes anything; call it after the store was bootstrapped and
    /// before recovery or serving. The record is written to a temporary name,
    /// fsynced, hard-linked to `restore.admission.json` (which fails rather than
    /// overwrites) and the directory is fsynced.
    pub fn admit(&self) -> Result<(), StartupGateError> {
        match &self.pending {
            Some(pending) => write_admission(&self.data_dir, pending),
            None => Ok(()),
        }
    }
}

/// Decides whether `data` may be mounted. Construct with the `--data` path and
/// the operator's `--trusted-revocations-db`, if any.
#[derive(Debug, Clone)]
pub struct StartupGate {
    data: PathBuf,
    trusted_anchor: Option<PathBuf>,
}

impl StartupGate {
    pub fn new(data: &Path, trusted_anchor: Option<&Path>) -> Self {
        Self {
            data: data.to_path_buf(),
            trusted_anchor: trusted_anchor.map(Path::to_path_buf),
        }
    }

    /// Read-only. Never creates, modifies or removes anything.
    pub async fn evaluate(&self) -> Result<GateDecision, StartupGateError> {
        let dir = data_directory(&self.data);
        let isolation = IsolationPolicy::reference_host(dir.to_string_lossy().into_owned());
        let decision = |posture, pending| GateDecision {
            posture,
            isolation: isolation.clone(),
            data_dir: dir.clone(),
            pending,
        };

        let receipt_path = dir.join(RESTORE_RECEIPT_FILE);
        if !entry_present(&receipt_path, "restore receipt")? {
            if self.trusted_anchor.is_some() {
                return Err(StartupGateError::Rejected(ANCHOR_WITHOUT_RESTORE.into()));
            }
            code_execution_gate(&isolation)?;
            return Ok(decision(RecoveryPosture::Normal, None));
        }

        let receipt_bytes = read_small_regular_file(&receipt_path, "restore receipt")?;
        let receipt_sha256 = hash(&receipt_bytes);

        let admission_path = dir.join(RESTORE_ADMISSION_FILE);
        if entry_present(&admission_path, "restore admission record")? {
            let admission = read_admission(&admission_path)?;
            if admission.receipt_sha256 != receipt_sha256 {
                return Err(quarantine(
                    "admission record is bound to a different restore receipt",
                ));
            }
            if self.trusted_anchor.is_some() {
                return Err(StartupGateError::Rejected(ANCHOR_WITHOUT_RESTORE.into()));
            }
            code_execution_gate(&isolation)?;
            return Ok(decision(RecoveryPosture::AdmittedPreviously, None));
        }

        let Some(anchor) = self.trusted_anchor.as_deref() else {
            return Err(quarantine(
                "restored data directory has no admission record and no --trusted-revocations-db anchor was given",
            ));
        };
        let pending = self
            .verify_restored(&dir, &receipt_bytes, receipt_sha256, anchor, &isolation)
            .await?;
        Ok(decision(RecoveryPosture::RestoredVerified, Some(pending)))
    }

    async fn verify_restored(
        &self,
        dir: &Path,
        receipt_bytes: &[u8],
        receipt_sha256: String,
        anchor: &Path,
        isolation: &IsolationPolicy,
    ) -> Result<PendingAdmission, StartupGateError> {
        // (a) the receipt
        let receipt = parse_receipt(receipt_bytes).map_err(quarantine)?;

        // (e) deployment gate, then (b) anchor shape and independence
        let anchor_text = anchor
            .to_str()
            .ok_or_else(|| quarantine("trusted anchor path is not valid UTF-8"))?;
        if !anchor.is_absolute() {
            return Err(quarantine(format!(
                "trusted anchor path must be absolute: {anchor_text}"
            )));
        }
        validate_deployment_security(&deployment_config(Some(anchor)), isolation)
            .map_err(|error| quarantine(error_reason(&error)))?;
        check_anchor_independence(anchor, &self.data, dir).map_err(quarantine)?;

        // (c) both databases, read-only, integrity- and migration-checked
        let data_facts = read_control_plane_facts(&self.data)
            .await
            .map_err(|error| {
                quarantine(format!("data directory database: {}", error_reason(&error)))
            })?;
        let anchor_facts = read_control_plane_facts(anchor).await.map_err(|error| {
            quarantine(format!("trusted anchor database: {}", error_reason(&error)))
        })?;

        // (d) equality
        compare_facts(&data_facts, &anchor_facts, &receipt).map_err(|reason| {
            quarantine(format!(
                "{reason}{}",
                anchor_hint(&receipt.trusted_anchor.path_local_only, anchor_text)
            ))
        })?;

        Ok(PendingAdmission {
            receipt_sha256,
            anchor_path_local_only: anchor_text.to_owned(),
            verified_facts_digest: facts_digest(&data_facts),
            namespaces: data_facts.watermarks.keys().cloned().collect(),
        })
    }
}

/// The receipt records which anchor the restore was verified against. That text
/// lives in the directory being verified and is never trusted; it only helps the
/// operator when the anchor named now is a different file.
fn anchor_hint(recorded: &str, given: &str) -> String {
    if recorded == given {
        return String::new();
    }
    let shown: String = recorded.chars().take(200).collect();
    format!(" [the restore was verified against {shown:?}, not this anchor]")
}

fn quarantine(reason: impl Into<String>) -> StartupGateError {
    StartupGateError::Quarantine(reason.into())
}

fn error_reason(error: &Error) -> String {
    match error {
        Error::Invalid(message) | Error::Conflict(message) => message.clone(),
        other => other.to_string(),
    }
}

/// The anchor-free deployment check a non-recovery startup runs.
fn code_execution_gate(isolation: &IsolationPolicy) -> Result<(), StartupGateError> {
    validate_code_execution_gate(&deployment_config(None), isolation)
        .map_err(|error| StartupGateError::Rejected(error_reason(&error)))
}

/// Whether `path` names anything. Only "not found" means absent: an entry that
/// cannot be inspected is not assumed to be missing.
fn entry_present(path: &Path, what: &str) -> Result<bool, StartupGateError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(quarantine(format!("cannot inspect {what}: {error}"))),
    }
}

/// Read a regular, non-symlink file of at most [`MAX_RECEIPT_BYTES`].
fn read_small_regular_file(path: &Path, what: &str) -> Result<Vec<u8>, StartupGateError> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|error| quarantine(format!("cannot inspect {what}: {error}")))?;
    if meta.file_type().is_symlink() {
        return Err(quarantine(format!("{what} must not be a symlink")));
    }
    if !meta.is_file() {
        return Err(quarantine(format!("{what} is not a regular file")));
    }
    if meta.len() > MAX_RECEIPT_BYTES {
        return Err(quarantine(format!(
            "{what} is larger than {MAX_RECEIPT_BYTES} bytes"
        )));
    }
    let file = std::fs::File::open(path)
        .map_err(|error| quarantine(format!("cannot read {what}: {error}")))?;
    // The path may have been swapped after the check: judge the open handle too.
    let opened = file
        .metadata()
        .map_err(|error| quarantine(format!("cannot inspect {what}: {error}")))?;
    if !opened.is_file() {
        return Err(quarantine(format!("{what} is not a regular file")));
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| quarantine(format!("cannot read {what}: {error}")))?;
    if u64::try_from(bytes.len()).map_or(true, |length| length > MAX_RECEIPT_BYTES) {
        return Err(quarantine(format!(
            "{what} is larger than {MAX_RECEIPT_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

fn is_lower_hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// ---------------------------------------------------------------------------
// (a) restore.json
// ---------------------------------------------------------------------------

/// `restore_backup.py` writes exactly these keys (`rsia.restore_receipt.v2`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreReceipt {
    schema_version: String,
    backup_manifest_sha256: String,
    #[allow(dead_code)]
    replayed_revoke_events: u64,
    isolated: bool,
    trusted_anchor: ReceiptAnchor,
    receipt_scope: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptAnchor {
    /// Diagnostic only: it sits inside the directory being verified, so it never
    /// decides anything (see [`anchor_hint`]).
    path_local_only: String,
    watermarks: BTreeMap<String, ReceiptWatermark>,
    control_plane_digest: String,
    #[allow(dead_code)]
    verified_at: String,
    #[allow(dead_code)]
    scope: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptWatermark {
    seq: u64,
    digest: String,
}

fn parse_receipt(bytes: &[u8]) -> Result<RestoreReceipt, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "restore receipt is not valid UTF-8".to_owned())?;
    let value = parse_strict_json(text).map_err(|error| {
        format!(
            "restore receipt is not strict JSON: {}",
            error_reason(&error)
        )
    })?;
    let Value::Object(object) = &value else {
        return Err("restore receipt is not a JSON object".into());
    };
    match object.get("schema_version").and_then(Value::as_str) {
        Some(RESTORE_RECEIPT_SCHEMA) => {}
        Some(other) => {
            return Err(format!(
                "unsupported restore receipt schema_version {other:?}; re-run restore_backup.py"
            ));
        }
        None => {
            return Err(
                "restore receipt has no schema_version (first-version receipt); re-run restore_backup.py"
                    .into(),
            );
        }
    }
    let receipt: RestoreReceipt = serde_json::from_value(value)
        .map_err(|error| format!("restore receipt does not match the v2 schema: {error}"))?;
    if receipt.schema_version != RESTORE_RECEIPT_SCHEMA {
        return Err("restore receipt schema_version changed during parsing".into());
    }
    if receipt.isolated {
        return Err("restore receipt says the restored directory is isolated".into());
    }
    if receipt.receipt_scope != RECEIPT_SCOPE {
        return Err(format!(
            "restore receipt scope is {:?}, not {RECEIPT_SCOPE:?}",
            receipt.receipt_scope
        ));
    }
    if !is_lower_hex64(&receipt.backup_manifest_sha256) {
        return Err("restore receipt backup_manifest_sha256 is not 64 lowercase hex digits".into());
    }
    if !is_lower_hex64(&receipt.trusted_anchor.control_plane_digest) {
        return Err("restore receipt control_plane_digest is not 64 lowercase hex digits".into());
    }
    if receipt.trusted_anchor.watermarks.is_empty() {
        return Err("restore receipt has no revoke watermark".into());
    }
    for (namespace, watermark) in &receipt.trusted_anchor.watermarks {
        if namespace.is_empty() || !is_lower_hex64(&watermark.digest) {
            return Err(format!(
                "restore receipt watermark for namespace {namespace:?} is malformed"
            ));
        }
    }
    Ok(receipt)
}

// ---------------------------------------------------------------------------
// admission record
// ---------------------------------------------------------------------------

fn read_admission(path: &Path) -> Result<AdmissionRecord, StartupGateError> {
    let bytes = read_small_regular_file(path, "restore admission record")?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| quarantine("restore admission record is not valid UTF-8"))?;
    let value = parse_strict_json(text).map_err(|error| {
        quarantine(format!(
            "restore admission record is not strict JSON: {}",
            error_reason(&error)
        ))
    })?;
    let record: AdmissionRecord = serde_json::from_value(value).map_err(|error| {
        quarantine(format!(
            "restore admission record does not match its schema: {error}"
        ))
    })?;
    let sorted_unique = record.namespaces.windows(2).all(|pair| pair[0] < pair[1]);
    if record.schema_version != RESTORE_ADMISSION_SCHEMA
        || record.scope != RECEIPT_SCOPE
        || !is_lower_hex64(&record.receipt_sha256)
        || !is_lower_hex64(&record.verified_facts_digest)
        || record.anchor_path_local_only.is_empty()
        || record.namespaces.is_empty()
        || record.namespaces.iter().any(String::is_empty)
        || !sorted_unique
        || record.admitted_at < 0
    {
        return Err(quarantine("restore admission record is malformed"));
    }
    Ok(record)
}

fn write_admission(dir: &Path, pending: &PendingAdmission) -> Result<(), StartupGateError> {
    let failed = |what: &str, error: &dyn fmt::Display| {
        StartupGateError::Admission(format!("{what}: {error}"))
    };
    let record = AdmissionRecord {
        schema_version: RESTORE_ADMISSION_SCHEMA.into(),
        receipt_sha256: pending.receipt_sha256.clone(),
        anchor_path_local_only: pending.anchor_path_local_only.clone(),
        verified_facts_digest: pending.verified_facts_digest.clone(),
        namespaces: pending.namespaces.clone(),
        admitted_at: evo_core::now(),
        scope: RECEIPT_SCOPE.into(),
    };
    let mut bytes = serde_json::to_vec(&record)
        .map_err(|error| failed("cannot serialize the admission record", &error))?;
    bytes.push(b'\n');

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let temp = dir.join(format!(
        ".{RESTORE_ADMISSION_FILE}.tmp-{}-{unique}",
        std::process::id()
    ));
    let target = dir.join(RESTORE_ADMISSION_FILE);

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| failed("cannot create the admission temp file", &error))?;
    let linked = file
        .write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| failed("cannot write the admission temp file", &error))
        .and_then(|()| {
            // Fails if the final name exists: an admission is never overwritten.
            std::fs::hard_link(&temp, &target)
                .map_err(|error| failed("cannot publish the admission record", &error))
        });
    drop(file);
    let removed = std::fs::remove_file(&temp);
    linked?;
    removed.map_err(|error| failed("cannot remove the admission temp file", &error))?;
    std::fs::File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| failed("cannot fsync the data directory", &error))
}

// ---------------------------------------------------------------------------
// (b) anchor independence
// ---------------------------------------------------------------------------

/// The anchor must be independent of the directory it vouches for. The absolute
/// path requirement and the shape checks (regular file, not a symlink, SQLite
/// header) already ran before this; it adds the ones that need `D`.
fn check_anchor_independence(anchor: &Path, data_db: &Path, data_dir: &Path) -> Result<(), String> {
    let canonical = std::fs::canonicalize(anchor)
        .map_err(|error| format!("cannot resolve the trusted anchor path: {error}"))?;
    if canonical != anchor {
        return Err(format!(
            "trusted anchor path is not canonical (a symlink or relative component in its path); use {}",
            canonical.display()
        ));
    }

    check_anchor_file_identity(anchor, data_db)?;

    let data_canonical = std::fs::canonicalize(data_dir)
        .map_err(|error| format!("cannot resolve the data directory: {error}"))?;
    if canonical.starts_with(&data_canonical) {
        return Err("trusted anchor lies inside the data directory being verified".into());
    }
    let parent = canonical
        .parent()
        .ok_or_else(|| "trusted anchor has no parent directory".to_owned())?;
    match std::fs::symlink_metadata(parent.join(BACKUP_MANIFEST_FILE)) {
        Ok(_) => {
            return Err(format!(
                "trusted anchor directory contains {BACKUP_MANIFEST_FILE}; it holds a backup, not a current control plane"
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot inspect the trusted anchor directory: {error}"
            ));
        }
    }
    probe_anchor_lock(parent)
}

/// The anchor is not the data directory's database under another name, and has
/// no other name of its own.
#[cfg(unix)]
fn check_anchor_file_identity(anchor: &Path, data_db: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let anchor_meta = std::fs::symlink_metadata(anchor)
        .map_err(|error| format!("cannot inspect the trusted anchor: {error}"))?;
    let data_meta = std::fs::metadata(data_db)
        .map_err(|error| format!("cannot inspect the data directory database: {error}"))?;
    if anchor_meta.dev() == data_meta.dev() && anchor_meta.ino() == data_meta.ino() {
        return Err(
            "trusted anchor is the data directory's own database (same device and inode)".into(),
        );
    }
    if anchor_meta.nlink() != 1 {
        return Err(format!(
            "trusted anchor has {} hard links; it must be the only name of its file",
            anchor_meta.nlink()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_anchor_file_identity(_anchor: &Path, _data_db: &Path) -> Result<(), String> {
    Err("trusted anchor identity cannot be verified on this platform".into())
}

/// An anchor directory whose `.rsia.lock` is held belongs to a running RSIA
/// process, whose state is still moving. The probe takes a shared lock on a
/// read-only handle and releases it immediately.
fn probe_anchor_lock(anchor_dir: &Path) -> Result<(), String> {
    let lock_path = anchor_dir.join(DATA_LOCK_FILE);
    match std::fs::symlink_metadata(&lock_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot inspect the anchor lock file: {error}")),
        Ok(meta) if !meta.is_file() => {
            return Err("the anchor lock path is not a regular file".into());
        }
        Ok(_) => {}
    }
    let file = std::fs::File::open(&lock_path)
        .map_err(|error| format!("cannot open the anchor lock file: {error}"))?;
    match file.try_lock_shared() {
        Ok(()) => {
            let _ = file.unlock();
            Ok(())
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            Err("anchor is held by a running RSIA process (its .rsia.lock is locked)".into())
        }
        Err(std::fs::TryLockError::Error(error)) => {
            Err(format!("cannot probe the anchor lock file: {error}"))
        }
    }
}

// ---------------------------------------------------------------------------
// (d) equality
// ---------------------------------------------------------------------------

/// Names up to [`DIFF_SAMPLE`] keys that differ between the data directory and
/// the anchor. Only keys are named, never values.
fn describe_diff<V: PartialEq>(
    data: &BTreeMap<String, V>,
    anchor: &BTreeMap<String, V>,
) -> Option<String> {
    let sample = |keys: Vec<&String>| {
        let shown: Vec<&str> = keys
            .iter()
            .take(DIFF_SAMPLE)
            .map(|key| key.as_str())
            .collect();
        let more = keys.len().saturating_sub(DIFF_SAMPLE);
        if more == 0 {
            shown.join(", ")
        } else {
            format!("{}, and {more} more", shown.join(", "))
        }
    };
    let only_anchor: Vec<&String> = anchor
        .keys()
        .filter(|key| !data.contains_key(*key))
        .collect();
    let only_data: Vec<&String> = data
        .keys()
        .filter(|key| !anchor.contains_key(*key))
        .collect();
    let changed: Vec<&String> = data
        .iter()
        .filter(|(key, value)| anchor.get(*key).is_some_and(|other| other != *value))
        .map(|(key, _)| key)
        .collect();
    if only_anchor.is_empty() && only_data.is_empty() && changed.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !only_anchor.is_empty() {
        parts.push(format!(
            "only in the trusted anchor: {}",
            sample(only_anchor)
        ));
    }
    if !only_data.is_empty() {
        parts.push(format!("only in the data directory: {}", sample(only_data)));
    }
    if !changed.is_empty() {
        parts.push(format!("changed: {}", sample(changed)));
    }
    Some(parts.join("; "))
}

fn compare_facts(
    data: &ControlPlaneFacts,
    anchor: &ControlPlaneFacts,
    receipt: &RestoreReceipt,
) -> Result<(), String> {
    const RERUN: &str = "re-run restore_backup.py from a fresh backup";

    // Watermarks: the data directory against the anchor first (an anchor that
    // moved on is the actionable case), then against the receipt.
    if data.watermarks.is_empty() {
        return Err("data directory database has no revoke watermark".into());
    }
    let new_namespaces: Vec<&str> = anchor
        .watermarks
        .keys()
        .filter(|namespace| !data.watermarks.contains_key(*namespace))
        .map(String::as_str)
        .collect();
    if !new_namespaces.is_empty() {
        return Err(format!(
            "trusted anchor advanced since restore (new namespace(s): {}); {RERUN}",
            new_namespaces.join(", ")
        ));
    }
    let unknown_namespaces: Vec<&str> = data
        .watermarks
        .keys()
        .filter(|namespace| !anchor.watermarks.contains_key(*namespace))
        .map(String::as_str)
        .collect();
    if !unknown_namespaces.is_empty() {
        return Err(format!(
            "data directory has namespace(s) the trusted anchor does not know: {}",
            unknown_namespaces.join(", ")
        ));
    }
    for (namespace, data_mark) in &data.watermarks {
        let anchor_mark = &anchor.watermarks[namespace];
        if anchor_mark.seq > data_mark.seq {
            return Err(format!(
                "trusted anchor advanced since restore (namespace {namespace}: seq {} → {}); {RERUN}",
                data_mark.seq, anchor_mark.seq
            ));
        }
        if anchor_mark.seq < data_mark.seq {
            return Err(format!(
                "data directory watermark is ahead of the trusted anchor (namespace {namespace}: seq {} vs {})",
                data_mark.seq, anchor_mark.seq
            ));
        }
        if anchor_mark.digest != data_mark.digest {
            return Err(format!(
                "watermark digest differs at seq {} (namespace {namespace})",
                data_mark.seq
            ));
        }
    }
    let receipt_marks = &receipt.trusted_anchor.watermarks;
    let receipt_matches = receipt_marks.len() == data.watermarks.len()
        && data
            .watermarks
            .iter()
            .all(|(namespace, mark): (&String, &WatermarkFact)| {
                receipt_marks.get(namespace).is_some_and(|stored| {
                    i64::try_from(stored.seq) == Ok(mark.seq) && stored.digest == mark.digest
                })
            });
    if !receipt_matches {
        return Err("data directory watermarks differ from the restore receipt".into());
    }

    let tombstones = |facts: &ControlPlaneFacts| -> BTreeMap<String, Value> {
        facts
            .tombstones
            .iter()
            .map(|((namespace, id), body)| (format!("{namespace}/{id}"), body.clone()))
            .collect()
    };
    if let Some(difference) = describe_diff(&tombstones(data), &tombstones(anchor)) {
        return Err(format!(
            "revocation tombstones differ between the data directory and the trusted anchor ({difference}); {RERUN}"
        ));
    }

    let objects = |facts: &ControlPlaneFacts| -> BTreeMap<String, Value> {
        facts
            .protected_objects
            .iter()
            .map(|((namespace, kind, id), body)| (format!("{namespace}/{kind}/{id}"), body.clone()))
            .collect()
    };
    if let Some(difference) = describe_diff(&objects(data), &objects(anchor)) {
        return Err(format!(
            "consumed accounting facts differ between the data directory and the trusted anchor ({difference}); {RERUN}"
        ));
    }

    if let Some(difference) = describe_diff(&data.root_budget_tables, &anchor.root_budget_tables) {
        return Err(format!(
            "root budget tables differ between the data directory and the trusted anchor ({difference}); {RERUN}"
        ));
    }
    Ok(())
}

/// Digest of the facts a verification compared, stored in the admission record.
///
/// The preimage is one canonical JSON document (object keys sorted, no
/// whitespace, strings escaped by `serde_json`):
///
/// ```text
/// {"schema":"rsia.restore_admission.verified_facts.v1",
///  "watermarks":{"<ns>":{"digest":"..","seq":N}},
///  "tombstones":[["<ns>","<id>",<body>],..]            // sorted by (ns, id)
///  "protected_objects":[["<ns>","<kind>","<id>",<body>],..]   // sorted by (ns, kind, id)
///  "root_budget_tables":{"<table>":["<quote()d row>",..]}}    // rows sorted
/// ```
///
/// Bodies are the parsed JSON values. This is Rust's own definition: it does
/// not reproduce the `control_plane_digest` in `restore.json`, whose Python
/// `json.dumps` canonicalization differs, which is why the two databases are
/// compared directly instead.
pub fn facts_digest(facts: &ControlPlaneFacts) -> String {
    let watermarks: serde_json::Map<String, Value> = facts
        .watermarks
        .iter()
        .map(|(namespace, mark)| {
            (
                namespace.clone(),
                serde_json::json!({"seq": mark.seq, "digest": mark.digest}),
            )
        })
        .collect();
    let tombstones: Vec<Value> = facts
        .tombstones
        .iter()
        .map(|((namespace, id), body)| serde_json::json!([namespace, id, body]))
        .collect();
    let protected: Vec<Value> = facts
        .protected_objects
        .iter()
        .map(|((namespace, kind, id), body)| serde_json::json!([namespace, kind, id, body]))
        .collect();
    let tables: serde_json::Map<String, Value> = facts
        .root_budget_tables
        .iter()
        .map(|(table, rows)| (table.clone(), serde_json::json!(rows)))
        .collect();
    let document = serde_json::json!({
        "schema": "rsia.restore_admission.verified_facts.v1",
        "watermarks": watermarks,
        "tombstones": tombstones,
        "protected_objects": protected,
        "root_budget_tables": tables,
    });
    let mut canonical = String::new();
    canonical_json(&document, &mut canonical);
    hash(canonical.as_bytes())
}

fn canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical_json(item, out);
            }
            out.push(']');
        }
        Value::Object(object) => {
            let keys: BTreeSet<&String> = object.keys().collect();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical_json(&object[key], out);
            }
            out.push('}');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_directory_reads_a_bare_file_name_as_the_current_directory() {
        assert_eq!(data_directory(Path::new("rsia.sqlite3")), Path::new("."));
        assert_eq!(data_directory(Path::new("./rsia.sqlite3")), Path::new("."));
        assert_eq!(
            data_directory(Path::new("/var/lib/rsia/rsia.sqlite3")),
            Path::new("/var/lib/rsia")
        );
        assert_eq!(data_directory(Path::new("/")), Path::new("."));
    }

    #[test]
    fn deployment_config_can_never_request_code_execution_or_network() {
        for anchor in [None, Some(Path::new("/anchor/rsia.sqlite3"))] {
            let cfg = deployment_config(anchor);
            assert!(!cfg.allow_code_execution);
            assert!(!cfg.sandbox_enabled);
            assert!(!cfg.allow_external_network);
            assert_eq!(
                cfg.trusted_revocations_db_path.as_deref(),
                anchor.and_then(Path::to_str)
            );
        }
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_depth() {
        let left: Value = serde_json::from_str(r#"{"b":[{"y":1,"x":2}],"a":"é\n"}"#).unwrap();
        let right: Value = serde_json::from_str(r#"{"a":"é\n","b":[{"x":2,"y":1}]}"#).unwrap();
        let (mut a, mut b) = (String::new(), String::new());
        canonical_json(&left, &mut a);
        canonical_json(&right, &mut b);
        assert_eq!(a, r#"{"a":"é\n","b":[{"x":2,"y":1}]}"#);
        assert_eq!(a, b);
    }

    #[test]
    fn facts_digest_changes_with_every_compared_fact() {
        let base = ControlPlaneFacts {
            watermarks: BTreeMap::from([(
                "n".to_owned(),
                WatermarkFact {
                    seq: 1,
                    digest: "d".repeat(64),
                },
            )]),
            tombstones: BTreeMap::new(),
            protected_objects: BTreeMap::new(),
            root_budget_tables: BTreeMap::new(),
        };
        let digest = facts_digest(&base);
        assert_eq!(digest.len(), 64);
        assert_eq!(digest, facts_digest(&base.clone()));
        let mut moved = base.clone();
        moved.watermarks.get_mut("n").unwrap().seq = 2;
        let mut tombstoned = base.clone();
        tombstoned
            .tombstones
            .insert(("n".into(), "t".into()), serde_json::json!({"id": "t"}));
        let mut accounted = base.clone();
        accounted.protected_objects.insert(
            ("n".into(), "budget".into(), "b".into()),
            serde_json::json!({"id": "b"}),
        );
        let mut budgeted = base.clone();
        budgeted
            .root_budget_tables
            .insert("root_budgets".into(), vec!["1,'a'".into()]);
        let digests: BTreeSet<String> = [&base, &moved, &tombstoned, &accounted, &budgeted]
            .into_iter()
            .map(facts_digest)
            .collect();
        assert_eq!(digests.len(), 5);
    }

    #[test]
    fn describe_diff_names_a_bounded_sample_of_keys_and_never_values() {
        let data: BTreeMap<String, Value> = BTreeMap::from([
            ("a".to_owned(), serde_json::json!(1)),
            ("b".to_owned(), serde_json::json!("SECRET-old")),
            ("c".to_owned(), serde_json::json!(3)),
        ]);
        let anchor: BTreeMap<String, Value> = BTreeMap::from([
            ("b".to_owned(), serde_json::json!("SECRET-new")),
            ("c".to_owned(), serde_json::json!(3)),
            ("d".to_owned(), serde_json::json!(4)),
        ]);
        let text = describe_diff(&data, &anchor).unwrap();
        assert_eq!(
            text,
            "only in the trusted anchor: d; only in the data directory: a; changed: b"
        );
        assert!(!text.contains("SECRET"));
        assert_eq!(describe_diff(&data, &data), None);

        let many: BTreeMap<String, Value> = (0..6)
            .map(|index| (format!("k{index}"), serde_json::json!(index)))
            .collect();
        assert_eq!(
            describe_diff(&BTreeMap::new(), &many).unwrap(),
            "only in the trusted anchor: k0, k1, k2, and 3 more"
        );
    }

    #[test]
    fn the_anchor_hint_only_speaks_when_the_named_anchor_differs_from_the_recorded_one() {
        assert_eq!(anchor_hint("/a/rsia.sqlite3", "/a/rsia.sqlite3"), "");
        let hint = anchor_hint("/recorded\nnewline", "/given");
        assert!(hint.contains("/recorded\\nnewline"), "{hint}");
        assert!(!hint.contains('\n'), "control characters are escaped");
        assert!(anchor_hint(&"x".repeat(1000), "/given").len() < 300);
    }

    #[test]
    fn startup_line_reports_the_isolation_facts_not_literals() {
        let isolation = IsolationPolicy::reference_host("/d");
        let decision = GateDecision {
            posture: RecoveryPosture::AdmittedPreviously,
            isolation,
            data_dir: PathBuf::from("/d"),
            pending: None,
        };
        assert_eq!(
            decision.startup_line(),
            "rsia startup: code_execution=disabled sandbox=unavailable recovery=admitted_previously"
        );
    }

    #[test]
    fn error_display_is_the_operator_contract() {
        assert_eq!(
            StartupGateError::Quarantine("why".into()).to_string(),
            "recovery_quarantine: why; not mounting"
        );
        assert_eq!(
            StartupGateError::Rejected(ANCHOR_WITHOUT_RESTORE.into()).to_string(),
            "startup_rejected: --trusted-revocations-db is only accepted for a restored data directory that has not been admitted"
        );
    }
}
