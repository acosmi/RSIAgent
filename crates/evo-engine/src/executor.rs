//! Isolated task execution and root budget. Candidate code execution stays disabled.
use evo_core::{Error, Result, identifier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

pub const CODE_EXECUTION: &str = "disabled";

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
        Ok(Self {
            id,
            billing_scope,
            limit,
            reserved: 0,
            spent: 0,
            phase: BudgetPhase::Reserved,
            lease_token,
            lease_until: now + 60,
        })
    }

    pub fn reserve(&mut self, amount: i64) -> Result<()> {
        if self.phase != BudgetPhase::Reserved {
            return Err(Error::Conflict("budget not reservable".into()));
        }
        if amount <= 0 {
            return Err(Error::Invalid("reservation must be positive".into()));
        }
        if self.reserved + self.spent + amount > self.limit {
            return Err(Error::Budget);
        }
        self.reserved += amount;
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
        self.spent += actual;
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

    pub fn cancel(&mut self, token: &str, now: i64) -> Result<()> {
        self.fence(token, now)?;
        match self.phase {
            BudgetPhase::Reserved | BudgetPhase::Dispatched => {
                self.phase = BudgetPhase::Cancelled;
                Ok(())
            }
            BudgetPhase::Uncertain => Err(Error::Conflict(
                "uncertain bills stay for manual reconcile".into(),
            )),
            _ => Err(Error::Conflict("budget not cancellable".into())),
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
}
