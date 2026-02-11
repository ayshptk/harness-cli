use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use futures::StreamExt;
use harness::{
    config::{AgentKind, OutputFormat, PermissionMode, TaskConfig},
    event::Event,
    logger::SessionLogger,
    models::{ModelRegistry, ModelResolution},
    run_task_with_cancel,
    settings::{ProjectConfig, Settings},
};

#[derive(Parser)]
#[command(
    name = "harness",
    about = "Unified coding agent harness",
    long_about = "Run Claude Code, OpenCode, Codex, or Cursor from a single CLI interface.\n\n\
                  Outputs NDJSON/text/JSON/markdown to stdout.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Run a task on a coding agent.
    Run {
        /// Which agent to use: claude, opencode, codex, cursor (optional — auto-detects)
        #[arg(short, long)]
        agent: Option<String>,

        /// The prompt / task description (reads from stdin if omitted and stdin is piped)
        #[arg(short, long)]
        prompt: Option<String>,

        /// Read prompt from a file
        #[arg(long)]
        prompt_file: Option<PathBuf>,

        /// Working directory for the agent
        #[arg(short = 'd', long)]
        cwd: Option<PathBuf>,

        /// Model to use (e.g. "sonnet", "opus", "gpt-5-codex")
        #[arg(short, long)]
        model: Option<String>,

        /// Permission mode: full-access (default, yolo) or read-only
        #[arg(long)]
        permissions: Option<String>,

        /// Output format: text, json, stream-json, markdown
        #[arg(short, long, default_value = "stream-json")]
        output: String,

        /// Maximum agentic turns
        #[arg(long)]
        max_turns: Option<u32>,

        /// Maximum spend in USD
        #[arg(long)]
        max_budget: Option<f64>,

        /// Timeout in seconds
        #[arg(long)]
        timeout: Option<u64>,

        /// Custom system prompt (replaces default)
        #[arg(long)]
        system_prompt: Option<String>,

        /// Append to default system prompt
        #[arg(long)]
        append_system_prompt: Option<String>,

        /// Override agent binary path
        #[arg(long)]
        binary: Option<PathBuf>,

        /// Print the resolved command without executing
        #[arg(long)]
        dry_run: bool,

        /// Enable verbose (debug-level) logging to stderr
        #[arg(short = 'v', long)]
        verbose: bool,

        /// Write output to a file in addition to stdout
        #[arg(long)]
        output_file: Option<PathBuf>,

        /// Extra flags passed through to the agent verbatim
        #[arg(last = true)]
        extra: Vec<String>,
    },

    /// List available agents on this system.
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Check if a specific agent is available.
    Check {
        /// Agent to check: claude, opencode, codex, cursor
        agent: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Show agent capabilities
        #[arg(long)]
        capabilities: bool,

        /// Show diagnostic information (binary path, PATH, env vars)
        #[arg(long)]
        diagnose: bool,
    },

    /// Manage configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Manage the model registry.
    Models {
        #[command(subcommand)]
        action: ModelsAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show the fully resolved configuration (global + project merged).
    Show,
    /// Create a template harness.toml in the current directory.
    Init,
    /// Print the config file path.
    Path,
}

#[derive(Subcommand)]
enum ModelsAction {
    /// List all models in the registry.
    List {
        /// Output as JSON.
        #[arg(long)]
        json: bool,

        /// Filter to models that have a mapping for this agent.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Force-fetch the latest registry from GitHub.
    Update,
    /// Resolve a model name for a specific agent.
    Resolve {
        /// The model name to resolve.
        name: String,

        /// The agent to resolve for.
        #[arg(long)]
        agent: String,
    },
    /// Print the cached registry file path.
    Path,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Check for --verbose / -v before tracing init (it's inside Run subcommand
    // but we peek at raw args to set log level early).
    let raw_args: Vec<String> = std::env::args().collect();
    let verbose_requested = raw_args.iter().any(|a| a == "--verbose" || a == "-v");

    // Load project config (harness.toml) and legacy settings.
    let cwd = std::env::current_dir().ok();
    let (project_config, project_config_path) = match cwd
        .as_deref()
        .and_then(ProjectConfig::load_with_path)
    {
        Some((config, path)) => (Some(config), Some(path)),
        None => (None, None),
    };
    let settings = Settings::load_with_project(cwd.as_deref());

    // Emit deprecation warning if old config files exist but no harness.toml.
    if project_config.is_none() {
        if let Some(ref dir) = cwd {
            let mut check = dir.clone();
            loop {
                if check.join(".harnessrc.toml").exists() {
                    eprintln!(
                        "warning: .harnessrc.toml is deprecated, migrate to harness.toml (run `harness config init`)"
                    );
                    break;
                }
                if !check.pop() {
                    break;
                }
            }
        }
    }

    // Initialize tracing: RUST_LOG takes precedence, then --verbose, then settings.
    let default_level = if verbose_requested {
        "debug"
    } else {
        project_config
            .as_ref()
            .and_then(|c| c.log_level.as_deref())
            .or(settings.log_level.as_deref())
            .unwrap_or("warn")
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_level)),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            agent,
            prompt,
            prompt_file,
            cwd,
            model,
            permissions,
            output,
            max_turns,
            max_budget,
            timeout,
            system_prompt,
            append_system_prompt,
            binary,
            dry_run,
            verbose: _,
            output_file,
            extra,
        } => {
            // Resolve agent: CLI flag > project config > legacy config > auto-detect.
            let agent_kind = match resolve_agent(agent.as_deref(), project_config.as_ref(), &settings) {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            };

            // Resolve prompt: --prompt > --prompt-file > stdin.
            let resolved_prompt = match resolve_prompt(prompt, prompt_file) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            };

            // Resolve permissions: CLI flag > project config > legacy settings > full-access.
            let perm_str = permissions
                .or_else(|| {
                    project_config
                        .as_ref()
                        .and_then(|c| c.default_permissions.clone())
                })
                .or_else(|| settings.default_permissions.clone())
                .unwrap_or_else(|| "full-access".to_string());
            let permission_mode = match perm_str.as_str() {
                "full-access" | "full" | "yolo" | "default" => PermissionMode::FullAccess,
                "read-only" | "readonly" | "plan" => PermissionMode::ReadOnly,
                other => {
                    eprintln!("error: unknown permission mode: `{other}` (expected: full-access, read-only)");
                    return ExitCode::from(2);
                }
            };

            let output_format = match output.as_str() {
                "text" => OutputFormat::Text,
                "json" => OutputFormat::Json,
                "stream-json" | "stream_json" | "ndjson" => OutputFormat::StreamJson,
                "markdown" | "md" => OutputFormat::Markdown,
                other => {
                    eprintln!("error: unknown output format: `{other}`");
                    return ExitCode::from(2);
                }
            };

            // Merge settings: CLI flags > project config > legacy settings.
            let raw_model = model
                .or_else(|| {
                    project_config
                        .as_ref()
                        .and_then(|c| c.agent_model(agent_kind))
                })
                .or_else(|| settings.agent_model(agent_kind));

            // Resolve model through the registry.
            let resolved_model = raw_model.map(|m| {
                resolve_model(&m, agent_kind, project_config.as_ref())
            });

            let resolved_binary = binary
                .or_else(|| {
                    project_config
                        .as_ref()
                        .and_then(|c| c.agent_binary(agent_kind))
                })
                .or_else(|| settings.agent_binary(agent_kind));
            let resolved_timeout = timeout
                .or_else(|| {
                    project_config
                        .as_ref()
                        .and_then(|c| c.default_timeout_secs)
                })
                .or(settings.default_timeout_secs);

            // Prepend agent extra_args from config before CLI extra args.
            let mut resolved_extra = project_config
                .as_ref()
                .map(|c| c.agent_extra_args(agent_kind))
                .unwrap_or_else(|| settings.agent_extra_args(agent_kind));
            resolved_extra.extend(extra);

            let config = TaskConfig {
                prompt: resolved_prompt,
                agent: agent_kind,
                cwd,
                model: resolved_model,
                permission_mode,
                output_format,
                max_turns,
                max_budget_usd: max_budget,
                timeout_secs: resolved_timeout,
                system_prompt,
                append_system_prompt,
                binary_path: resolved_binary,
                env: std::collections::HashMap::new(),
                extra_args: resolved_extra,
            };

            // Dry-run: show the resolved command and exit.
            if dry_run {
                return run_dry_run(&config);
            }

            // Print config validation warnings.
            let runner = harness::agents::create_runner(agent_kind);
            for warning in runner.validate_config(&config) {
                eprintln!("warning: {warning}");
            }

            run_headless(config, output_file).await
        }

        Commands::List { json } => {
            let available = harness::available_agents();
            if json {
                let items: Vec<_> = available
                    .iter()
                    .map(|a| {
                        let runner = harness::agents::create_runner(*a);
                        let dummy_config = TaskConfig::new("", *a);
                        let version = runner.version(&dummy_config);
                        serde_json::json!({
                            "agent": a.default_binary(),
                            "display_name": a.display_name(),
                            "version": version,
                        })
                    })
                    .collect();
                match serde_json::to_string_pretty(&items) {
                    Ok(json) => println!("{json}"),
                    Err(e) => {
                        eprintln!("error: failed to serialize output: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            } else if available.is_empty() {
                println!("No agents found. Install one of: claude, opencode, codex, cursor-agent");
            } else {
                println!("Available agents:");
                for agent in &available {
                    let runner = harness::agents::create_runner(*agent);
                    let dummy_config = TaskConfig::new("", *agent);
                    let version = runner
                        .version(&dummy_config)
                        .unwrap_or_else(|| "unknown".to_string());
                    println!(
                        "  - {} ({}) [{}]",
                        agent.display_name(),
                        agent.default_binary(),
                        version
                    );
                }
            }
            ExitCode::SUCCESS
        }

        Commands::Check {
            agent,
            json,
            capabilities,
            diagnose,
        } => {
            let agent_kind = match agent.parse::<AgentKind>() {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            };

            let runner = harness::agents::create_runner(agent_kind);
            let dummy_config = TaskConfig::new("", agent_kind);
            let is_available = runner.is_available();
            let version = runner.version(&dummy_config);
            let caps = runner.capabilities();

            if json {
                let mut obj = serde_json::json!({
                    "agent": agent_kind.default_binary(),
                    "display_name": agent_kind.display_name(),
                    "available": is_available,
                    "version": version,
                });
                if capabilities {
                    if let Ok(caps_val) = serde_json::to_value(&caps) {
                        obj["capabilities"] = caps_val;
                    }
                }
                if diagnose {
                    let diag = diagnose_agent(agent_kind, &dummy_config, &*runner);
                    obj["diagnostics"] = diag;
                }
                match serde_json::to_string_pretty(&obj) {
                    Ok(json) => println!("{json}"),
                    Err(e) => {
                        eprintln!("error: failed to serialize output: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                if is_available {
                    let ver = version.unwrap_or_else(|| "unknown version".to_string());
                    println!("{} is available ({})", agent_kind.display_name(), ver);
                } else {
                    eprintln!(
                        "{} is not available (binary `{}` not found in PATH)",
                        agent_kind.display_name(),
                        agent_kind.default_binary()
                    );
                }
                if capabilities {
                    println!("Capabilities:");
                    println!("  system_prompt:        {}", caps.supports_system_prompt);
                    println!("  append_system_prompt: {}", caps.supports_append_system_prompt);
                    println!("  budget:               {}", caps.supports_budget);
                    println!("  model:                {}", caps.supports_model);
                    println!("  max_turns:            {}", caps.supports_max_turns);
                }
                if diagnose {
                    println!("Diagnostics:");
                    let binary_path = runner.binary_path(&dummy_config);
                    println!(
                        "  binary_path: {}",
                        binary_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| "not found".into())
                    );
                    println!("  candidates:  {:?}", agent_kind.binary_candidates());

                    // Show relevant API key env vars (set/not set, never the value).
                    for key in agent_kind.api_key_env_vars() {
                        let status = if std::env::var(key).is_ok() {
                            "set"
                        } else {
                            "not set"
                        };
                        println!("  {key}: {status}");
                    }
                }
            }

            if is_available {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }

        Commands::Config { action } => match action {
            ConfigAction::Show => {
                // Show effective merged config: project config if available, else legacy.
                if let Some(ref pc) = project_config {
                    match toml::to_string_pretty(pc) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("error: failed to serialize config: {e}");
                            return ExitCode::FAILURE;
                        }
                    }
                } else {
                    match toml::to_string_pretty(&settings) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("error: failed to serialize config: {e}");
                            return ExitCode::FAILURE;
                        }
                    }
                }
                ExitCode::SUCCESS
            }
            ConfigAction::Init => {
                let path = std::env::current_dir()
                    .map(|d| d.join("harness.toml"))
                    .ok();
                let path = match path {
                    Some(p) => p,
                    None => {
                        eprintln!("error: cannot determine current directory");
                        return ExitCode::FAILURE;
                    }
                };
                if path.exists() {
                    eprintln!("Config file already exists at: {}", path.display());
                    return ExitCode::FAILURE;
                }
                match std::fs::write(&path, ProjectConfig::template()) {
                    Ok(()) => {
                        println!("Created config at: {}", path.display());
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error: failed to write config: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            ConfigAction::Path => {
                if let Some(ref p) = project_config_path {
                    println!("{}", p.display());
                } else if let Some(p) = Settings::config_path() {
                    println!("{}", p.display());
                } else {
                    eprintln!("error: cannot determine config directory");
                    return ExitCode::FAILURE;
                }
                ExitCode::SUCCESS
            }
        },

        Commands::Models { action } => match action {
            ModelsAction::List { json, agent } => {
                let registry = build_registry(project_config.as_ref());

                // Optional agent filter.
                let agent_filter: Option<AgentKind> = agent.as_deref().and_then(|a| {
                    match a.parse::<AgentKind>() {
                        Ok(k) => Some(k),
                        Err(e) => {
                            eprintln!("error: {e}");
                            None
                        }
                    }
                });
                if agent.is_some() && agent_filter.is_none() {
                    return ExitCode::from(2);
                }

                if json {
                    let entries: Vec<serde_json::Value> = registry
                        .names()
                        .iter()
                        .filter_map(|name| {
                            let entry = registry.models.get(*name)?;
                            if let Some(agent_kind) = agent_filter {
                                entry.agent_model(agent_kind)?;
                            }
                            let mut obj = serde_json::json!({
                                "name": name,
                                "description": entry.description,
                                "provider": entry.provider,
                            });
                            if let Some(ref v) = entry.claude {
                                obj["claude"] = serde_json::json!(v);
                            }
                            if let Some(ref v) = entry.codex {
                                obj["codex"] = serde_json::json!(v);
                            }
                            if let Some(ref v) = entry.opencode {
                                obj["opencode"] = serde_json::json!(v);
                            }
                            if let Some(ref v) = entry.cursor {
                                obj["cursor"] = serde_json::json!(v);
                            }
                            Some(obj)
                        })
                        .collect();
                    match serde_json::to_string_pretty(&entries) {
                        Ok(json) => println!("{json}"),
                        Err(e) => {
                            eprintln!("error: failed to serialize output: {e}");
                            return ExitCode::FAILURE;
                        }
                    }
                } else {
                    let names = registry.names();
                    if names.is_empty() {
                        println!("No models in registry.");
                    } else {
                        println!("Model registry ({} models):\n", names.len());
                        for name in &names {
                            let entry = &registry.models[*name];
                            if let Some(agent_kind) = agent_filter {
                                if entry.agent_model(agent_kind).is_none() {
                                    continue;
                                }
                            }
                            println!("  {} — {}", name, entry.description);
                            let agents = entry.supported_agents();
                            let mappings: Vec<String> = agents
                                .iter()
                                .map(|a| {
                                    let id = entry.agent_model(*a).unwrap_or("?");
                                    format!("{}={}", a.default_binary(), id)
                                })
                                .collect();
                            println!("    [{}] {}", entry.provider, mappings.join(", "));
                        }
                    }
                }
                ExitCode::SUCCESS
            }

            ModelsAction::Update => {
                match harness::registry::force_update() {
                    Ok(msg) => {
                        println!("{msg}");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::FAILURE
                    }
                }
            }

            ModelsAction::Resolve { name, agent } => {
                let agent_kind = match agent.parse::<AgentKind>() {
                    Ok(k) => k,
                    Err(e) => {
                        eprintln!("error: {e}");
                        return ExitCode::from(2);
                    }
                };

                let registry = build_registry(project_config.as_ref());
                let resolution = registry.resolve(&name, agent_kind);
                match &resolution {
                    ModelResolution::Resolved {
                        canonical_name,
                        agent_id,
                    } => {
                        println!("{agent_id}");
                        eprintln!(
                            "Resolved `{canonical_name}` for {} -> {agent_id}",
                            agent_kind.display_name()
                        );
                    }
                    ModelResolution::NoAgentMapping { canonical_name } => {
                        println!("{canonical_name}");
                        eprintln!(
                            "Model `{canonical_name}` exists but has no mapping for {} — passing through",
                            agent_kind.display_name()
                        );
                    }
                    ModelResolution::Passthrough { raw } => {
                        println!("{raw}");
                        eprintln!("Model `{raw}` not found in registry — passing through");
                    }
                }
                ExitCode::SUCCESS
            }

            ModelsAction::Path => {
                match harness::registry::canonical_path() {
                    Some(p) => println!("{}", p.display()),
                    None => {
                        eprintln!("error: cannot determine home directory");
                        return ExitCode::FAILURE;
                    }
                }
                ExitCode::SUCCESS
            }
        },
    }
}

fn format_token_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

fn resolve_agent(
    agent_arg: Option<&str>,
    project_config: Option<&ProjectConfig>,
    settings: &Settings,
) -> std::result::Result<AgentKind, String> {
    // 1. CLI flag.
    if let Some(name) = agent_arg {
        return name.parse();
    }

    // 2. Project config default (harness.toml).
    if let Some(kind) = project_config.and_then(|c| c.resolve_default_agent()) {
        return Ok(kind);
    }

    // 3. Legacy config file default.
    if let Some(kind) = settings.resolve_default_agent() {
        return Ok(kind);
    }

    // 4. Auto-detect: if exactly one agent is installed, use it.
    let available = harness::available_agents();
    match available.len() {
        0 => Err("no agent specified and none found in PATH. Install one of: claude, opencode, codex, cursor-agent".to_string()),
        1 => Ok(available[0]),
        _ => {
            let names: Vec<_> = available.iter().map(|a| a.default_binary()).collect();
            Err(format!(
                "no agent specified and multiple found: {}. Use --agent to choose.",
                names.join(", ")
            ))
        }
    }
}

fn resolve_prompt(
    prompt_arg: Option<String>,
    prompt_file: Option<PathBuf>,
) -> std::result::Result<String, String> {
    // 1. --prompt flag.
    if let Some(p) = prompt_arg {
        return Ok(p);
    }

    // 2. --prompt-file flag.
    if let Some(path) = prompt_file {
        return std::fs::read_to_string(&path)
            .map(|s| s.trim().to_string())
            .map_err(|e| format!("failed to read prompt file {}: {e}", path.display()));
    }

    // 3. stdin if not a TTY.
    if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        let trimmed = buf.trim().to_string();
        if trimmed.is_empty() {
            return Err("no prompt provided (--prompt, --prompt-file, or pipe to stdin)".to_string());
        }
        return Ok(trimmed);
    }

    Err("no prompt provided. Use --prompt, --prompt-file, or pipe to stdin".to_string())
}

/// Resolve a model name through the registry chain:
/// 1. Project harness.toml [models] section
/// 2. Canonical ~/.harness/models.toml
/// 3. Builtin models.toml
/// 4. Passthrough (return as-is)
fn resolve_model(
    raw_name: &str,
    agent: AgentKind,
    project_config: Option<&ProjectConfig>,
) -> String {
    // 1. Check project config models.
    if let Some(pc) = project_config {
        let project_reg = pc.model_registry();
        let res = project_reg.resolve(raw_name, agent);
        if let ModelResolution::Resolved { agent_id, .. } = res {
            return agent_id;
        }
    }

    // 2. Load canonical registry (cached/fetched/builtin).
    let canonical = harness::registry::load_canonical();

    // 3. If project config has a partial entry (found but no agent mapping),
    //    also try canonical before giving up.
    let res = canonical.resolve(raw_name, agent);
    match res {
        ModelResolution::Resolved { agent_id, .. } => agent_id,
        ModelResolution::NoAgentMapping { canonical_name } => {
            eprintln!(
                "warning: model `{canonical_name}` has no mapping for {} — passing through as-is",
                agent.display_name()
            );
            raw_name.to_string()
        }
        ModelResolution::Passthrough { raw } => raw,
    }
}

/// Build the effective model registry by merging all sources.
fn build_registry(project_config: Option<&ProjectConfig>) -> ModelRegistry {
    let canonical = harness::registry::load_canonical();
    if let Some(pc) = project_config {
        let project_reg = pc.model_registry();
        canonical.merge(&project_reg)
    } else {
        canonical
    }
}

fn diagnose_agent(
    agent_kind: AgentKind,
    config: &TaskConfig,
    runner: &dyn harness::runner::AgentRunner,
) -> serde_json::Value {
    let binary_path = runner
        .binary_path(config)
        .ok()
        .map(|p| p.display().to_string());

    let env_obj: serde_json::Map<String, serde_json::Value> = agent_kind
        .api_key_env_vars()
        .iter()
        .map(|k| {
            let status = if std::env::var(k).is_ok() { "set" } else { "not set" };
            (k.to_string(), serde_json::json!(status))
        })
        .collect();

    serde_json::json!({
        "binary_path": binary_path,
        "candidates": agent_kind.binary_candidates(),
        "env": env_obj,
    })
}

/// Shell-quote a string if it contains characters that need escaping.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // If it's safe as-is, return unchanged.
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '=' | '@'))
    {
        return s.to_string();
    }
    // Wrap in single quotes, escaping any embedded single quotes.
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn run_dry_run(config: &TaskConfig) -> ExitCode {
    let runner = harness::agents::create_runner(config.agent);

    let binary = match runner.binary_path(config) {
        Ok(b) => b.display().to_string(),
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let args = runner.build_args(config);
    let env_vars = runner.build_env(config);
    let cwd = config
        .cwd
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".to_string());

    println!("Binary: {binary}");
    let quoted: Vec<String> = args.iter().map(|a| shell_quote(a)).collect();
    println!("Args:   {}", quoted.join(" "));
    if !env_vars.is_empty() {
        println!("Env:");
        for (k, v) in &env_vars {
            println!("  {k}={v}");
        }
    }
    if !config.env.is_empty() {
        println!("User env:");
        for (k, v) in &config.env {
            println!("  {k}={v}");
        }
    }
    println!("Cwd:    {cwd}");

    ExitCode::SUCCESS
}

/// Helper that writes to stdout and optionally tees to a file.
struct TeeWriter {
    file: Option<std::fs::File>,
}

impl TeeWriter {
    fn new(path: Option<&PathBuf>) -> Self {
        let file = path.and_then(|p| {
            std::fs::File::create(p)
                .map_err(|e| {
                    eprintln!("warning: could not open output file {}: {e}", p.display());
                    e
                })
                .ok()
        });
        Self { file }
    }

    fn print(&mut self, text: &str) {
        print!("{text}");
        if let Some(ref mut f) = self.file {
            if let Err(e) = std::io::Write::write_all(f, text.as_bytes()) {
                tracing::debug!("failed to write to output file: {e}");
            }
        }
    }

    fn println(&mut self, text: &str) {
        println!("{text}");
        if let Some(ref mut f) = self.file {
            if let Err(e) = std::io::Write::write_all(f, text.as_bytes())
                .and_then(|()| std::io::Write::write_all(f, b"\n"))
            {
                tracing::debug!("failed to write to output file: {e}");
            }
        }
    }
}

async fn run_headless(config: TaskConfig, output_file: Option<PathBuf>) -> ExitCode {
    let output_format = config.output_format;
    let timeout_secs = config.timeout_secs;

    // Create a cancellation token for graceful shutdown.
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_for_signal = cancel_token.clone();

    // Install SIGINT/SIGTERM handler that cancels the token.
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        cancel_for_signal.cancel();
    });

    let handle = match run_task_with_cancel(&config, Some(cancel_token.clone())).await {
        Ok(h) => h,
        Err(e) => {
            match output_format {
                OutputFormat::Json | OutputFormat::StreamJson => {
                    let err = serde_json::json!({
                        "type": "error",
                        "message": e.to_string(),
                    });
                    println!("{}", err);
                }
                OutputFormat::Text | OutputFormat::Markdown => {
                    eprintln!("error: {e}");
                }
            }
            return ExitCode::FAILURE;
        }
    };

    let mut stream = handle.stream;

    // Create session logger — generate an ID from timestamp.
    let session_id = format!(
        "session-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        std::process::id()
    );
    let mut logger = SessionLogger::new(&session_id, &config).ok();

    // Open output file for tee if requested.
    let mut tee = TeeWriter::new(output_file.as_ref());

    let mut final_text = String::new();
    let mut success = false;
    let mut real_session_id = String::new();
    let mut duration_ms = None;
    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;
    let mut total_cost = 0.0f64;
    let mut agent_name = config.agent.display_name().to_string();
    let mut model_name = config.model.clone().unwrap_or_default();
    let mut md_header_printed = false;

    let cancel_for_timeout = cancel_token.clone();
    let process = async {
        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    // Log to session file.
                    if let Some(ref mut log) = logger {
                        log.log_event(&event);
                    }

                    match output_format {
                        OutputFormat::StreamJson => {
                            if let Ok(json) = serde_json::to_string(&event) {
                                tee.println(&json);
                            }
                        }
                        OutputFormat::Text => {
                            // In text mode, only print text deltas and messages.
                            match &event {
                                Event::TextDelta(d) => tee.print(&d.text),
                                Event::Message(m) => {
                                    if matches!(m.role, harness::event::Role::Assistant) {
                                        tee.println(&m.text);
                                    }
                                }
                                Event::Error(e) => eprintln!("error: {}", e.message),
                                _ => {}
                            }
                        }
                        OutputFormat::Markdown => {
                            // Emit a markdown header on first event.
                            if !md_header_printed {
                                tee.println(&format!("# harness session — {agent_name}\n"));
                                if !model_name.is_empty() {
                                    tee.println(&format!("**Model:** {model_name}\n"));
                                }
                                tee.println("---\n");
                                md_header_printed = true;
                            }
                            match &event {
                                Event::TextDelta(d) => tee.print(&d.text),
                                Event::Message(m) => {
                                    let role = match m.role {
                                        harness::event::Role::Assistant => "Assistant",
                                        harness::event::Role::User => "User",
                                        harness::event::Role::System => "System",
                                    };
                                    tee.println(&format!("\n### {role}\n"));
                                    tee.println(&m.text);
                                    tee.println("");
                                }
                                Event::ToolStart(t) => {
                                    tee.println(&format!(
                                        "\n> **Tool:** `{}` ({})",
                                        t.tool_name, t.call_id
                                    ));
                                    if let Some(ref input) = t.input {
                                        tee.println("```json");
                                        tee.println(
                                            &serde_json::to_string_pretty(input)
                                                .unwrap_or_default(),
                                        );
                                        tee.println("```");
                                    }
                                }
                                Event::ToolEnd(t) => {
                                    let status = if t.success { "ok" } else { "fail" };
                                    tee.println(&format!(
                                        "> **Tool done:** `{}` [{}]",
                                        t.tool_name, status
                                    ));
                                }
                                Event::Result(r) => {
                                    let status =
                                        if r.success { "Success" } else { "Error" };
                                    tee.println(&format!("\n---\n\n**Result:** {status}"));
                                    if !r.text.is_empty() {
                                        tee.println("");
                                        tee.println(&r.text);
                                    }
                                }
                                Event::Error(e) => {
                                    tee.println(&format!("\n> **Error:** {}", e.message));
                                }
                                Event::SessionStart(s) => {
                                    if !s.session_id.is_empty() {
                                        tee.println(&format!(
                                            "**Session:** {}\n",
                                            s.session_id
                                        ));
                                    }
                                }
                                Event::UsageDelta(_) => {}
                            }
                        }
                        OutputFormat::Json => {
                            // Collect for final JSON output.
                        }
                    }

                    // Track final result and usage.
                    match &event {
                        Event::Result(r) => {
                            success = r.success;
                            final_text.clone_from(&r.text);
                            real_session_id.clone_from(&r.session_id);
                            duration_ms = r.duration_ms;
                            if let Some(c) = r.total_cost_usd {
                                total_cost = total_cost.max(c);
                            }
                            if let Some(ref u) = r.usage {
                                if let Some(i) = u.input_tokens {
                                    total_input_tokens = total_input_tokens.max(i);
                                }
                                if let Some(o) = u.output_tokens {
                                    total_output_tokens = total_output_tokens.max(o);
                                }
                            }
                        }
                        Event::SessionStart(s) => {
                            real_session_id.clone_from(&s.session_id);
                            if let Some(ref m) = s.model {
                                model_name.clone_from(m);
                            }
                            agent_name = s.agent.clone();
                        }
                        Event::Message(m) => {
                            if matches!(m.role, harness::event::Role::Assistant) {
                                final_text.clone_from(&m.text);
                            }
                        }
                        Event::UsageDelta(u) => {
                            if let Some(i) = u.usage.input_tokens {
                                total_input_tokens += i;
                            }
                            if let Some(o) = u.usage.output_tokens {
                                total_output_tokens += o;
                            }
                            if let Some(c) = u.usage.cost_usd {
                                total_cost += c;
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    match output_format {
                        OutputFormat::StreamJson => {
                            let err_event = Event::Error(harness::event::ErrorEvent {
                                message: e.to_string(),
                                code: None,
                                timestamp_ms: 0,
                            });
                            if let Ok(json) = serde_json::to_string(&err_event) {
                                tee.println(&json);
                            }
                        }
                        OutputFormat::Text | OutputFormat::Markdown => {
                            eprintln!("error: {e}");
                        }
                        OutputFormat::Json => {}
                    }
                    success = false;
                    break;
                }
            }
        }
    };

    if let Some(timeout) = timeout_secs {
        match tokio::time::timeout(std::time::Duration::from_secs(timeout), process).await {
            Ok(()) => {}
            Err(_) => {
                // Timeout: cancel the subprocess gracefully.
                cancel_for_timeout.cancel();
                let msg = format!("timed out after {timeout}s");
                match output_format {
                    OutputFormat::StreamJson => {
                        let err = Event::Error(harness::event::ErrorEvent {
                            message: msg.clone(),
                            code: Some("timeout".into()),
                            timestamp_ms: 0,
                        });
                        if let Ok(json) = serde_json::to_string(&err) {
                            println!("{json}");
                        }
                    }
                    OutputFormat::Text | OutputFormat::Markdown => eprintln!("error: {msg}"),
                    OutputFormat::Json => {}
                }
                success = false;
            }
        }
    } else {
        process.await;
    }

    // Print cost summary to stderr for text/markdown modes.
    if matches!(output_format, OutputFormat::Text | OutputFormat::Markdown)
        && (total_cost > 0.0 || total_input_tokens > 0)
    {
        let dur_str = duration_ms
            .map(|ms| format!("{:.1}s", ms as f64 / 1000.0))
            .unwrap_or_default();
        eprintln!(
            "Total: {} in / {} out, ${:.3}{}",
            format_token_count(total_input_tokens),
            format_token_count(total_output_tokens),
            total_cost,
            if dur_str.is_empty() {
                String::new()
            } else {
                format!(", {dur_str}")
            }
        );
    }

    // Finalize session log.
    if let Some(ref mut log) = logger {
        log.finalize(success, duration_ms);
    }

    // For JSON output mode, emit the collected result.
    if output_format == OutputFormat::Json {
        let result = serde_json::json!({
            "type": "result",
            "success": success,
            "result": final_text,
            "session_id": real_session_id,
        });
        match serde_json::to_string_pretty(&result) {
            Ok(json) => tee.println(&json),
            Err(e) => {
                eprintln!("error: failed to serialize result: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    if success {
        ExitCode::SUCCESS
    } else {
        // Exit 0 if we got text output (no explicit failure), exit 1 otherwise.
        if !final_text.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── shell_quote ─────────────────────────────────────────────

    #[test]
    fn shell_quote_empty() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_safe_string() {
        assert_eq!(shell_quote("hello"), "hello");
        assert_eq!(shell_quote("--model"), "--model");
        assert_eq!(shell_quote("/usr/bin/claude"), "/usr/bin/claude");
        assert_eq!(shell_quote("key=value"), "key=value");
    }

    #[test]
    fn shell_quote_with_spaces() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
    }

    #[test]
    fn shell_quote_with_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_quote_with_special_chars() {
        assert_eq!(shell_quote("foo;bar"), "'foo;bar'");
        assert_eq!(shell_quote("a&b"), "'a&b'");
        assert_eq!(shell_quote("$(cmd)"), "'$(cmd)'");
    }

    // ─── format_token_count ──────────────────────────────────────

    #[test]
    fn format_token_count_small() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(42), "42");
        assert_eq!(format_token_count(999), "999");
    }

    #[test]
    fn format_token_count_thousands() {
        assert_eq!(format_token_count(1_000), "1.0k");
        assert_eq!(format_token_count(1_500), "1.5k");
        assert_eq!(format_token_count(999_999), "1000.0k");
    }

    #[test]
    fn format_token_count_millions() {
        assert_eq!(format_token_count(1_000_000), "1.0M");
        assert_eq!(format_token_count(2_500_000), "2.5M");
    }
}
