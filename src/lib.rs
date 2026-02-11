//! Unified coding agent harness — run Claude Code, OpenCode, Codex, or Cursor
//! through a single Rust API.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use harness::{AgentKind, TaskConfig};
//!
//! # #[tokio::main]
//! # async fn main() -> harness::Result<()> {
//! let config = TaskConfig::new("fix the bug", AgentKind::Claude);
//! let mut stream = harness::run_task(&config).await?;
//! # Ok(())
//! # }
//! ```

pub mod agents;
pub mod config;
pub mod error;
pub mod event;
pub mod logger;
pub mod models;
pub mod normalize;
pub mod process;
pub mod registry;
pub mod runner;
pub mod settings;

pub use config::{AgentKind, OutputFormat, PermissionMode, TaskConfig, TaskConfigBuilder};
pub use error::{Error, Result};
pub use event::{Event, UsageData};
pub use models::{ModelEntry, ModelRegistry, ModelResolution};
pub use normalize::NormalizeConfig;
pub use process::StreamHandle;
pub use runner::{AgentCapabilities, AgentRunner, EventStream};

/// Re-export the cancel token type for convenience.
pub use tokio_util::sync::CancellationToken;

/// Create a runner for the given agent kind and execute the task, returning
/// a stream of unified events.
///
/// This is the simple API — for cancellation support, use `run_task_with_cancel`.
pub async fn run_task(config: &TaskConfig) -> Result<EventStream> {
    let handle = run_task_with_cancel(config, None).await?;
    Ok(handle.stream)
}

/// Run a task with an optional cancellation token.
///
/// Returns a `StreamHandle` containing the event stream and the cancel token.
/// If no token is provided, a new one is created internally.
pub async fn run_task_with_cancel(
    config: &TaskConfig,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
) -> Result<StreamHandle> {
    let runner = agents::create_runner(config.agent);

    // If the user provided a custom binary path, skip the availability check
    // (the path will be validated at spawn time). Otherwise, check PATH.
    if config.binary_path.is_none() && !runner.is_available() {
        return Err(Error::BinaryNotFound {
            agent: config.agent.display_name().to_string(),
            binary: config.agent.default_binary().to_string(),
        });
    }

    let mut handle = runner.run(config, cancel_token).await?;

    let norm_config = NormalizeConfig {
        cwd: config
            .cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .or_else(|| std::env::current_dir().ok().map(|p| p.display().to_string())),
        model: config.model.clone(),
        prompt: Some(config.prompt.clone()),
    };
    handle.stream = normalize::normalize_stream(handle.stream, norm_config);

    Ok(handle)
}

/// List which agents are currently available on this system.
pub fn available_agents() -> Vec<AgentKind> {
    use AgentKind::*;
    [Claude, OpenCode, Codex, Cursor]
        .into_iter()
        .filter(|kind| {
            let runner = agents::create_runner(*kind);
            runner.is_available()
        })
        .collect()
}
