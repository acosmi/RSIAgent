//! generate → execute_dev → observe → decide. Intermediate nodes are not published.
use evo_core::strategy::{ExplorationPolicy, PrefixView};
use evo_core::{Error, Result, Validate};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Generated,
    ExecutedDev,
    Observed,
    Selected,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchNode {
    pub id: String,
    pub prefix: PrefixView,
    pub state: NodeState,
    pub published: bool,
}

pub struct Coordinator {
    pub policy: ExplorationPolicy,
    pub nodes: Vec<SearchNode>,
}

impl Coordinator {
    pub fn new(policy: ExplorationPolicy) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            policy,
            nodes: Vec::new(),
        })
    }

    pub fn generate(&mut self, id: String, prefix: PrefixView) -> Result<()> {
        prefix.validate()?;
        if self.nodes.len() as u8 >= self.policy.max_nodes {
            return Err(Error::Budget);
        }
        if prefix.depth > self.policy.max_depth {
            return Err(Error::Invalid("depth cap".into()));
        }
        self.nodes.push(SearchNode {
            id,
            prefix,
            state: NodeState::Generated,
            published: false,
        });
        Ok(())
    }

    pub fn execute_dev(&mut self, id: &str) -> Result<()> {
        self.advance(id, NodeState::Generated, NodeState::ExecutedDev)
    }

    pub fn observe(&mut self, id: &str) -> Result<()> {
        self.advance(id, NodeState::ExecutedDev, NodeState::Observed)
    }

    pub fn decide(&mut self, id: &str) -> Result<()> {
        self.advance(id, NodeState::Observed, NodeState::Selected)
    }

    pub fn publish(&self, id: &str) -> Result<()> {
        let node = self
            .nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or(Error::NotFound)?;
        if node.state != NodeState::Selected {
            return Err(Error::Conflict(
                "search intermediate nodes are not published".into(),
            ));
        }
        Err(Error::Conflict(
            "final candidate must be re-resolved against approved_parent before publish".into(),
        ))
    }

    fn advance(&mut self, id: &str, from: NodeState, to: NodeState) -> Result<()> {
        let node = self
            .nodes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or(Error::NotFound)?;
        if node.state != from {
            return Err(Error::Conflict("illegal node transition".into()));
        }
        node.state = to;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::strategy::ExplorationPolicy;

    fn prefix() -> PrefixView {
        PrefixView {
            search_parent: "s1".into(),
            approved_parent: "a1".into(),
            depth: 1,
        }
    }

    #[test]
    fn w_greater_than_one_rejected() {
        let p = ExplorationPolicy {
            w: 2,
            ..Default::default()
        };
        assert!(Coordinator::new(p).is_err());
    }

    #[test]
    fn must_eval_before_decide_and_cannot_publish_mid_search() {
        let mut c = Coordinator::new(ExplorationPolicy::default()).unwrap();
        c.generate("n1".into(), prefix()).unwrap();
        assert!(c.decide("n1").is_err());
        c.execute_dev("n1").unwrap();
        c.observe("n1").unwrap();
        c.decide("n1").unwrap();
        assert!(c.publish("n1").is_err());
    }

    #[test]
    fn search_parent_is_not_approved_parent() {
        let p = PrefixView {
            search_parent: "same".into(),
            approved_parent: "same".into(),
            depth: 1,
        };
        let mut c = Coordinator::new(ExplorationPolicy::default()).unwrap();
        assert!(c.generate("n1".into(), p).is_err());
    }
}
