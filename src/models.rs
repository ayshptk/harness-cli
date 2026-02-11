use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::AgentKind;

/// A single model entry in the registry.
///
/// Contains metadata (`description`, `provider`) and per-agent model ID mappings.
/// Not every agent needs a mapping — only those that support the model.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ModelEntry {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub provider: String,
    /// Model ID for Claude Code (e.g. `claude-sonnet-4-5-20250929`).
    #[serde(default)]
    pub claude: Option<String>,
    /// Model ID for Codex CLI.
    #[serde(default)]
    pub codex: Option<String>,
    /// Model ID for OpenCode CLI.
    #[serde(default)]
    pub opencode: Option<String>,
    /// Model ID for Cursor CLI.
    #[serde(default)]
    pub cursor: Option<String>,
}

impl ModelEntry {
    /// Get the model ID string for the given agent, if this model supports that agent.
    pub fn agent_model(&self, kind: AgentKind) -> Option<&str> {
        match kind {
            AgentKind::Claude => self.claude.as_deref(),
            AgentKind::Codex => self.codex.as_deref(),
            AgentKind::OpenCode => self.opencode.as_deref(),
            AgentKind::Cursor => self.cursor.as_deref(),
        }
    }

    /// Return all agent kinds that have a mapping in this entry.
    pub fn supported_agents(&self) -> Vec<AgentKind> {
        let mut agents = Vec::new();
        if self.claude.is_some() {
            agents.push(AgentKind::Claude);
        }
        if self.codex.is_some() {
            agents.push(AgentKind::Codex);
        }
        if self.opencode.is_some() {
            agents.push(AgentKind::OpenCode);
        }
        if self.cursor.is_some() {
            agents.push(AgentKind::Cursor);
        }
        agents
    }
}

/// The model registry — a map from canonical names to model entries.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ModelRegistry {
    #[serde(default)]
    pub models: HashMap<String, ModelEntry>,
}

/// The outcome of resolving a model name against the registry.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelResolution {
    /// Found in registry and the agent has a mapping.
    Resolved {
        canonical_name: String,
        agent_id: String,
    },
    /// Found in registry but no mapping for this agent.
    NoAgentMapping { canonical_name: String },
    /// Not found in registry — pass through as-is.
    Passthrough { raw: String },
}

impl ModelResolution {
    /// Return the model ID string to pass to the agent CLI.
    pub fn model_id(&self) -> &str {
        match self {
            ModelResolution::Resolved { agent_id, .. } => agent_id,
            ModelResolution::NoAgentMapping { canonical_name } => canonical_name,
            ModelResolution::Passthrough { raw } => raw,
        }
    }
}

impl ModelRegistry {
    /// Parse the builtin models.toml compiled into the binary.
    pub fn builtin() -> Self {
        let content = include_str!("../models.toml");
        match toml::from_str(content) {
            Ok(reg) => reg,
            Err(e) => {
                tracing::warn!("builtin models.toml is malformed: {e}");
                Self::default()
            }
        }
    }

    /// Parse a TOML string into a registry.
    pub fn from_toml(content: &str) -> Result<Self, String> {
        toml::from_str(content).map_err(|e| e.to_string())
    }

    /// Merge another registry into this one. `overrides` wins on conflicts.
    pub fn merge(&self, overrides: &ModelRegistry) -> ModelRegistry {
        let mut merged = self.clone();
        for (name, entry) in &overrides.models {
            merged.models.insert(name.clone(), entry.clone());
        }
        merged
    }

    /// Look up a model name and resolve it for the given agent.
    pub fn resolve(&self, name: &str, agent: AgentKind) -> ModelResolution {
        if let Some(entry) = self.models.get(name) {
            if let Some(agent_id) = entry.agent_model(agent) {
                ModelResolution::Resolved {
                    canonical_name: name.to_string(),
                    agent_id: agent_id.to_string(),
                }
            } else {
                ModelResolution::NoAgentMapping {
                    canonical_name: name.to_string(),
                }
            }
        } else {
            ModelResolution::Passthrough {
                raw: name.to_string(),
            }
        }
    }

    /// Return all canonical model names, sorted.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.models.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Return models that have a mapping for the given agent.
    pub fn models_for_agent(&self, agent: AgentKind) -> Vec<(&str, &str)> {
        let mut result: Vec<(&str, &str)> = self
            .models
            .iter()
            .filter_map(|(name, entry)| {
                entry
                    .agent_model(agent)
                    .map(|id| (name.as_str(), id))
            })
            .collect();
        result.sort_by_key(|(name, _)| *name);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_parses() {
        let reg = ModelRegistry::builtin();
        assert!(!reg.models.is_empty());
        assert!(reg.models.contains_key("opus"));
    }

    #[test]
    fn builtin_opus_has_claude_mapping() {
        let reg = ModelRegistry::builtin();
        let entry = reg.models.get("opus").unwrap();
        assert_eq!(
            entry.agent_model(AgentKind::Claude),
            Some("claude-opus-4-6")
        );
        assert!(entry.agent_model(AgentKind::Codex).is_none());
    }

    #[test]
    fn builtin_opus_has_multi_agent_mapping() {
        let reg = ModelRegistry::builtin();
        let entry = reg.models.get("opus").unwrap();
        assert_eq!(entry.agent_model(AgentKind::Claude), Some("claude-opus-4-6"));
        assert_eq!(entry.agent_model(AgentKind::OpenCode), Some("anthropic/claude-opus-4-6"));
        assert_eq!(entry.agent_model(AgentKind::Cursor), Some("claude-opus-4-6"));
        assert!(entry.agent_model(AgentKind::Codex).is_none());
    }

    #[test]
    fn resolve_known_model_with_agent() {
        let reg = ModelRegistry::builtin();
        let res = reg.resolve("opus", AgentKind::Claude);
        assert_eq!(
            res,
            ModelResolution::Resolved {
                canonical_name: "opus".into(),
                agent_id: "claude-opus-4-6".into(),
            }
        );
        assert_eq!(res.model_id(), "claude-opus-4-6");
    }

    #[test]
    fn resolve_known_model_no_agent_mapping() {
        let reg = ModelRegistry::builtin();
        let res = reg.resolve("opus", AgentKind::Codex);
        assert_eq!(
            res,
            ModelResolution::NoAgentMapping {
                canonical_name: "opus".into(),
            }
        );
        assert_eq!(res.model_id(), "opus");
    }

    #[test]
    fn resolve_unknown_model_passthrough() {
        let reg = ModelRegistry::builtin();
        let res = reg.resolve("my-custom-model", AgentKind::Claude);
        assert_eq!(
            res,
            ModelResolution::Passthrough {
                raw: "my-custom-model".into(),
            }
        );
        assert_eq!(res.model_id(), "my-custom-model");
    }

    #[test]
    fn from_toml_valid() {
        let toml = r#"
[models.test-model]
description = "Test"
provider = "test"
claude = "test-id"
"#;
        let reg = ModelRegistry::from_toml(toml).unwrap();
        assert!(reg.models.contains_key("test-model"));
    }

    #[test]
    fn from_toml_empty() {
        let reg = ModelRegistry::from_toml("").unwrap();
        assert!(reg.models.is_empty());
    }

    #[test]
    fn from_toml_invalid() {
        let result = ModelRegistry::from_toml("not valid toml {{{{");
        assert!(result.is_err());
    }

    #[test]
    fn merge_disjoint() {
        let a = ModelRegistry::from_toml(
            r#"
[models.a]
description = "A"
provider = "test"
claude = "a-claude"
"#,
        )
        .unwrap();
        let b = ModelRegistry::from_toml(
            r#"
[models.b]
description = "B"
provider = "test"
codex = "b-codex"
"#,
        )
        .unwrap();
        let merged = a.merge(&b);
        assert!(merged.models.contains_key("a"));
        assert!(merged.models.contains_key("b"));
    }

    #[test]
    fn merge_override() {
        let base = ModelRegistry::from_toml(
            r#"
[models.sonnet]
description = "Original"
provider = "anthropic"
claude = "original-id"
"#,
        )
        .unwrap();
        let overrides = ModelRegistry::from_toml(
            r#"
[models.sonnet]
description = "Custom"
provider = "anthropic"
claude = "custom-id"
"#,
        )
        .unwrap();
        let merged = base.merge(&overrides);
        let entry = merged.models.get("sonnet").unwrap();
        assert_eq!(entry.claude.as_deref(), Some("custom-id"));
        assert_eq!(entry.description, "Custom");
    }

    #[test]
    fn names_sorted() {
        let reg = ModelRegistry::builtin();
        let names = reg.names();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn models_for_agent_filters() {
        let reg = ModelRegistry::builtin();
        let claude_models = reg.models_for_agent(AgentKind::Claude);
        // Claude should have opus
        assert!(claude_models.iter().any(|(name, _)| *name == "opus"));
        // Codex should have no models (opus is not mapped for codex)
        let codex_models = reg.models_for_agent(AgentKind::Codex);
        assert!(codex_models.is_empty());
    }

    #[test]
    fn supported_agents() {
        let entry = ModelEntry {
            description: "test".into(),
            provider: "test".into(),
            claude: Some("c".into()),
            codex: None,
            opencode: Some("o".into()),
            cursor: None,
        };
        let agents = entry.supported_agents();
        assert_eq!(agents, vec![AgentKind::Claude, AgentKind::OpenCode]);
    }

    #[test]
    fn model_entry_default() {
        let entry = ModelEntry::default();
        assert!(entry.description.is_empty());
        assert!(entry.claude.is_none());
        assert!(entry.codex.is_none());
        assert!(entry.opencode.is_none());
        assert!(entry.cursor.is_none());
    }
}
