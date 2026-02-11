use harness::config::*;

// ─── AgentKind parsing ───────────────────────────────────────────

#[test]
fn parse_agent_kind_all_variants() {
    let cases = vec![
        ("claude", AgentKind::Claude),
        ("claude-code", AgentKind::Claude),
        ("claude_code", AgentKind::Claude),
        ("CLAUDE", AgentKind::Claude),
        ("opencode", AgentKind::OpenCode),
        ("open-code", AgentKind::OpenCode),
        ("open_code", AgentKind::OpenCode),
        ("codex", AgentKind::Codex),
        ("openai-codex", AgentKind::Codex),
        ("openai_codex", AgentKind::Codex),
        ("cursor", AgentKind::Cursor),
        ("cursor-agent", AgentKind::Cursor),
        ("cursor_agent", AgentKind::Cursor),
    ];

    for (input, expected) in cases {
        let parsed: AgentKind = input.parse().unwrap();
        assert_eq!(parsed, expected, "Failed to parse `{input}`");
    }
}

#[test]
fn parse_agent_kind_invalid() {
    let result: Result<AgentKind, _> = "foobar".parse();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("unknown agent"));
}

// ─── AgentKind properties ────────────────────────────────────────

#[test]
fn agent_kind_default_binary() {
    assert_eq!(AgentKind::Claude.default_binary(), "claude");
    assert_eq!(AgentKind::OpenCode.default_binary(), "opencode");
    assert_eq!(AgentKind::Codex.default_binary(), "codex");
    assert_eq!(AgentKind::Cursor.default_binary(), "cursor-agent");
}

#[test]
fn agent_kind_display_name() {
    assert_eq!(AgentKind::Claude.display_name(), "Claude Code");
    assert_eq!(AgentKind::OpenCode.display_name(), "OpenCode");
    assert_eq!(AgentKind::Codex.display_name(), "Codex");
    assert_eq!(AgentKind::Cursor.display_name(), "Cursor");
}

#[test]
fn agent_kind_display_trait() {
    assert_eq!(format!("{}", AgentKind::Claude), "Claude Code");
    assert_eq!(format!("{}", AgentKind::Cursor), "Cursor");
}

// ─── TaskConfig ──────────────────────────────────────────────────

#[test]
fn task_config_new_defaults() {
    let config = TaskConfig::new("hello", AgentKind::Claude);
    assert_eq!(config.prompt, "hello");
    assert_eq!(config.agent, AgentKind::Claude);
    assert!(config.cwd.is_none());
    assert!(config.model.is_none());
    assert_eq!(config.permission_mode, PermissionMode::FullAccess);
    assert_eq!(config.output_format, OutputFormat::StreamJson);
    assert!(config.max_turns.is_none());
    assert!(config.max_budget_usd.is_none());
    assert!(config.timeout_secs.is_none());
    assert!(config.system_prompt.is_none());
    assert!(config.append_system_prompt.is_none());
    assert!(config.binary_path.is_none());
    assert!(config.env.is_empty());
    assert!(config.extra_args.is_empty());
}

#[test]
fn task_config_json_round_trip() {
    let config = TaskConfig::new("do the thing", AgentKind::Codex);
    let json = serde_json::to_string(&config).unwrap();
    let parsed: TaskConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.prompt, "do the thing");
    assert_eq!(parsed.agent, AgentKind::Codex);
}

// ─── PermissionMode ──────────────────────────────────────────────

#[test]
fn permission_mode_default_is_full_access() {
    let mode = PermissionMode::default();
    assert_eq!(mode, PermissionMode::FullAccess);
}

#[test]
fn permission_mode_json_round_trip() {
    let modes = vec![PermissionMode::FullAccess, PermissionMode::ReadOnly];

    for mode in modes {
        let json = serde_json::to_string(&mode).unwrap();
        let parsed: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, mode, "Round-trip failed for {mode:?}");
    }
}

// ─── OutputFormat ────────────────────────────────────────────────

#[test]
fn output_format_default_is_stream_json() {
    let fmt = OutputFormat::default();
    assert_eq!(fmt, OutputFormat::StreamJson);
}

#[test]
fn output_format_json_round_trip() {
    let formats = vec![
        OutputFormat::Text,
        OutputFormat::Json,
        OutputFormat::StreamJson,
        OutputFormat::Markdown,
    ];

    for fmt in formats {
        let json = serde_json::to_string(&fmt).unwrap();
        let parsed: OutputFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, fmt, "Round-trip failed for {fmt:?}");
    }
}

// ─── TaskConfigBuilder ──────────────────────────────────────────

#[test]
fn builder_minimal() {
    let config = TaskConfig::builder("hello", AgentKind::Claude).build();
    assert_eq!(config.prompt, "hello");
    assert_eq!(config.agent, AgentKind::Claude);
    assert!(config.model.is_none());
    assert_eq!(config.permission_mode, PermissionMode::FullAccess);
}

#[test]
fn builder_full() {
    let config = TaskConfig::builder("do work", AgentKind::Codex)
        .model("gpt-5-codex")
        .cwd("/tmp")
        .permission_mode(PermissionMode::ReadOnly)
        .output_format(OutputFormat::Json)
        .max_turns(10)
        .max_budget_usd(1.50)
        .timeout_secs(120)
        .system_prompt("be helpful")
        .append_system_prompt("also be concise")
        .binary_path("/usr/local/bin/codex")
        .build();
    assert_eq!(config.prompt, "do work");
    assert_eq!(config.agent, AgentKind::Codex);
    assert_eq!(config.model.as_deref(), Some("gpt-5-codex"));
    assert_eq!(config.cwd.as_ref().unwrap().to_str().unwrap(), "/tmp");
    assert_eq!(config.permission_mode, PermissionMode::ReadOnly);
    assert_eq!(config.output_format, OutputFormat::Json);
    assert_eq!(config.max_turns, Some(10));
    assert_eq!(config.max_budget_usd, Some(1.50));
    assert_eq!(config.timeout_secs, Some(120));
    assert_eq!(config.system_prompt.as_deref(), Some("be helpful"));
    assert_eq!(
        config.append_system_prompt.as_deref(),
        Some("also be concise")
    );
    assert_eq!(
        config.binary_path.as_ref().unwrap().to_str().unwrap(),
        "/usr/local/bin/codex"
    );
}

#[test]
fn builder_env_adds() {
    let config = TaskConfig::builder("task", AgentKind::Claude)
        .env("API_KEY", "sk-test")
        .env("DEBUG", "1")
        .build();
    assert_eq!(config.env.len(), 2);
    assert_eq!(config.env.get("API_KEY").unwrap(), "sk-test");
    assert_eq!(config.env.get("DEBUG").unwrap(), "1");
}

#[test]
fn builder_extra_args() {
    let config = TaskConfig::builder("task", AgentKind::Cursor)
        .extra_arg("--verbose")
        .extra_args(vec!["--debug".into(), "--trace".into()])
        .build();
    assert_eq!(config.extra_args, vec!["--verbose", "--debug", "--trace"]);
}

#[test]
fn builder_read_only_shorthand() {
    let config = TaskConfig::builder("analyze", AgentKind::OpenCode)
        .read_only()
        .build();
    assert_eq!(config.permission_mode, PermissionMode::ReadOnly);
}

#[test]
fn builder_chaining_order_independent() {
    // Verify that setting model then timeout is the same as timeout then model.
    let a = TaskConfig::builder("p", AgentKind::Claude)
        .model("opus")
        .timeout_secs(60)
        .build();
    let b = TaskConfig::builder("p", AgentKind::Claude)
        .timeout_secs(60)
        .model("opus")
        .build();
    assert_eq!(a.model, b.model);
    assert_eq!(a.timeout_secs, b.timeout_secs);
}

#[test]
fn builder_env_overwrites_same_key() {
    let config = TaskConfig::builder("task", AgentKind::Claude)
        .env("KEY", "first")
        .env("KEY", "second")
        .build();
    assert_eq!(config.env.len(), 1);
    assert_eq!(config.env.get("KEY").unwrap(), "second");
}

#[test]
fn builder_accepts_string_types() {
    // Verify impl Into<String> works with String, &str, and String::from.
    let prompt = String::from("my prompt");
    let model = String::from("sonnet");
    let config = TaskConfig::builder(prompt, AgentKind::Claude)
        .model(model)
        .system_prompt("system".to_string())
        .build();
    assert_eq!(config.prompt, "my prompt");
    assert_eq!(config.model.as_deref(), Some("sonnet"));
    assert_eq!(config.system_prompt.as_deref(), Some("system"));
}
