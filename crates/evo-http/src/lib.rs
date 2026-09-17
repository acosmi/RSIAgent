//! Authenticated host HTTP API. Identity never comes from the JSON body.
use evo_core::{Context, Error, Result, Role, identifier};
use serde_json::Value;

pub const DEFAULT_BIND: &str = "127.0.0.1:7788";

pub fn context_from_trusted_headers(namespace: &str, actor: &str, role: Role) -> Result<Context> {
    Context::new(namespace, actor, role)
}

pub fn reject_body_identity(body: &Value) -> Result<()> {
    for key in ["actor", "role", "namespace"] {
        if body.get(key).is_some() {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}

pub fn parse_role(raw: &str) -> Result<Role> {
    identifier(raw)?;
    match raw {
        "agent" => Ok(Role::Agent),
        "host" => Ok(Role::Host),
        "evaluator" => Ok(Role::Evaluator),
        "admin" => Ok(Role::Admin),
        "worker" => Ok(Role::Worker),
        _ => Err(Error::Invalid("unknown role".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn body_cannot_mint_admin() {
        assert!(reject_body_identity(&json!({"actor":"root","role":"admin"})).is_err());
        assert!(reject_body_identity(&json!({"goal":"x"})).is_ok());
    }
}
