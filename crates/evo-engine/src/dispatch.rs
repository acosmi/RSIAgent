//! Role-gated dispatch. Model tools and admin ops stay on separate lists.
use evo_core::contract::{MODEL_TOOLS, admin_ops, reject_admin_as_model_tool};
use evo_core::{Context, Error, Result, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    ModelTool,
    Admin,
}

pub fn classify(name: &str) -> Result<Surface> {
    if MODEL_TOOLS.contains(&name) {
        return Ok(Surface::ModelTool);
    }
    if admin_ops().contains(&name) {
        return Ok(Surface::Admin);
    }
    Err(Error::Invalid("unknown operation".into()))
}

pub fn authorize(ctx: &Context, name: &str) -> Result<Surface> {
    match classify(name)? {
        Surface::ModelTool => {
            reject_admin_as_model_tool(name)?;
            ctx.require(&[Role::Agent, Role::Host])?;
            Ok(Surface::ModelTool)
        }
        Surface::Admin => {
            ctx.require(&[Role::Admin, Role::Evaluator])?;
            Ok(Surface::Admin)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_cannot_register_experiments() {
        let ctx = Context::new("n", "a", Role::Agent).unwrap();
        assert!(authorize(&ctx, "experiment.register").is_err());
        assert!(authorize(&ctx, "evo_prepare").is_ok());
    }

    #[test]
    fn evaluator_cannot_be_listed_as_model_tool() {
        assert!(reject_admin_as_model_tool("evaluation.start").is_err());
        let ctx = Context::new("n", "e", Role::Evaluator).unwrap();
        assert!(authorize(&ctx, "evaluation.start").is_ok());
    }
}
