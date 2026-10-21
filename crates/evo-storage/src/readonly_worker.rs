//! Fixed live-WAL reader. Pipe data is never an authority or a log record.
use crate::lifecycle::DirectoryWriterGuard;
use evo_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{File, Metadata},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    sync::Arc,
    thread::JoinHandle,
};

const ARG: &str = "--rsia-private-original-read-v1";
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Operation {
    RawSnapshot,
    ControlFacts,
    RestoreSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    dev: u64,
    ino: u64,
    mode: u32,
    uid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    directory: Member,
    database: Member,
    shm: Member,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    nonce: String,
    build: String,
    operation: Operation,
    path: Vec<u8>,
    source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Stage {
    Ready,
    Closed,
    Refused,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fault {
    code: String,
    message: Option<String>,
}

impl Fault {
    fn from_error(error: Error) -> Self {
        let (code, message) = match error {
            Error::Invalid(message) => ("invalid", Some(message)),
            Error::Conflict(message) => ("conflict", Some(message)),
            Error::NotFound => ("not_found", None),
            Error::Forbidden => ("forbidden", None),
            Error::Budget => ("budget", None),
            Error::Cancelled => ("cancelled", None),
            Error::ClockRegressed => ("clock_regressed", None),
            Error::Internal => ("internal", None),
        };
        Self {
            code: code.into(),
            message,
        }
    }
    fn into_error(self) -> Result<Error> {
        match (self.code.as_str(), self.message) {
            ("invalid", Some(message)) => Ok(Error::Invalid(message)),
            ("conflict", Some(message)) => Ok(Error::Conflict(message)),
            ("not_found", None) => Ok(Error::NotFound),
            ("forbidden", None) => Ok(Error::Forbidden),
            ("budget", None) => Ok(Error::Budget),
            ("cancelled", None) => Ok(Error::Cancelled),
            ("clock_regressed", None) => Ok(Error::ClockRegressed),
            ("internal", None) => Ok(Error::Internal),
            _ => Err(Error::ClockRegressed),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    version: u32,
    nonce: String,
    build: String,
    operation: Operation,
    pid: u32,
    source: Source,
    stage: Stage,
    snapshot_digest: Option<String>,
    payload: Option<Value>,
    error: Option<Fault>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Continue {
    version: u32,
    nonce: String,
    pid: u32,
}

/// Only run_live can create this; it is not deserializable and carries no
/// Current, role, native writer or publication permission.
pub(super) struct ClosedPayload {
    payload: Value,
}
impl ClosedPayload {
    pub(super) fn into_value(self) -> Value {
        self.payload
    }
}

fn refused<T>() -> Result<T> {
    Err(Error::ClockRegressed)
}

fn exact(value: &Value, keys: &[&str]) -> Result<()> {
    let object = value.as_object().ok_or(Error::ClockRegressed)?;
    if object.len() != keys.len() || keys.iter().any(|key| !object.contains_key(*key)) {
        return refused();
    }
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8], keys: &[&str]) -> Result<T> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::ClockRegressed)?;
    let value = crate::owned_occurrence::parse_owned_json(text)?;
    exact(&value, keys)?;
    serde_json::from_value(value).map_err(|_| Error::ClockRegressed)
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| Error::Internal)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn payload_digest(value: &Value) -> Result<String> {
    evo_core::fingerprint(value)
}

fn build_id() -> Result<String> {
    // Explicit source and dependency identity for this fixed reader. The actual
    // fresh-target receipts bind toolchain, profile and executable separately.
    let parts = [
        evo_core::hash(include_bytes!("budget.rs")),
        evo_core::hash(include_bytes!("dependency_read.rs")),
        evo_core::hash(include_bytes!("e05_issue.rs")),
        evo_core::hash(include_bytes!("lib.rs")),
        evo_core::hash(include_bytes!("lifecycle.rs")),
        evo_core::hash(include_bytes!("migration_clock.rs")),
        evo_core::hash(include_bytes!("owned_occurrence.rs")),
        evo_core::hash(include_bytes!("readonly_worker.rs")),
        evo_core::hash(include_bytes!("replay.rs")),
        evo_core::hash(include_bytes!("typed_budget.rs")),
        evo_core::hash(include_bytes!("write_clock.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/clock.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/contract.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/curriculum.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/evaluation.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/evidence.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/features.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/formal_version.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/holdout.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/import_fixture.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/improver.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/lib.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/management_registry.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/model_protocol.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/optimization.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/prepare_contract.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/replay.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/replay_economics.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/sequential.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/skill_edit.rs")),
        evo_core::hash(include_bytes!("../../evo-core/src/strategy.rs")),
        evo_core::hash(include_bytes!("../migrations/0001_runtime.sql")),
        evo_core::hash(include_bytes!("../migrations/0002_revoke_graph.sql")),
        evo_core::hash(include_bytes!("../migrations/0004_replay_worlds.sql")),
        evo_core::hash(include_bytes!("../migrations/0005_root_budget.sql")),
        evo_core::hash(include_bytes!("../migrations/0006_revoke_cleanup.sql")),
        evo_core::hash(include_bytes!("../migrations/0007_formal_parent_kind.sql")),
        evo_core::hash(include_bytes!("../../../apps/rsia/src/main.rs")),
        evo_core::hash(include_bytes!("../Cargo.toml")),
        evo_core::hash(include_bytes!("../../evo-core/Cargo.toml")),
        evo_core::hash(include_bytes!("../../../apps/rsia/Cargo.toml")),
        evo_core::hash(include_bytes!("../../../Cargo.toml")),
        evo_core::hash(include_bytes!("../../../Cargo.lock")),
        env!("CARGO_PKG_VERSION").to_owned(),
        std::env::consts::ARCH.to_owned(),
        std::env::consts::OS.to_owned(),
    ];
    evo_core::fingerprint(&parts.as_slice())
}

#[cfg(unix)]
fn member(path: &Path, directory: bool) -> Result<Member> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).map_err(|_| Error::ClockRegressed)?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return refused();
    }
    Ok(Member {
        dev: metadata.dev(),
        ino: metadata.ino(),
        mode: metadata.mode(),
        uid: metadata.uid(),
    })
}

#[cfg(not(unix))]
fn member(_path: &Path, _directory: bool) -> Result<Member> {
    refused()
}

fn source(path: &Path) -> Result<Source> {
    let directory = path.parent().ok_or(Error::ClockRegressed)?;
    let mut shm = path.as_os_str().to_owned();
    shm.push("-shm");
    Ok(Source {
        directory: member(directory, true)?,
        database: member(path, false)?,
        shm: member(Path::new(&shm), false)?,
    })
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn from_path_bytes(bytes: Vec<u8>) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    if bytes.contains(&0) {
        return refused();
    }
    let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
    if !path.is_absolute() {
        return refused();
    }
    Ok(path)
}

#[cfg(not(unix))]
fn path_bytes(_path: &Path) -> Vec<u8> {
    Vec::new()
}
#[cfg(not(unix))]
fn from_path_bytes(_bytes: Vec<u8>) -> Result<PathBuf> {
    refused()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExecutableIdentity {
    member: Member,
    bytes: u64,
    modified: std::time::SystemTime,
}
struct Executable {
    path: PathBuf,
    retained: File,
    identity: ExecutableIdentity,
}

fn executable_identity(path: &Path, metadata: Metadata) -> Result<ExecutableIdentity> {
    Ok(ExecutableIdentity {
        member: member(path, false)?,
        bytes: metadata.len(),
        modified: metadata.modified().map_err(|_| Error::ClockRegressed)?,
    })
}

impl Executable {
    fn locate() -> Result<Self> {
        let current = std::env::current_exe()
            .map_err(|_| Error::ClockRegressed)?
            .canonicalize()
            .map_err(|_| Error::ClockRegressed)?;
        let current_member = member(&current, false)?;
        let directory = current.parent().ok_or(Error::ClockRegressed)?;
        let path = if current.file_name().is_some_and(|name| name == "rsia") {
            current.clone()
        } else if directory.file_name().is_some_and(|name| name == "deps") {
            directory
                .parent()
                .ok_or(Error::ClockRegressed)?
                .join("rsia")
        } else {
            directory.join("rsia")
        };
        // No PATH/environment search, stale alternate profile, or arbitrary
        // program fallback. Installed library hosts use the same fixed sibling.
        let retained = File::open(&path).map_err(|_| Error::ClockRegressed)?;
        let identity = executable_identity(
            &path,
            retained.metadata().map_err(|_| Error::ClockRegressed)?,
        )?;
        if identity.member.uid != current_member.uid
            || identity.member.mode & 0o022 != 0
            || identity.member.mode & 0o111 == 0
        {
            return refused();
        }
        let executable = Self {
            path,
            retained,
            identity,
        };
        executable.recheck()?;
        Ok(executable)
    }

    fn recheck(&self) -> Result<()> {
        let metadata = self
            .retained
            .metadata()
            .map_err(|_| Error::ClockRegressed)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.dev() != self.identity.member.dev
                || metadata.ino() != self.identity.member.ino
            {
                return refused();
            }
        }
        if executable_identity(&self.path, metadata)? != self.identity {
            return refused();
        }
        Ok(())
    }
}

/// Unknown terminal retains the real mount, never a path or caller boolean.
struct UntilTerminal<T> {
    retained: Option<Arc<T>>,
    observed: bool,
}
impl<T> Drop for UntilTerminal<T> {
    fn drop(&mut self) {
        if !self.observed {
            std::mem::forget(self.retained.take());
        }
    }
}

struct Owner {
    child: Option<Child>,
    input: Option<ChildStdin>,
    output: Option<BufReader<ChildStdout>>,
    error: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    stdout_eof: bool,
    lifetime: UntilTerminal<DirectoryWriterGuard>,
}

impl Owner {
    fn line(&mut self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let count = self
            .output
            .as_mut()
            .ok_or(Error::ClockRegressed)?
            .read_until(b'\n', &mut bytes)
            .map_err(|_| Error::ClockRegressed)?;
        if count == 0 || bytes.last() != Some(&b'\n') {
            return refused();
        }
        Ok(bytes)
    }

    fn finish(&mut self) -> Result<(ExitStatus, Vec<u8>)> {
        let mut trailing = Vec::new();
        self.output
            .as_mut()
            .ok_or(Error::ClockRegressed)?
            .read_to_end(&mut trailing)
            .map_err(|_| Error::ClockRegressed)?;
        self.stdout_eof = true;
        let status = self
            .child
            .as_mut()
            .ok_or(Error::ClockRegressed)?
            .wait()
            .map_err(|_| Error::ClockRegressed)?;
        self.child.take();
        let stderr = self
            .error
            .take()
            .ok_or(Error::ClockRegressed)?
            .join()
            .map_err(|_| Error::ClockRegressed)?
            .map_err(|_| Error::ClockRegressed)?;
        // Both actual pipes reached EOF and the actual owned child was waited.
        self.lifetime.observed = self.stdout_eof;
        if !trailing.is_empty() {
            return refused();
        }
        Ok((status, stderr))
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        drop(self.input.take());
        let Some(child) = self.child.as_mut() else {
            return;
        };
        let _ = child.kill();
        let waited = child.wait().is_ok();
        if !waited {
            std::mem::forget(self.child.take());
            std::mem::forget(self.output.take());
            std::mem::forget(self.error.take());
            return;
        }
        self.child.take();
        let eof = self
            .output
            .take()
            .is_some_and(|mut output| std::io::copy(&mut output, &mut std::io::sink()).is_ok());
        let error_eof = self
            .error
            .take()
            .is_some_and(|handle| matches!(handle.join(), Ok(Ok(_))));
        self.lifetime.observed = eof && error_eof;
    }
}

fn frame(bytes: &[u8]) -> Result<Frame> {
    decode(
        bytes,
        &[
            "version",
            "nonce",
            "build",
            "operation",
            "pid",
            "source",
            "stage",
            "snapshot_digest",
            "payload",
            "error",
        ],
    )
}

fn binding(frame: &Frame, request: &Request, pid: u32) -> Result<()> {
    if frame.version != VERSION
        || frame.nonce != request.nonce
        || frame.build != request.build
        || frame.operation != request.operation
        || frame.pid != pid
        || frame.source != request.source
    {
        return refused();
    }
    Ok(())
}

fn ready(frame: &Frame) -> Result<&str> {
    let digest = frame
        .snapshot_digest
        .as_deref()
        .ok_or(Error::ClockRegressed)?;
    if frame.stage != Stage::Ready
        || frame.payload.is_some()
        || frame.error.is_some()
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return refused();
    }
    Ok(digest)
}

fn closed(
    frame: Frame,
    request: &Request,
    pid: u32,
    digest: &str,
    success: bool,
) -> Result<ClosedPayload> {
    binding(&frame, request, pid)?;
    if !success {
        return match (frame.stage, frame.payload, frame.error) {
            (Stage::Refused, None, Some(error)) => Err(error.into_error()?),
            _ => refused(),
        };
    }
    if frame.stage != Stage::Closed
        || frame.error.is_some()
        || frame.snapshot_digest.as_deref() != Some(digest)
    {
        return refused();
    }
    let payload = frame.payload.ok_or(Error::ClockRegressed)?;
    if payload_digest(&payload)? != digest {
        return refused();
    }
    Ok(ClosedPayload { payload })
}

pub(super) fn run_live(
    path: &Path,
    operation: Operation,
    mount: Option<Arc<DirectoryWriterGuard>>,
) -> Result<ClosedPayload> {
    run_live_with_ready(path, operation, mount, || Ok(()))
}

pub(super) fn run_live_with_ready(
    path: &Path,
    operation: Operation,
    mount: Option<Arc<DirectoryWriterGuard>>,
    on_ready: impl FnOnce() -> Result<()>,
) -> Result<ClosedPayload> {
    let path = path.canonicalize().map_err(|_| Error::ClockRegressed)?;
    let executable = Executable::locate()?;
    let request = Request {
        version: VERSION,
        nonce: uuid::Uuid::new_v4().to_string(),
        build: build_id()?,
        operation,
        path: path_bytes(&path),
        source: source(&path)?,
    };
    let child = Command::new(&executable.path)
        .arg(ARG)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| Error::ClockRegressed)?;
    let pid = child.id();
    let mut owner = Owner {
        child: Some(child),
        input: None,
        output: None,
        error: None,
        stdout_eof: false,
        lifetime: UntilTerminal {
            retained: mount,
            observed: false,
        },
    };
    owner.input = owner.child.as_mut().and_then(|child| child.stdin.take());
    owner.output = owner
        .child
        .as_mut()
        .and_then(|child| child.stdout.take())
        .map(BufReader::new);
    let mut error = owner
        .child
        .as_mut()
        .and_then(|child| child.stderr.take())
        .ok_or(Error::ClockRegressed)?;
    owner.error = Some(
        std::thread::Builder::new()
            .name("rsia-original-read-stderr".into())
            .spawn(move || {
                let mut bytes = Vec::new();
                error.read_to_end(&mut bytes)?;
                Ok(bytes)
            })
            .map_err(|_| Error::ClockRegressed)?,
    );
    owner
        .input
        .as_mut()
        .ok_or(Error::ClockRegressed)?
        .write_all(&encode(&request)?)
        .map_err(|_| Error::ClockRegressed)?;
    owner
        .input
        .as_mut()
        .ok_or(Error::ClockRegressed)?
        .flush()
        .map_err(|_| Error::ClockRegressed)?;
    let first = frame(&owner.line()?)?;
    binding(&first, &request, pid)?;
    if first.stage == Stage::Refused {
        drop(owner.input.take());
        let (status, _stderr) = owner.finish()?;
        executable.recheck()?;
        if source(&path)? != request.source {
            return refused();
        }
        return closed(first, &request, pid, "", status.success());
    }
    let digest = ready(&first)?.to_owned();
    on_ready()?;
    let resume = Continue {
        version: VERSION,
        nonce: request.nonce.clone(),
        pid,
    };
    let mut input = owner.input.take().ok_or(Error::ClockRegressed)?;
    input
        .write_all(&encode(&resume)?)
        .map_err(|_| Error::ClockRegressed)?;
    input.flush().map_err(|_| Error::ClockRegressed)?;
    drop(input);
    let result = frame(&owner.line()?)?;
    let (status, stderr) = owner.finish()?;
    executable.recheck()?;
    if source(&path)? != request.source {
        return refused();
    }
    if status.success() && !stderr.is_empty() {
        return refused();
    }
    closed(result, &request, pid, &digest, status.success())
}

fn write_frame(output: &mut impl Write, frame: &Frame) -> Result<()> {
    output
        .write_all(&encode(frame)?)
        .map_err(|_| Error::Internal)?;
    output.flush().map_err(|_| Error::Internal)
}

fn child_main() -> Result<()> {
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut line = Vec::new();
    input
        .read_until(b'\n', &mut line)
        .map_err(|_| Error::Internal)?;
    let request: Request = decode(
        &line,
        &["version", "nonce", "build", "operation", "path", "source"],
    )?;
    if request.version != VERSION
        || request.build != build_id()?
        || uuid::Uuid::parse_str(&request.nonce).is_err()
    {
        return refused();
    }
    let path = from_path_bytes(request.path.clone())?;
    if path.canonicalize().map_err(|_| Error::ClockRegressed)? != path
        || source(&path)? != request.source
    {
        return refused();
    }
    let mut output = std::io::stdout().lock();
    let make = |stage, snapshot_digest, payload, error| Frame {
        version: VERSION,
        nonce: request.nonce.clone(),
        build: request.build.clone(),
        operation: request.operation,
        pid: std::process::id(),
        source: request.source.clone(),
        stage,
        snapshot_digest,
        payload,
        error,
    };
    let result = crate::lifecycle::fresh_live_read_worker_r50(&path, |connection| {
        let first = match request.operation {
            Operation::RawSnapshot => crate::owned_occurrence::raw_worker_value_r50(connection)?,
            Operation::RestoreSnapshot => {
                crate::owned_occurrence::restore_worker_value_r50(connection)?
            }
            Operation::ControlFacts => {
                crate::lifecycle::control_facts_worker_value_r50(connection)?
            }
        };
        let digest = payload_digest(&first)?;
        write_frame(
            &mut output,
            &make(Stage::Ready, Some(digest.clone()), None, None),
        )?;
        let mut resume = Vec::new();
        input
            .read_until(b'\n', &mut resume)
            .map_err(|_| Error::Internal)?;
        let resume: Continue = decode(&resume, &["version", "nonce", "pid"])?;
        if resume.version != VERSION
            || resume.nonce != request.nonce
            || resume.pid != std::process::id()
        {
            return refused();
        }
        let mut trailing = Vec::new();
        input
            .read_to_end(&mut trailing)
            .map_err(|_| Error::Internal)?;
        if !trailing.is_empty() {
            return refused();
        }
        let second = match request.operation {
            Operation::RawSnapshot => crate::owned_occurrence::raw_worker_value_r50(connection)?,
            Operation::RestoreSnapshot => {
                crate::owned_occurrence::restore_worker_value_r50(connection)?
            }
            Operation::ControlFacts => {
                crate::lifecycle::control_facts_worker_value_r50(connection)?
            }
        };
        if payload_digest(&second)? != digest {
            return refused();
        }
        Ok((digest, second))
    });
    // fresh_live_read_worker_r50 has completed actual rollback/native close.
    match result {
        Ok((digest, payload)) => {
            if source(&path)? != request.source {
                return refused();
            }
            write_frame(
                &mut output,
                &make(Stage::Closed, Some(digest), Some(payload), None),
            )
        }
        Err(error) => {
            write_frame(
                &mut output,
                &make(Stage::Refused, None, None, Some(Fault::from_error(error))),
            )?;
            Err(Error::ClockRegressed)
        }
    }
}

/// A hidden fixed data-reader entry, checked before any normal CLI bootstrap.
/// It never opens a Store, accepts SQL, or imports role/authority fields.
pub(super) fn entry_if_requested() -> Option<i32> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let restore = if arguments == [std::ffi::OsString::from(RESTORE_READ_ARG_R50)] {
        Some(RestoreEntryR50::Read)
    } else if arguments == [std::ffi::OsString::from(RESTORE_COPY_ARG_R50)] {
        Some(RestoreEntryR50::Copy)
    } else {
        None
    };
    if let Some(entry) = restore {
        return Some(
            match std::panic::catch_unwind(|| restore_entry_main_r50(entry)) {
                Ok(Ok(())) => 0,
                _ => 71,
            },
        );
    }
    if arguments != [std::ffi::OsString::from(ARG)] {
        return None;
    }
    Some(match std::panic::catch_unwind(child_main) {
        Ok(Ok(())) => 0,
        _ => 71,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request {
        let member = Member {
            dev: 1,
            ino: 2,
            mode: 0o100600,
            uid: 1,
        };
        Request {
            version: VERSION,
            nonce: uuid::Uuid::new_v4().to_string(),
            build: "fixed-build".into(),
            operation: Operation::RawSnapshot,
            path: b"/db".to_vec(),
            source: Source {
                directory: member.clone(),
                database: member.clone(),
                shm: member,
            },
        }
    }
    fn completion(request: &Request, payload: Value) -> Frame {
        Frame {
            version: VERSION,
            nonce: request.nonce.clone(),
            build: request.build.clone(),
            operation: request.operation,
            pid: 17,
            source: request.source.clone(),
            stage: Stage::Closed,
            snapshot_digest: Some(payload_digest(&payload).unwrap()),
            payload: Some(payload),
            error: None,
        }
    }
    #[test]
    fn completed_data_requires_binding_terminal_and_untruncated_payload() {
        let request = request();
        let payload = serde_json::json!({"tables":[]});
        let digest = payload_digest(&payload).unwrap();
        assert!(
            closed(
                completion(&request, payload.clone()),
                &request,
                17,
                &digest,
                true
            )
            .is_ok()
        );
        assert!(
            closed(
                completion(&request, payload.clone()),
                &request,
                17,
                &digest,
                false
            )
            .is_err()
        );
        for change in 0..7 {
            let mut value = completion(&request, payload.clone());
            match change {
                0 => value.nonce.push('x'),
                1 => value.operation = Operation::ControlFacts,
                2 => value.build.push('x'),
                3 => value.pid += 1,
                4 => value.source.database.ino += 1,
                5 => value.stage = Stage::Ready,
                _ => value.payload = None,
            }
            assert!(closed(value, &request, 17, &digest, true).is_err());
        }
        let bytes = encode(&completion(&request, payload)).unwrap();
        assert!(frame(&bytes[..bytes.len() - 2]).is_err());
        assert!(frame(b"{\"version\":1,\"version\":1}").is_err());
        let mut trailing = bytes;
        trailing.extend_from_slice(b"{}");
        assert!(frame(&trailing).is_err());
    }
    #[test]
    fn unknown_terminal_retains_actual_resource_and_known_terminal_releases() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Probe(Arc<AtomicUsize>);
        impl Drop for Probe {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicUsize::new(0));
        let known = Arc::new(Probe(Arc::clone(&dropped)));
        drop(UntilTerminal {
            retained: Some(known),
            observed: true,
        });
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        let unknown = Arc::new(Probe(Arc::clone(&dropped)));
        let observer = Arc::clone(&unknown);
        drop(UntilTerminal {
            retained: Some(unknown),
            observed: false,
        });
        assert_eq!(Arc::strong_count(&observer), 2);
        drop(observer);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}

const RESTORE_READ_ARG_R50: &str = "--rsia-private-restore-read-v1";
const RESTORE_COPY_ARG_R50: &str = "--rsia-private-restore-copy-v1";
#[derive(Clone, Copy)]
enum RestoreEntryR50 {
    Read,
    Copy,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreEntryRequestR50 {
    version: u32,
    nonce: String,
    payload: Value,
}

fn restore_entry_main_r50(entry: RestoreEntryR50) -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Internal)?;
    let request: RestoreEntryRequestR50 = decode(&bytes, &["version", "nonce", "payload"])?;
    if request.version != VERSION || uuid::Uuid::parse_str(&request.nonce).is_err() {
        return refused();
    }
    let operation = match entry {
        RestoreEntryR50::Read => "restore_read",
        RestoreEntryR50::Copy => "restore_copy",
    };
    let result = match entry {
        RestoreEntryR50::Read => {
            let input: crate::lifecycle::RestoreReadRequestR50 =
                serde_json::from_value(request.payload.clone())
                    .map_err(|_| Error::ClockRegressed)?;
            if serde_json::to_value(&input).map_err(|_| Error::Internal)? != request.payload {
                return refused();
            }
            crate::lifecycle::restore_read_r50(input)
        }
        RestoreEntryR50::Copy => {
            let input: crate::lifecycle::RestoreCopyRequestR50 =
                serde_json::from_value(request.payload.clone())
                    .map_err(|_| Error::ClockRegressed)?;
            if serde_json::to_value(&input).map_err(|_| Error::Internal)? != request.payload {
                return refused();
            }
            let handle = tokio::runtime::Handle::try_current().map_err(|_| Error::Internal)?;
            if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
                return refused();
            }
            // rsia's existing runtime drives the existing migration adapter.
            // No runtime, thread, service or general-purpose SQL interface is created.
            tokio::task::block_in_place(|| {
                handle.block_on(crate::lifecycle::restore_copy_r50(input))
            })
        }
    };
    let (payload, error, success) = match result {
        Ok(value) => (Some(value), None, true),
        Err(error) => (None, Some(Fault::from_error(error)), false),
    };
    let value = serde_json::json!({
        "version":VERSION,"nonce":request.nonce,"pid":std::process::id(),
        "build":build_id()?,"operation":operation,
        "request_sha256":evo_core::hash(&bytes),"result":payload,"error":error
    });
    let mut output = std::io::stdout().lock();
    output
        .write_all(&encode(&value)?)
        .map_err(|_| Error::Internal)?;
    output.flush().map_err(|_| Error::Internal)?;
    if success {
        Ok(())
    } else {
        Err(Error::ClockRegressed)
    }
}
