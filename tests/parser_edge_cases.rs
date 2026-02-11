// Edge case tests for the line parsers in each adapter.
// These test malformed input, missing fields, and other boundary conditions.

// ─── Claude parser edge cases ────────────────────────────────────

mod claude {
    use harness::agents::claude::ClaudeRunner;
    use harness::config::{AgentKind, PermissionMode, TaskConfig};
    use harness::runner::AgentRunner;

    #[test]
    fn build_args_with_system_prompt() {
        let mut config = TaskConfig::new("hello", AgentKind::Claude);
        config.system_prompt = Some("You are a Rust expert".into());
        let args = ClaudeRunner.build_args(&config);
        assert!(args.contains(&"--system-prompt".to_string()));
        assert!(args.contains(&"You are a Rust expert".to_string()));
    }

    #[test]
    fn build_args_with_append_system_prompt() {
        let mut config = TaskConfig::new("hello", AgentKind::Claude);
        config.append_system_prompt = Some("Be concise".into());
        let args = ClaudeRunner.build_args(&config);
        assert!(args.contains(&"--append-system-prompt".to_string()));
        assert!(args.contains(&"Be concise".to_string()));
    }

    #[test]
    fn build_args_full_access_mode() {
        let config = TaskConfig::new("edit", AgentKind::Claude);
        let args = ClaudeRunner.build_args(&config);
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn build_args_read_only_mode() {
        let mut config = TaskConfig::new("plan", AgentKind::Claude);
        config.permission_mode = PermissionMode::ReadOnly;
        let args = ClaudeRunner.build_args(&config);
        assert!(args.contains(&"--permission-mode".to_string()));
        assert!(args.contains(&"plan".to_string()));
        assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn build_args_with_budget() {
        let mut config = TaskConfig::new("task", AgentKind::Claude);
        config.max_budget_usd = Some(5.0);
        let args = ClaudeRunner.build_args(&config);
        assert!(args.contains(&"--max-budget-usd".to_string()));
        assert!(args.contains(&"5".to_string()));
    }
}

// ─── Codex parser edge cases ─────────────────────────────────────

mod codex {
    use harness::agents::codex::CodexRunner;
    use harness::config::{AgentKind, PermissionMode, TaskConfig};
    use harness::runner::AgentRunner;

    #[test]
    fn build_args_full_access_mode() {
        let config = TaskConfig::new("hello", AgentKind::Codex);
        let args = CodexRunner.build_args(&config);
        assert_eq!(args[0], "exec");
        assert!(args.contains(&"--json".to_string()));
        // Full access (default) sets danger-full-access sandbox.
        assert!(args.iter().any(|a| a.contains("danger-full-access")));
        assert_eq!(args.last().unwrap(), "hello");
    }

    #[test]
    fn build_args_read_only_mode() {
        let mut config = TaskConfig::new("plan", AgentKind::Codex);
        config.permission_mode = PermissionMode::ReadOnly;
        let args = CodexRunner.build_args(&config);
        assert!(args.iter().any(|a| a.contains("read-only")));
        assert!(!args.iter().any(|a| a.contains("danger-full-access")));
    }

    #[test]
    fn prompt_is_always_last_arg() {
        let mut config = TaskConfig::new("my prompt", AgentKind::Codex);
        config.model = Some("gpt-5".into());
        config.extra_args = vec!["--extra".to_string()];
        let args = CodexRunner.build_args(&config);
        assert_eq!(args.last().unwrap(), "my prompt");
    }
}

// ─── Cursor parser edge cases ────────────────────────────────────

mod cursor {
    use harness::agents::cursor::CursorRunner;
    use harness::config::{AgentKind, PermissionMode, TaskConfig};
    use harness::runner::AgentRunner;

    #[test]
    fn build_args_full_access_uses_force() {
        let config = TaskConfig::new("hello", AgentKind::Cursor);
        let args = CursorRunner.build_args(&config);
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        // Full access (default) uses --force.
        assert!(args.contains(&"--force".to_string()));
    }

    #[test]
    fn build_args_read_only_uses_plan_mode() {
        let mut config = TaskConfig::new("analyze", AgentKind::Cursor);
        config.permission_mode = PermissionMode::ReadOnly;
        let args = CursorRunner.build_args(&config);
        assert!(args.contains(&"--mode".to_string()));
        assert!(args.contains(&"plan".to_string()));
        assert!(!args.contains(&"--force".to_string()));
    }

    #[test]
    fn prompt_is_always_last() {
        let mut config = TaskConfig::new("my prompt", AgentKind::Cursor);
        config.model = Some("gpt-5.2".into());
        config.extra_args = vec!["--custom".to_string()];
        let args = CursorRunner.build_args(&config);
        assert_eq!(args.last().unwrap(), "my prompt");
    }
}

// ─── OpenCode parser edge cases ──────────────────────────────────

mod opencode {
    use harness::agents::opencode::OpenCodeRunner;
    use harness::config::{AgentKind, TaskConfig};
    use harness::runner::AgentRunner;

    #[test]
    fn build_args_default() {
        let config = TaskConfig::new("hello", AgentKind::OpenCode);
        let args = OpenCodeRunner.build_args(&config);
        assert_eq!(args[0], "run");
        assert!(args.contains(&"--format".to_string()));
        assert!(args.contains(&"json".to_string()));
    }

    #[test]
    fn build_args_with_model() {
        let mut config = TaskConfig::new("hello", AgentKind::OpenCode);
        config.model = Some("anthropic/claude-sonnet-4-5".into());
        let args = OpenCodeRunner.build_args(&config);
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"anthropic/claude-sonnet-4-5".to_string()));
    }

    #[test]
    fn build_args_full_access_no_special_flags() {
        let config = TaskConfig::new("task", AgentKind::OpenCode);
        let args = OpenCodeRunner.build_args(&config);
        // OpenCode run auto-approves everything, so no special flags needed.
        assert!(!args.contains(&"--agent".to_string()));
    }

    #[test]
    fn prompt_is_last() {
        let mut config = TaskConfig::new("my prompt", AgentKind::OpenCode);
        config.model = Some("model".into());
        config.extra_args = vec!["--extra".to_string()];
        let args = OpenCodeRunner.build_args(&config);
        assert_eq!(args.last().unwrap(), "my prompt");
    }
}
