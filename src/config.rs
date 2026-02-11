use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Which coding agent backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Claude,
    OpenCode,
    Codex,
    Cursor,
}

impl AgentKind {
    /// Default binary name for this agent (first in the candidates list).
    pub fn default_binary(&self) -> &'static str {
        self.binary_candidates()[0]
    }

    /// All known binary names for this agent, in priority order.
    /// Different install methods / platforms may use different names.
    pub fn binary_candidates(&self) -> &'static [&'static str] {
        match self {
            AgentKind::Claude => &["claude"],
            AgentKind::OpenCode => &["opencode"],
            AgentKind::Codex => &["codex"],
            // Cursor ships as "agent" on some installs, "cursor-agent" on others
            AgentKind::Cursor => &["cursor-agent", "agent"],
        }
    }

    /// Environment variable names for API keys relevant to this agent.
    pub fn api_key_env_vars(&self) -> &'static [&'static str] {
        match self {
            AgentKind::Claude => &["ANTHROPIC_API_KEY"],
            AgentKind::Codex => &["OPENAI_API_KEY"],
            AgentKind::OpenCode => &["ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
            AgentKind::Cursor => &["CURSOR_API_KEY"],
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            AgentKind::Claude => "Claude Code",
            AgentKind::OpenCode => "OpenCode",
            AgentKind::Codex => "Codex",
            AgentKind::Cursor => "Cursor",
        }
    }
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

impl std::str::FromStr for AgentKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "claude" | "claude-code" | "claude_code" => Ok(AgentKind::Claude),
            "opencode" | "open-code" | "open_code" => Ok(AgentKind::OpenCode),
            "codex" | "openai-codex" | "openai_codex" => Ok(AgentKind::Codex),
            "cursor" | "cursor-agent" | "cursor_agent" => Ok(AgentKind::Cursor),
            _ => Err(format!(
                "unknown agent: `{s}` (expected: claude, opencode, codex, cursor)"
            )),
        }
    }
}

/// How the agent should handle tool permission prompts.
///
/// Only two modes: full access (default — "yolo") or read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Full access — auto-approve everything (yolo mode). This is the default.
    #[default]
    FullAccess,
    /// Read-only / plan mode — the agent cannot make changes.
    ReadOnly,
}

/// Desired output format for the final result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    /// Plain text — only the final assistant message.
    Text,
    /// JSON — structured result object.
    Json,
    /// NDJSON stream — one event per line as the run progresses.
    #[default]
    StreamJson,
    /// Markdown — human-readable transcript with headings and code blocks.
    Markdown,
}

/// Unified task configuration — everything needed to run a task on any agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConfig {
    /// The prompt / instruction to send to the agent.
    pub prompt: String,

    /// Which agent backend to use.
    pub agent: AgentKind,

    /// Working directory for the agent.
    #[serde(default)]
    pub cwd: Option<PathBuf>,

    /// Model override (e.g. "sonnet", "gpt-5-codex", "claude-opus-4-6").
    #[serde(default)]
    pub model: Option<String>,

    /// How to handle tool approvals.
    #[serde(default)]
    pub permission_mode: PermissionMode,

    /// Output format.
    #[serde(default)]
    pub output_format: OutputFormat,

    /// Maximum number of agentic turns before stopping.
    #[serde(default)]
    pub max_turns: Option<u32>,

    /// Maximum spend in USD before stopping.
    #[serde(default)]
    pub max_budget_usd: Option<f64>,

    /// Timeout in seconds for the entire run.
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Custom system prompt (replaces default).
    #[serde(default)]
    pub system_prompt: Option<String>,

    /// Custom system prompt to append to the default.
    #[serde(default)]
    pub append_system_prompt: Option<String>,

    /// Override the agent binary path.
    #[serde(default)]
    pub binary_path: Option<PathBuf>,

    /// Additional environment variables to set for the agent process.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Extra agent-specific flags passed through verbatim.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl TaskConfig {
    pub fn new(prompt: impl Into<String>, agent: AgentKind) -> Self {
        Self {
            prompt: prompt.into(),
            agent,
            cwd: None,
            model: None,
            permission_mode: PermissionMode::FullAccess,
            output_format: OutputFormat::StreamJson,
            max_turns: None,
            max_budget_usd: None,
            timeout_secs: None,
            system_prompt: None,
            append_system_prompt: None,
            binary_path: None,
            env: HashMap::new(),
            extra_args: Vec::new(),
        }
    }

    /// Create a builder for `TaskConfig`.
    pub fn builder(prompt: impl Into<String>, agent: AgentKind) -> TaskConfigBuilder {
        TaskConfigBuilder::new(prompt, agent)
    }
}

/// Fluent builder for `TaskConfig`.
///
/// ```rust,no_run
/// use harness::config::{AgentKind, TaskConfig};
/// let config = TaskConfig::builder("fix the bug", AgentKind::Claude)
///     .model("opus")
///     .timeout_secs(60)
///     .build();
/// ```
pub struct TaskConfigBuilder {
    config: TaskConfig,
}

impl TaskConfigBuilder {
    pub fn new(prompt: impl Into<String>, agent: AgentKind) -> Self {
        Self {
            config: TaskConfig::new(prompt, agent),
        }
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.config.cwd = Some(cwd.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = Some(model.into());
        self
    }

    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.config.permission_mode = mode;
        self
    }

    pub fn read_only(mut self) -> Self {
        self.config.permission_mode = PermissionMode::ReadOnly;
        self
    }

    pub fn output_format(mut self, format: OutputFormat) -> Self {
        self.config.output_format = format;
        self
    }

    pub fn max_turns(mut self, turns: u32) -> Self {
        self.config.max_turns = Some(turns);
        self
    }

    pub fn max_budget_usd(mut self, budget: f64) -> Self {
        self.config.max_budget_usd = Some(budget);
        self
    }

    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.config.timeout_secs = Some(secs);
        self
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.system_prompt = Some(prompt.into());
        self
    }

    pub fn append_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.append_system_prompt = Some(prompt.into());
        self
    }

    pub fn binary_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.binary_path = Some(path.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.env.insert(key.into(), value.into());
        self
    }

    pub fn extra_arg(mut self, arg: impl Into<String>) -> Self {
        self.config.extra_args.push(arg.into());
        self
    }

    pub fn extra_args(mut self, args: Vec<String>) -> Self {
        self.config.extra_args.extend(args);
        self
    }

    pub fn build(self) -> TaskConfig {
        self.config
    }
}
