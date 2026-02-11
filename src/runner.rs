use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::config::{AgentKind, TaskConfig};
use crate::error::{Error, Result};
use crate::event::Event;
use crate::process::StreamHandle;

/// A boxed, pinned event stream returned by agent runners.
pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event>> + Send>>;

/// Check if any of the binary candidates for the given agent kind exist in PATH.
pub fn find_binary(kind: AgentKind) -> Option<PathBuf> {
    kind.binary_candidates()
        .iter()
        .find_map(|name| which::which(name).ok())
}

/// Check if any binary candidate is available on the system.
pub fn is_any_binary_available(kind: AgentKind) -> bool {
    find_binary(kind).is_some()
}

/// Resolve binary path: user override > PATH candidates > error.
pub fn resolve_binary(kind: AgentKind, config: &TaskConfig) -> Result<PathBuf> {
    if let Some(ref p) = config.binary_path {
        return Ok(p.clone());
    }
    find_binary(kind).ok_or_else(|| Error::BinaryNotFound {
        agent: kind.display_name().to_string(),
        binary: kind.binary_candidates().join(" or "),
    })
}

/// Describes what features an agent supports.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentCapabilities {
    pub supports_system_prompt: bool,
    pub supports_budget: bool,
    pub supports_model: bool,
    pub supports_max_turns: bool,
    pub supports_append_system_prompt: bool,
}

/// A config validation warning.
#[derive(Debug, Clone)]
pub struct ConfigWarning {
    pub message: String,
}

impl std::fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Trait implemented by each agent adapter.
///
/// The runner translates a unified `TaskConfig` into the agent-specific CLI
/// invocation and converts the agent's streaming output into a unified
/// `EventStream`.
#[async_trait]
pub trait AgentRunner: Send + Sync {
    /// Human-readable name of this agent backend.
    fn name(&self) -> &str;

    /// Check whether the agent binary is available on the system.
    fn is_available(&self) -> bool;

    /// Resolve the binary path (user override or PATH lookup).
    fn binary_path(&self, config: &TaskConfig) -> Result<std::path::PathBuf>;

    /// Build the command-line arguments for the agent process.
    fn build_args(&self, config: &TaskConfig) -> Vec<String>;

    /// Build the environment variables for the agent process.
    fn build_env(&self, config: &TaskConfig) -> Vec<(String, String)>;

    /// Run the task and return a `StreamHandle` with event stream and cancel token.
    async fn run(
        &self,
        config: &TaskConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Result<StreamHandle>;

    /// Get the version of the installed agent binary.
    fn version(&self, config: &TaskConfig) -> Option<String> {
        let binary = self.binary_path(config).ok()?;
        let output = std::process::Command::new(&binary)
            .arg("--version")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .ok()?;

        if output.status.success() {
            let version_str = String::from_utf8_lossy(&output.stdout);
            Some(version_str.trim().to_string())
        } else {
            None
        }
    }

    /// What features this agent supports.
    fn capabilities(&self) -> AgentCapabilities {
        // Default: conservative — subclasses override.
        AgentCapabilities::default()
    }

    /// Validate config against this agent's capabilities.
    fn validate_config(&self, config: &TaskConfig) -> Vec<ConfigWarning> {
        let caps = self.capabilities();
        let mut warnings = Vec::new();

        if config.system_prompt.is_some() && !caps.supports_system_prompt {
            warnings.push(ConfigWarning {
                message: format!("{} does not support --system-prompt", self.name()),
            });
        }
        if config.max_budget_usd.is_some() && !caps.supports_budget {
            warnings.push(ConfigWarning {
                message: format!("{} does not support --max-budget", self.name()),
            });
        }
        if config.model.is_some() && !caps.supports_model {
            warnings.push(ConfigWarning {
                message: format!("{} does not support --model", self.name()),
            });
        }
        if config.max_turns.is_some() && !caps.supports_max_turns {
            warnings.push(ConfigWarning {
                message: format!("{} does not support --max-turns", self.name()),
            });
        }
        if config.append_system_prompt.is_some() && !caps.supports_append_system_prompt {
            warnings.push(ConfigWarning {
                message: format!("{} does not support --append-system-prompt", self.name()),
            });
        }

        warnings
    }
}
