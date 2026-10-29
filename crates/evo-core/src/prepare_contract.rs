//! Pure prepare configuration and its borrowed typed operation contract.
//! Serialization retains the original host-service field declaration order.

use crate::contract::{CapabilityLevel, HostCapabilities, SystemSnapshot};
use crate::{Error, Result, identifier, text};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostPrepareConfig {
    pub profile_id: String,
    pub system_snapshot: SystemSnapshot,
    pub host_surface_id: String,
    pub host_capabilities: HostCapabilities,
    pub evolution_enabled: bool,
    pub capability_level: CapabilityLevel,
}

impl HostPrepareConfig {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.profile_id)?;
        identifier(&self.host_surface_id)?;
        if self.system_snapshot.profile_id != self.profile_id {
            return Err(Error::Conflict("system snapshot profile mismatch".into()));
        }
        if self.system_snapshot.schema_version != "rsia.system_snapshot.v2" {
            return Err(Error::Invalid("unsupported system snapshot schema".into()));
        }
        for value in [
            &self.system_snapshot.profile_id,
            &self.system_snapshot.host_id,
            &self.system_snapshot.model_id,
        ] {
            identifier(value)?;
        }
        text(&self.system_snapshot.host_version, "host_version", 128)?;
        validate_digest(
            &self.system_snapshot.mandatory_context_digest,
            "mandatory_context_digest",
        )?;
        if self.system_snapshot.tools.len() > 64 {
            return Err(Error::Invalid("too many host tools".into()));
        }
        ensure_unique(&self.system_snapshot.tools, "duplicate host tool")?;
        for tool in &self.system_snapshot.tools {
            identifier(tool)?;
        }
        for capability in self
            .host_capabilities
            .available
            .iter()
            .chain(&self.host_capabilities.granted)
        {
            identifier(capability)?;
        }
        if !self
            .host_capabilities
            .granted
            .is_subset(&self.host_capabilities.available)
        {
            return Err(Error::Conflict(
                "granted capabilities must be available".into(),
            ));
        }
        self.system_snapshot.digest()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PrepareOperation<'a> {
    pub request: &'a crate::Prepare,
    pub config: &'a HostPrepareConfig,
}

/// Complete producer configuration. Legacy configuration/operation bytes above
/// retain their original meaning and never synthesize a missing tool source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteHostPrepareConfig {
    pub profile_id: String,
    pub system_snapshot: crate::contract::SystemSnapshotV3,
    pub host_surface_id: String,
    pub host_capabilities: HostCapabilities,
    pub evolution_enabled: bool,
    pub capability_level: CapabilityLevel,
}
impl CompleteHostPrepareConfig {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.profile_id)?;
        identifier(&self.host_surface_id)?;
        if self.system_snapshot.profile_id != self.profile_id {
            return Err(Error::Conflict("system snapshot profile mismatch".into()));
        }
        self.system_snapshot.validate()?;
        for capability in self
            .host_capabilities
            .available
            .iter()
            .chain(&self.host_capabilities.granted)
        {
            identifier(capability)?;
        }
        if !self
            .host_capabilities
            .granted
            .is_subset(&self.host_capabilities.available)
        {
            return Err(Error::Conflict(
                "granted capabilities must be available".into(),
            ));
        }
        self.system_snapshot
            .environment_digest(&self.host_capabilities)?;
        Ok(())
    }
    /// Startup adapter only: source is supplied by the actual protocol producer,
    /// never loaded from or written back into a historical configuration record.
    pub fn from_producer(
        legacy: &HostPrepareConfig,
        source: crate::contract::ToolSourceV1,
    ) -> Result<Self> {
        legacy.validate()?;
        let snapshot = &legacy.system_snapshot;
        let complete = Self {
            profile_id: legacy.profile_id.clone(),
            system_snapshot: crate::contract::SystemSnapshotV3 {
                schema_version: crate::contract::SYSTEM_SNAPSHOT_SCHEMA_V3.into(),
                profile_id: snapshot.profile_id.clone(),
                host_id: snapshot.host_id.clone(),
                host_version: snapshot.host_version.clone(),
                model_id: snapshot.model_id.clone(),
                tools: source.names()?,
                mandatory_context_digest: snapshot.mandatory_context_digest.clone(),
                tool_source: source,
            },
            host_surface_id: legacy.host_surface_id.clone(),
            host_capabilities: legacy.host_capabilities.clone(),
            evolution_enabled: legacy.evolution_enabled,
            capability_level: legacy.capability_level,
        };
        complete.validate()?;
        Ok(complete)
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct CompletePrepareOperation<'a> {
    pub schema_version: &'static str,
    pub request: &'a crate::Prepare,
    pub config: &'a CompleteHostPrepareConfig,
}

fn ensure_unique(values: &[String], message: &str) -> Result<()> {
    let unique = values.iter().collect::<std::collections::BTreeSet<_>>();
    if unique.len() != values.len() {
        return Err(Error::Conflict(message.into()));
    }
    Ok(())
}

fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(Error::Invalid(format!(
            "{name}: expected lowercase sha256 digest"
        )));
    }
    Ok(())
}
