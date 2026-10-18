//! Isolated task execution and root budget. Candidate code execution stays disabled.
use evo_core::{Error, Result, identifier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

pub const CODE_EXECUTION: &str = "disabled";

/// Observed OS identity, constructed only by the trusted fixed runtime check.
/// Role names and caller-provided PIDs cannot construct this capability.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ObservedProcessIdentity {
    pub pid: u32,
    pub uid: u32,
    pub executable_digest: String,
    pub workspace: String,
}
pub(crate) fn observe_registered_process(
    uid: u32,
    code: &str,
    workspace: &str,
) -> Result<ObservedProcessIdentity> {
    use std::os::unix::fs::MetadataExt;
    let path = std::fs::canonicalize(workspace).map_err(|_| Error::Forbidden)?;
    let cwd = std::env::current_dir().map_err(|_| Error::Internal)?;
    let metadata = std::fs::metadata(&path).map_err(|_| Error::Forbidden)?;
    if cwd != path || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(Error::Forbidden);
    }
    // Fixed platform utility; no shell or candidate-controlled executable.
    let observed = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .env_clear()
        .output()
        .map_err(|_| Error::Internal)?;
    if !observed.status.success() {
        return Err(Error::Forbidden);
    }
    let observed_uid: u32 = std::str::from_utf8(&observed.stdout)
        .map_err(|_| Error::Internal)?
        .trim()
        .parse()
        .map_err(|_| Error::Internal)?;
    let executable = std::env::current_exe().map_err(|_| Error::Internal)?;
    let bytes = std::fs::read(executable).map_err(|_| Error::Internal)?;
    let actual_digest = evo_core::hash(&bytes);
    if observed_uid != uid || actual_digest != code {
        return Err(Error::Forbidden);
    }
    Ok(ObservedProcessIdentity {
        pid: std::process::id(),
        uid,
        executable_digest: actual_digest,
        workspace: path.to_string_lossy().into_owned(),
    })
}

/// Kernel peer credentials are obtained from the connected Unix socket. No
/// JSON field, Role, actor alias, PID or environment variable can mint this.
#[derive(Debug, Clone)]
pub(crate) struct ObservedPeerIdentity {
    uid: u32,
    gid: u32,
    pid: Option<u32>,
}
impl ObservedPeerIdentity {
    pub(crate) fn uid(&self) -> u32 {
        self.uid
    }
    pub(crate) fn gid(&self) -> u32 {
        self.gid
    }
    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }
}
pub(crate) fn observe_kernel_peer(stream: &tokio::net::UnixStream) -> Result<ObservedPeerIdentity> {
    let cred = stream.peer_cred().map_err(|_| Error::Forbidden)?;
    let pid = cred
        .pid()
        .map(u32::try_from)
        .transpose()
        .map_err(|_| Error::Forbidden)?;
    Ok(ObservedPeerIdentity {
        uid: cred.uid(),
        gid: cred.gid(),
        pid,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPhase {
    Reserved,
    Dispatched,
    Finalized,
    Uncertain,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudget {
    pub id: String,
    pub billing_scope: String,
    pub limit: i64,
    pub reserved: i64,
    pub spent: i64,
    pub phase: BudgetPhase,
    pub lease_token: String,
    pub lease_until: i64,
}

impl RootBudget {
    pub fn open(
        id: impl Into<String>,
        billing_scope: impl Into<String>,
        limit: i64,
        lease_token: impl Into<String>,
        now: i64,
    ) -> Result<Self> {
        let id = id.into();
        let billing_scope = billing_scope.into();
        let lease_token = lease_token.into();
        identifier(&id)?;
        identifier(&billing_scope)?;
        identifier(&lease_token)?;
        if limit < 0 {
            return Err(Error::Invalid("budget limit must be >= 0".into()));
        }
        let lease_until = now
            .checked_add(60)
            .ok_or_else(|| Error::Invalid("lease time overflow".into()))?;
        Ok(Self {
            id,
            billing_scope,
            limit,
            reserved: 0,
            spent: 0,
            phase: BudgetPhase::Reserved,
            lease_token,
            lease_until,
        })
    }

    pub fn reserve(&mut self, amount: i64) -> Result<()> {
        if self.phase != BudgetPhase::Reserved {
            return Err(Error::Conflict("budget not reservable".into()));
        }
        if amount <= 0 {
            return Err(Error::Invalid("reservation must be positive".into()));
        }
        let committed = self
            .reserved
            .checked_add(self.spent)
            .and_then(|value| value.checked_add(amount))
            .ok_or(Error::Budget)?;
        if committed > self.limit {
            return Err(Error::Budget);
        }
        self.reserved = self.reserved.checked_add(amount).ok_or(Error::Budget)?;
        Ok(())
    }

    pub fn dispatch(&mut self, token: &str, now: i64) -> Result<()> {
        self.fence(token, now)?;
        if self.phase != BudgetPhase::Reserved {
            return Err(Error::Conflict("dispatch requires reserved phase".into()));
        }
        self.phase = BudgetPhase::Dispatched;
        Ok(())
    }

    pub fn finalize(&mut self, token: &str, now: i64, actual: i64) -> Result<()> {
        self.fence(token, now)?;
        if self.phase != BudgetPhase::Dispatched {
            return Err(Error::Conflict("finalize requires dispatched phase".into()));
        }
        if actual < 0 || actual > self.reserved {
            return Err(Error::Invalid("actual usage out of reservation".into()));
        }
        self.spent = self.spent.checked_add(actual).ok_or(Error::Budget)?;
        self.reserved = 0;
        self.phase = BudgetPhase::Finalized;
        Ok(())
    }

    pub fn timeout_uncertain(&mut self, token: &str, now: i64) -> Result<()> {
        self.fence(token, now)?;
        if self.phase != BudgetPhase::Dispatched {
            return Err(Error::Conflict(
                "uncertain usage only after dispatch".into(),
            ));
        }
        self.phase = BudgetPhase::Uncertain;
        Ok(())
    }

    pub fn release_after_uncertain(&self) -> Result<()> {
        Err(Error::Conflict(
            "unknown usage is not released and not auto-resent".into(),
        ))
    }

    /// Cancel without refunding paid usage or releasing dispatched reservations.
    pub fn cancel(&mut self, token: &str, now: i64) -> Result<()> {
        self.fence(token, now)?;
        match self.phase {
            BudgetPhase::Reserved => {
                self.reserved = 0;
                self.phase = BudgetPhase::Cancelled;
                Ok(())
            }
            BudgetPhase::Dispatched => {
                self.phase = BudgetPhase::Uncertain;
                Ok(())
            }
            BudgetPhase::Cancelled | BudgetPhase::Uncertain => Ok(()),
            BudgetPhase::Finalized => Err(Error::Conflict("budget not cancellable".into())),
        }
    }

    fn fence(&self, token: &str, now: i64) -> Result<()> {
        if token != self.lease_token {
            return Err(Error::Cancelled);
        }
        if now > self.lease_until {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
}

pub fn same_billing_scope_cannot_split(a: &RootBudget, b: &RootBudget) -> Result<()> {
    if a.billing_scope == b.billing_scope && a.id != b.id {
        return Err(Error::Conflict(
            "namespaces cannot split one billing scope into independent roots".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct IsolationPolicy {
    pub workspace: String,
    pub allow_network: bool,
    pub sandbox_available: bool,
    pub code_execution: &'static str,
}

impl IsolationPolicy {
    pub fn reference_host(workspace: impl Into<String>) -> Self {
        Self {
            workspace: workspace.into(),
            allow_network: false,
            sandbox_available: false,
            code_execution: CODE_EXECUTION,
        }
    }

    pub fn deny_escape(&self, path: &str) -> Result<()> {
        let forbidden = [
            "/var/run/docker.sock",
            "/proc/1",
            "/etc/passwd",
            "answers.json",
            "rsia.sqlite3",
            ".env",
            "credentials",
        ];
        if forbidden.iter().any(|f| path.contains(f)) {
            return Err(Error::Forbidden);
        }
        let p = Path::new(path);
        if p.is_absolute() && !path.starts_with(&self.workspace) {
            return Err(Error::Forbidden);
        }
        Ok(())
    }

    pub fn execute_code(&self, _blob: &[u8]) -> Result<()> {
        if self.code_execution != "enabled" {
            return Err(Error::Invalid("code execution disabled".into()));
        }
        Err(Error::Invalid("code execution disabled".into()))
    }

    pub fn run_shell(&self, _cmd: &str) -> Result<()> {
        if !self.sandbox_available {
            return Err(Error::Invalid("sandbox_unavailable".into()));
        }
        Err(Error::Forbidden)
    }
}

pub fn broker_model_call(
    authorized: bool,
    destination: &str,
    allowlist: &BTreeSet<String>,
) -> Result<()> {
    if !authorized {
        return Err(Error::Forbidden);
    }
    if !allowlist.contains(destination) {
        return Err(Error::Forbidden);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolation_blocks_answers_db_and_docker() {
        let iso = IsolationPolicy::reference_host("/tmp/ws");
        assert!(iso.deny_escape("/tmp/ws/answers.json").is_err());
        assert!(iso.deny_escape("/tmp/rsia.sqlite3").is_err());
        assert!(iso.deny_escape("/var/run/docker.sock").is_err());
        assert!(iso.deny_escape("/tmp/ws/task.txt").is_ok());
        assert!(
            iso.run_shell("echo hi")
                .unwrap_err()
                .to_string()
                .contains("sandbox_unavailable")
        );
        assert!(iso.execute_code(b"fn main(){}").is_err());
    }

    #[test]
    fn timeout_does_not_release_or_resend() {
        let mut b = RootBudget::open("b1", "scope-a", 100, "lease1", 0).unwrap();
        b.reserve(10).unwrap();
        b.dispatch("lease1", 1).unwrap();
        b.timeout_uncertain("lease1", 2).unwrap();
        assert!(b.release_after_uncertain().is_err());
        assert!(b.dispatch("lease1", 3).is_err());
        assert_eq!(b.phase, BudgetPhase::Uncertain);
    }

    #[test]
    fn stale_lease_is_fenced() {
        let mut b = RootBudget::open("b1", "scope-a", 100, "lease1", 0).unwrap();
        b.reserve(10).unwrap();
        assert!(b.dispatch("other", 1).is_err());
        assert!(b.dispatch("lease1", 120).is_err());
    }

    #[test]
    fn one_billing_scope_across_namespaces() {
        let a = RootBudget::open("b1", "scope-a", 100, "l1", 0).unwrap();
        let b = RootBudget::open("b2", "scope-a", 100, "l2", 0).unwrap();
        assert!(same_billing_scope_cannot_split(&a, &b).is_err());
    }

    #[test]
    fn model_broker_uses_allowlist() {
        let allow = ["https://api.example".into()].into();
        assert!(broker_model_call(true, "https://api.example", &allow).is_ok());
        assert!(broker_model_call(true, "https://evil", &allow).is_err());
        assert!(broker_model_call(false, "https://api.example", &allow).is_err());
    }

    #[test]
    fn root_budget_arithmetic_overflow_is_rejected_without_panicking() {
        let mut budget = RootBudget::open("b1", "scope-a", i64::MAX, "lease1", 0).unwrap();
        budget.reserve(i64::MAX).unwrap();
        assert!(matches!(budget.reserve(1), Err(Error::Budget)));
        assert_eq!(budget.reserved, i64::MAX);
        assert!(RootBudget::open("b2", "scope-b", 1, "lease2", i64::MAX).is_err());
    }
}
