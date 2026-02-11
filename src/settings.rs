use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::AgentKind;
use crate::models::{ModelEntry, ModelRegistry};

/// User settings loaded from `~/.config/harness/config.toml` and optionally
/// merged with a project-level `.harnessrc.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    /// Default agent to use if `--agent` is omitted.
    #[serde(default)]
    pub default_agent: Option<String>,

    /// Default model to use if `--model` is omitted.
    #[serde(default)]
    pub default_model: Option<String>,

    /// Default permission mode (`full-access` or `read-only`).
    #[serde(default)]
    pub default_permissions: Option<String>,

    /// Default timeout in seconds.
    #[serde(default)]
    pub default_timeout_secs: Option<u64>,

    /// Log level for tracing output (e.g. "debug", "info", "warn").
    #[serde(default)]
    pub log_level: Option<String>,

    /// Per-agent configuration overrides.
    #[serde(default)]
    pub agents: HashMap<String, AgentSettings>,
}

/// Per-agent settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentSettings {
    /// Override binary path for this agent.
    #[serde(default)]
    pub binary: Option<String>,

    /// Default model for this agent.
    #[serde(default)]
    pub model: Option<String>,

    /// Extra args always prepended for this agent.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl Settings {
    /// Load settings from the default config file, optionally merged with a
    /// project-level `.harnessrc.toml` found by walking up from `cwd`.
    pub fn load() -> Self {
        Self::load_from(Self::config_path())
    }

    /// Load global settings, then merge project-level overrides from `cwd`.
    pub fn load_with_project(cwd: Option<&Path>) -> Self {
        let global = Self::load();
        if let Some(dir) = cwd {
            if let Some(project) = Self::load_project(dir) {
                return global.merge(&project);
            }
        }
        global
    }

    /// Load settings from a specific path.
    pub fn load_from(path: Option<PathBuf>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };

        if !path.exists() {
            return Self::default();
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("failed to read config file {}: {e}", path.display());
                return Self::default();
            }
        };

        match toml::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("failed to parse config file {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Walk up from `start` looking for `.harnessrc.toml`. Returns the parsed
    /// settings if found, `None` otherwise.
    pub fn load_project(start: &Path) -> Option<Self> {
        let mut dir = start.to_path_buf();
        loop {
            let candidate = dir.join(".harnessrc.toml");
            if candidate.exists() {
                return Some(Self::load_from(Some(candidate)));
            }
            if !dir.pop() {
                break;
            }
        }
        None
    }

    /// Merge another settings into this one. `other` (project) wins for scalar
    /// fields; `extra_args` in agent settings are concatenated.
    pub fn merge(&self, other: &Settings) -> Settings {
        let mut merged = self.clone();

        if other.default_agent.is_some() {
            merged.default_agent.clone_from(&other.default_agent);
        }
        if other.default_model.is_some() {
            merged.default_model.clone_from(&other.default_model);
        }
        if other.default_permissions.is_some() {
            merged
                .default_permissions
                .clone_from(&other.default_permissions);
        }
        if other.default_timeout_secs.is_some() {
            merged.default_timeout_secs = other.default_timeout_secs;
        }
        if other.log_level.is_some() {
            merged.log_level.clone_from(&other.log_level);
        }

        // Merge per-agent settings.
        for (key, other_agent) in &other.agents {
            let entry = merged
                .agents
                .entry(key.clone())
                .or_default();
            if other_agent.binary.is_some() {
                entry.binary.clone_from(&other_agent.binary);
            }
            if other_agent.model.is_some() {
                entry.model.clone_from(&other_agent.model);
            }
            // Concatenate extra_args (global first, then project).
            if !other_agent.extra_args.is_empty() {
                entry.extra_args.extend(other_agent.extra_args.clone());
            }
        }

        merged
    }

    /// Default config file path: `~/.config/harness/config.toml`.
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("harness").join("config.toml"))
    }

    /// Generate a template config file as a TOML string.
    pub fn template() -> &'static str {
        r#"# harness configuration — ~/.config/harness/config.toml

# Default agent when --agent is omitted.
# default_agent = "claude"

# Default model when --model is omitted.
# default_model = "claude-opus-4-6"

# Default permission mode: "full-access" or "read-only".
# default_permissions = "full-access"

# Default timeout in seconds.
# default_timeout_secs = 300

# Log level: "error", "warn", "info", "debug", "trace".
# log_level = "warn"

# Per-agent settings.
# [agents.claude]
# binary = "/opt/claude/bin/claude"
# model = "claude-opus-4-6"
# extra_args = ["--verbose"]

# [agents.codex]
# model = "gpt-5-codex"
# extra_args = []
"#
    }

    /// Resolve the default agent from settings.
    pub fn resolve_default_agent(&self) -> Option<AgentKind> {
        self.default_agent.as_ref()?.parse().ok()
    }

    /// Get agent-specific settings.
    pub fn agent_settings(&self, kind: AgentKind) -> Option<&AgentSettings> {
        let key = match kind {
            AgentKind::Claude => "claude",
            AgentKind::OpenCode => "opencode",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor",
        };
        self.agents.get(key)
    }

    /// Resolve the binary path for a given agent from settings.
    pub fn agent_binary(&self, kind: AgentKind) -> Option<PathBuf> {
        self.agent_settings(kind)
            .and_then(|s| s.binary.as_ref())
            .map(PathBuf::from)
    }

    /// Resolve the model for a given agent from settings.
    pub fn agent_model(&self, kind: AgentKind) -> Option<String> {
        // Agent-specific model takes precedence over global default.
        self.agent_settings(kind)
            .and_then(|s| s.model.clone())
            .or_else(|| self.default_model.clone())
    }

    /// Get agent-specific extra_args from settings.
    pub fn agent_extra_args(&self, kind: AgentKind) -> Vec<String> {
        self.agent_settings(kind)
            .map(|s| s.extra_args.clone())
            .unwrap_or_default()
    }
}

/// Project-level configuration loaded from `harness.toml` in the project directory.
///
/// This is the new config format — replaces `~/.config/harness/config.toml` (global)
/// and `.harnessrc.toml` (walk-up). Contains the same fields as `Settings` plus a
/// `[models]` section for project-level model overrides.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub default_agent: Option<String>,

    #[serde(default)]
    pub default_model: Option<String>,

    #[serde(default)]
    pub default_permissions: Option<String>,

    #[serde(default)]
    pub default_timeout_secs: Option<u64>,

    #[serde(default)]
    pub log_level: Option<String>,

    /// Per-agent configuration overrides.
    #[serde(default)]
    pub agents: HashMap<String, AgentSettings>,

    /// Project-level model overrides / additions.
    #[serde(default)]
    pub models: HashMap<String, ModelEntry>,
}

impl ProjectConfig {
    /// Load `harness.toml` by walking up from `dir` to find the nearest one.
    pub fn load(dir: &Path) -> Option<Self> {
        let (config, _path) = Self::load_with_path(dir)?;
        Some(config)
    }

    /// Load `harness.toml` by walking up from `dir`, returning both the config
    /// and the path where it was found.
    pub fn load_with_path(dir: &Path) -> Option<(Self, PathBuf)> {
        let mut current = dir.to_path_buf();
        loop {
            let path = current.join("harness.toml");
            if path.exists() {
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("failed to read {}: {e}", path.display());
                        return None;
                    }
                };
                return match toml::from_str(&content) {
                    Ok(c) => Some((c, path)),
                    Err(e) => {
                        tracing::warn!("failed to parse {}: {e}", path.display());
                        None
                    }
                };
            }
            if !current.pop() {
                break;
            }
        }
        None
    }

    /// Extract the `[models]` section as a `ModelRegistry`.
    pub fn model_registry(&self) -> ModelRegistry {
        ModelRegistry {
            models: self.models.clone(),
        }
    }

    /// Resolve the default agent from this config.
    pub fn resolve_default_agent(&self) -> Option<AgentKind> {
        self.default_agent.as_ref()?.parse().ok()
    }

    /// Get agent-specific settings.
    pub fn agent_settings(&self, kind: AgentKind) -> Option<&AgentSettings> {
        let key = match kind {
            AgentKind::Claude => "claude",
            AgentKind::OpenCode => "opencode",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor",
        };
        self.agents.get(key)
    }

    /// Resolve the binary path for a given agent.
    pub fn agent_binary(&self, kind: AgentKind) -> Option<PathBuf> {
        self.agent_settings(kind)
            .and_then(|s| s.binary.as_ref())
            .map(PathBuf::from)
    }

    /// Resolve the model for a given agent.
    pub fn agent_model(&self, kind: AgentKind) -> Option<String> {
        self.agent_settings(kind)
            .and_then(|s| s.model.clone())
            .or_else(|| self.default_model.clone())
    }

    /// Get agent-specific extra_args.
    pub fn agent_extra_args(&self, kind: AgentKind) -> Vec<String> {
        self.agent_settings(kind)
            .map(|s| s.extra_args.clone())
            .unwrap_or_default()
    }

    /// Generate a template `harness.toml` file.
    pub fn template() -> &'static str {
        r#"# harness project configuration — harness.toml
#
# Place this file in your project root.

# Default agent when --agent is omitted.
# default_agent = "claude"

# Default model when --model is omitted (uses model registry for translation).
# default_model = "sonnet"

# Default permission mode: "full-access" or "read-only".
# default_permissions = "full-access"

# Default timeout in seconds.
# default_timeout_secs = 300

# Log level: "error", "warn", "info", "debug", "trace".
# log_level = "warn"

# Per-agent settings.
# [agents.claude]
# binary = "/opt/claude/bin/claude"
# model = "sonnet"
# extra_args = ["--verbose"]

# Model registry overrides.
# These override or extend the canonical registry for this project.
# [models.my-model]
# description = "My custom model"
# provider = "anthropic"
# claude = "my-custom-model-id"
"#
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_config() {
        let settings: Settings = toml::from_str("").unwrap();
        assert!(settings.default_agent.is_none());
        assert!(settings.agents.is_empty());
    }

    #[test]
    fn parse_full_config() {
        let toml = r#"
default_agent = "claude"
default_model = "claude-opus-4-6"

[agents.claude]
binary = "/opt/claude/bin/claude"

[agents.codex]
model = "gpt-5-codex"
"#;
        let settings: Settings = toml::from_str(toml).unwrap();
        assert_eq!(settings.default_agent, Some("claude".to_string()));
        assert_eq!(settings.default_model, Some("claude-opus-4-6".to_string()));
        assert_eq!(
            settings.agents["claude"].binary,
            Some("/opt/claude/bin/claude".to_string())
        );
        assert_eq!(
            settings.agents["codex"].model,
            Some("gpt-5-codex".to_string())
        );
    }

    #[test]
    fn parse_expanded_config() {
        let toml = r#"
default_agent = "claude"
default_model = "opus"
default_permissions = "read-only"
default_timeout_secs = 300
log_level = "debug"

[agents.claude]
binary = "/usr/bin/claude"
model = "sonnet"
extra_args = ["--verbose", "--no-color"]
"#;
        let settings: Settings = toml::from_str(toml).unwrap();
        assert_eq!(settings.default_permissions, Some("read-only".into()));
        assert_eq!(settings.default_timeout_secs, Some(300));
        assert_eq!(settings.log_level, Some("debug".into()));
        let claude = settings.agent_settings(AgentKind::Claude).unwrap();
        assert_eq!(claude.extra_args, vec!["--verbose", "--no-color"]);
    }

    #[test]
    fn resolve_default_agent() {
        let settings = Settings {
            default_agent: Some("claude".to_string()),
            ..Default::default()
        };
        assert_eq!(settings.resolve_default_agent(), Some(AgentKind::Claude));
    }

    #[test]
    fn agent_model_prefers_specific() {
        let mut agents = HashMap::new();
        agents.insert(
            "claude".to_string(),
            AgentSettings {
                model: Some("sonnet".to_string()),
                ..Default::default()
            },
        );
        let settings = Settings {
            default_model: Some("opus".to_string()),
            agents,
            ..Default::default()
        };
        assert_eq!(
            settings.agent_model(AgentKind::Claude),
            Some("sonnet".to_string())
        );
        assert_eq!(
            settings.agent_model(AgentKind::Codex),
            Some("opus".to_string())
        );
    }

    #[test]
    fn load_nonexistent_returns_default() {
        let settings = Settings::load_from(Some(PathBuf::from("/nonexistent/path/config.toml")));
        assert!(settings.default_agent.is_none());
    }

    #[test]
    fn merge_project_overrides() {
        let global = Settings {
            default_agent: Some("claude".into()),
            default_model: Some("opus".into()),
            default_timeout_secs: Some(300),
            ..Default::default()
        };
        let project = Settings {
            default_model: Some("sonnet".into()),
            default_permissions: Some("read-only".into()),
            ..Default::default()
        };
        let merged = global.merge(&project);
        assert_eq!(merged.default_agent, Some("claude".into())); // kept from global
        assert_eq!(merged.default_model, Some("sonnet".into())); // overridden by project
        assert_eq!(merged.default_timeout_secs, Some(300)); // kept from global
        assert_eq!(merged.default_permissions, Some("read-only".into())); // from project
    }

    #[test]
    fn merge_agent_extra_args_concatenate() {
        let mut global_agents = HashMap::new();
        global_agents.insert(
            "claude".to_string(),
            AgentSettings {
                extra_args: vec!["--verbose".into()],
                ..Default::default()
            },
        );
        let global = Settings {
            agents: global_agents,
            ..Default::default()
        };

        let mut project_agents = HashMap::new();
        project_agents.insert(
            "claude".to_string(),
            AgentSettings {
                extra_args: vec!["--no-color".into()],
                model: Some("sonnet".into()),
                ..Default::default()
            },
        );
        let project = Settings {
            agents: project_agents,
            ..Default::default()
        };

        let merged = global.merge(&project);
        let claude = merged.agent_settings(AgentKind::Claude).unwrap();
        assert_eq!(claude.extra_args, vec!["--verbose", "--no-color"]);
        assert_eq!(claude.model, Some("sonnet".into()));
    }

    #[test]
    fn load_project_walks_up() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).unwrap();

        // Place .harnessrc.toml at `a/` level.
        let rc_path = tmp.path().join("a").join(".harnessrc.toml");
        std::fs::write(&rc_path, "default_agent = \"codex\"\n").unwrap();

        // Starting from `a/b/c`, should find `a/.harnessrc.toml`.
        let found = Settings::load_project(&deep);
        assert!(found.is_some());
        assert_eq!(found.unwrap().default_agent, Some("codex".into()));
    }

    #[test]
    fn agent_extra_args_from_settings() {
        let mut agents = HashMap::new();
        agents.insert(
            "claude".to_string(),
            AgentSettings {
                extra_args: vec!["--verbose".into()],
                ..Default::default()
            },
        );
        let settings = Settings {
            agents,
            ..Default::default()
        };
        assert_eq!(
            settings.agent_extra_args(AgentKind::Claude),
            vec!["--verbose"]
        );
        assert!(settings.agent_extra_args(AgentKind::Codex).is_empty());
    }

    #[test]
    fn template_parses_as_valid_toml() {
        // The template should parse (all lines are commented out).
        let result: std::result::Result<Settings, _> = toml::from_str(Settings::template());
        assert!(result.is_ok());
    }

    // ─── ProjectConfig tests ─────────────────────────────────────

    #[test]
    fn project_config_parse_empty() {
        let config: ProjectConfig = toml::from_str("").unwrap();
        assert!(config.default_agent.is_none());
        assert!(config.models.is_empty());
    }

    #[test]
    fn project_config_parse_with_models() {
        let toml = r#"
default_agent = "claude"
default_model = "sonnet"

[agents.claude]
binary = "/usr/bin/claude"

[models.my-model]
description = "Custom"
provider = "custom"
claude = "custom-id"
"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.default_agent, Some("claude".into()));
        assert_eq!(config.default_model, Some("sonnet".into()));
        assert!(config.models.contains_key("my-model"));
        assert_eq!(
            config.models["my-model"].claude,
            Some("custom-id".into())
        );
    }

    #[test]
    fn project_config_model_registry() {
        let toml = r#"
[models.test]
description = "Test"
provider = "test"
claude = "test-id"
"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let reg = config.model_registry();
        assert!(reg.models.contains_key("test"));
    }

    #[test]
    fn project_config_load_from_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("harness.toml"),
            "default_agent = \"claude\"\n",
        )
        .unwrap();
        let config = ProjectConfig::load(tmp.path());
        assert!(config.is_some());
        assert_eq!(config.unwrap().default_agent, Some("claude".into()));
    }

    #[test]
    fn project_config_load_walks_up() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).unwrap();

        // Place harness.toml at `a/` level.
        std::fs::write(
            tmp.path().join("a").join("harness.toml"),
            "default_agent = \"codex\"\n",
        )
        .unwrap();

        // Starting from `a/b/c`, should find `a/harness.toml`.
        let config = ProjectConfig::load(&deep);
        assert!(config.is_some());
        assert_eq!(config.unwrap().default_agent, Some("codex".into()));
    }

    #[test]
    fn project_config_load_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(ProjectConfig::load(tmp.path()).is_none());
    }

    #[test]
    fn project_config_template_parses() {
        let result: std::result::Result<ProjectConfig, _> =
            toml::from_str(ProjectConfig::template());
        assert!(result.is_ok());
    }
}
