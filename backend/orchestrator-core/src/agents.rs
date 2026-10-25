//! Agents: identity types, the input a user submits and the validated `AgentSpec`.
//! `AgentSpec::validated` is the only constructor, so an invalid agent never exists.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::{AgentId, TeamId};
use crate::llm::{Model, ReasoningEffort};
use crate::policy::{Decision, Effect, LeadAccess};
use crate::tools::ToolName;

/// A field-level validation failure; the API turns it into `{error: {code, message, field}}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {message}")]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl ValidationError {
    pub fn new(field: &str, message: &str) -> Self {
        Self {
            field: field.to_string(),
            message: message.to_string(),
        }
    }
}

/// An agent's fixed identifier, the name the model sees: `[a-z0-9_-]{1,32}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(try_from = "String")]
#[ts(export, type = "string")]
pub struct Slug(String);

impl Slug {
    pub fn new(value: &str) -> Result<Self, ValidationError> {
        let valid_char =
            |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        if value.is_empty() || value.len() > 32 || !value.chars().all(valid_char) {
            return Err(ValidationError::new(
                "slug",
                "use 1 to 32 of a-z, 0-9, _ and -",
            ));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Slug {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Slug::new(&value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Role {
    Lead,
    Specialist,
}

impl Role {
    pub fn max_steps(self) -> u32 {
        match self {
            Role::Lead => 12,
            Role::Specialist => 8,
        }
    }
}

/// What a user submits to create or replace an agent. A tool missing from `tools` is off.
#[derive(Debug, Clone, PartialEq, Deserialize, TS)]
#[ts(export, optional_fields)]
pub struct AgentInput {
    /// Create: the agent's team, the first live team when absent. Replace: absent or unchanged.
    pub team_id: Option<TeamId>,
    pub role: Role,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub model: Model,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub icon: String,
    pub colour: String,
    pub tools: BTreeMap<ToolName, Decision>,
    pub delegate_targets: Vec<String>,
}

/// A valid agent. Checks that span the team (one Lead, live targets) run in the write's transaction.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct AgentSpec {
    id: AgentId,
    team_id: TeamId,
    role: Role,
    slug: Slug,
    name: String,
    description: String,
    system_prompt: String,
    model: Model,
    reasoning_effort: Option<ReasoningEffort>,
    max_steps: u32,
    icon: String,
    colour: String,
    tools: BTreeMap<ToolName, Decision>,
    delegate_targets: Vec<Slug>,
    version: i64,
}

impl AgentSpec {
    /// `team_id` comes from storage, which resolves and checks `input.team_id`.
    pub fn validated(
        id: AgentId,
        team_id: TeamId,
        version: i64,
        input: AgentInput,
    ) -> Result<AgentSpec, ValidationError> {
        let slug = Slug::new(&input.slug)?;
        let delegate_targets = input
            .delegate_targets
            .iter()
            .map(|target| {
                Slug::new(target)
                    .map_err(|error| ValidationError::new("delegate_targets", &error.message))
            })
            .collect::<Result<Vec<_>, _>>()?;

        for (field, value) in [
            ("name", &input.name),
            ("description", &input.description),
            ("system_prompt", &input.system_prompt),
            ("icon", &input.icon),
            ("colour", &input.colour),
        ] {
            if value.trim().is_empty() {
                return Err(ValidationError::new(field, "must not be empty"));
            }
        }

        if let Some(effort) = input.reasoning_effort
            && !input.model.efforts().contains(&effort)
        {
            let message = if input.model.efforts().is_empty() {
                "this model takes no reasoning effort"
            } else {
                "this model does not take this reasoning effort"
            };
            return Err(ValidationError::new("reasoning_effort", message));
        }

        for (&tool, &decision) in &input.tools {
            let allowed_for_role = match (tool.lead(), input.role) {
                (LeadAccess::Only, Role::Lead)
                | (LeadAccess::Forbidden, Role::Specialist)
                | (LeadAccess::TrustedOnly, _) => true,
                (LeadAccess::Only, Role::Specialist) | (LeadAccess::Forbidden, Role::Lead) => false,
            };
            if !allowed_for_role {
                return Err(ValidationError::new(
                    "tools",
                    &format!("{tool:?} is not available to this role"),
                ));
            }
            if tool.effect() == Effect::External && decision == Decision::Allow {
                return Err(ValidationError::new(
                    "tools",
                    "external tools can be ask or deny, never allow",
                ));
            }
        }

        match input.role {
            Role::Specialist if !delegate_targets.is_empty() => {
                return Err(ValidationError::new(
                    "delegate_targets",
                    "only the Lead delegates",
                ));
            }
            Role::Specialist => {}
            Role::Lead => {
                let mut seen = BTreeSet::new();
                for target in &delegate_targets {
                    if target == &slug || !seen.insert(target.as_str()) {
                        return Err(ValidationError::new(
                            "delegate_targets",
                            "targets must be distinct and not the Lead itself",
                        ));
                    }
                }
            }
        }

        Ok(AgentSpec {
            id,
            team_id,
            role: input.role,
            slug,
            name: input.name,
            description: input.description,
            system_prompt: input.system_prompt,
            model: input.model,
            reasoning_effort: input.reasoning_effort,
            max_steps: input.role.max_steps(),
            icon: input.icon,
            colour: input.colour,
            tools: input.tools,
            delegate_targets,
            version,
        })
    }

    pub fn id(&self) -> AgentId {
        self.id
    }

    pub fn team_id(&self) -> TeamId {
        self.team_id
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn slug(&self) -> &Slug {
        &self.slug
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub fn model(&self) -> Model {
        self.model
    }

    pub fn reasoning_effort(&self) -> Option<ReasoningEffort> {
        self.reasoning_effort
    }

    pub fn max_steps(&self) -> u32 {
        self.max_steps
    }

    pub fn icon(&self) -> &str {
        &self.icon
    }

    pub fn colour(&self) -> &str {
        &self.colour
    }

    pub fn tools(&self) -> &BTreeMap<ToolName, Decision> {
        &self.tools
    }

    pub fn delegate_targets(&self) -> &[Slug] {
        &self.delegate_targets
    }

    pub fn version(&self) -> i64 {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(role: Role, slug: &str) -> AgentInput {
        AgentInput {
            team_id: None,
            role,
            slug: slug.to_string(),
            name: "N".to_string(),
            description: "D".to_string(),
            system_prompt: "P".to_string(),
            model: Model::Grok43,
            reasoning_effort: None,
            icon: "i".to_string(),
            colour: "c".to_string(),
            tools: BTreeMap::new(),
            delegate_targets: vec![],
        }
    }

    fn check(input: AgentInput) -> Result<AgentSpec, ValidationError> {
        AgentSpec::validated(AgentId::new(), TeamId::new(), 1, input)
    }

    #[test]
    fn agent_validation_rejects_each_broken_rule() {
        let mut lead_with_file = input(Role::Lead, "lead");
        lead_with_file
            .tools
            .insert(ToolName::FileRead, Decision::Allow);

        let mut specialist_with_plan = input(Role::Specialist, "s");
        specialist_with_plan
            .tools
            .insert(ToolName::Plan, Decision::Allow);

        let mut send_allowed = input(Role::Specialist, "s");
        send_allowed
            .tools
            .insert(ToolName::SendMessage, Decision::Allow);

        let mut specialist_targets = input(Role::Specialist, "s");
        specialist_targets.delegate_targets = vec!["x".to_string()];

        let mut lead_targets_itself = input(Role::Lead, "lead");
        lead_targets_itself.delegate_targets = vec!["lead".to_string()];

        let mut effort_on_non_reasoning = input(Role::Specialist, "s");
        effort_on_non_reasoning.model = Model::Grok420NonReasoning;
        effort_on_non_reasoning.reasoning_effort = Some(ReasoningEffort::Low);

        let mut empty_prompt = input(Role::Specialist, "s");
        empty_prompt.system_prompt = "  ".to_string();

        let bad_slug = input(Role::Specialist, "Bad Slug");

        let mut bad_target = input(Role::Lead, "lead");
        bad_target.delegate_targets = vec!["Not Valid".to_string()];

        for (case, field) in [
            (lead_with_file, "tools"),
            (specialist_with_plan, "tools"),
            (send_allowed, "tools"),
            (specialist_targets, "delegate_targets"),
            (lead_targets_itself, "delegate_targets"),
            (effort_on_non_reasoning, "reasoning_effort"),
            (empty_prompt, "system_prompt"),
            (bad_slug, "slug"),
            (bad_target, "delegate_targets"),
        ] {
            assert_eq!(
                check(case).map(|_| ()).map_err(|e| e.field),
                Err(field.to_string())
            );
        }
    }

    #[test]
    fn agent_validation_accepts_the_seed_shapes() {
        let mut lead = input(Role::Lead, "lead");
        lead.tools.insert(ToolName::Delegate, Decision::Allow);
        lead.tools.insert(ToolName::Plan, Decision::Allow);
        lead.delegate_targets = vec!["reader".to_string()];
        assert_eq!(check(lead.clone()).map(|spec| spec.max_steps()), Ok(12));
        for memory in [ToolName::MemoryRead, ToolName::MemorySearch] {
            let mut with_memory = lead.clone();
            with_memory.tools.insert(memory, Decision::Allow);
            assert!(check(with_memory).is_ok(), "the Lead may hold {memory:?}");
        }
        let mut with_write = lead.clone();
        with_write
            .tools
            .insert(ToolName::MemoryWrite, Decision::Allow);
        assert_eq!(
            check(with_write).map(|_| ()).map_err(|e| e.field),
            Err("tools".to_string()),
            "the Lead never holds memory_write"
        );

        let mut reader = input(Role::Specialist, "reader");
        reader.tools.insert(ToolName::InboxRead, Decision::Allow);
        reader.tools.insert(ToolName::SendMessage, Decision::Deny);
        assert_eq!(check(reader).map(|spec| spec.max_steps()), Ok(8));
    }

    #[test]
    fn slug_accepts_valid_and_rejects_invalid() {
        assert!(Slug::new("reader_2-a").is_ok());
        for bad in ["", "Reader", "has space", "dot.ted", &"a".repeat(33)] {
            assert!(Slug::new(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn slug_deserialize_validates() {
        assert!(serde_json::from_str::<Slug>("\"ok\"").is_ok());
        assert!(serde_json::from_str::<Slug>("\"NOT OK\"").is_err());
    }
}
