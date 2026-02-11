use harness::config::AgentKind;
use harness::models::{ModelEntry, ModelRegistry, ModelResolution};

// ─── Registry parsing ────────────────────────────────────────────

#[test]
fn builtin_registry_has_expected_models() {
    let reg = ModelRegistry::builtin();
    assert!(
        reg.models.contains_key("opus"),
        "builtin registry missing model: opus"
    );
}

#[test]
fn builtin_registry_model_metadata() {
    let reg = ModelRegistry::builtin();
    let opus = &reg.models["opus"];
    assert_eq!(opus.description, "Claude Opus 4.6");
    assert_eq!(opus.provider, "anthropic");
}

#[test]
fn parse_valid_toml() {
    let toml = r#"
[models.test]
description = "Test"
provider = "test"
claude = "test-claude-id"
codex = "test-codex-id"
"#;
    let reg = ModelRegistry::from_toml(toml).unwrap();
    assert!(reg.models.contains_key("test"));
    let entry = &reg.models["test"];
    assert_eq!(entry.claude.as_deref(), Some("test-claude-id"));
    assert_eq!(entry.codex.as_deref(), Some("test-codex-id"));
    assert!(entry.opencode.is_none());
    assert!(entry.cursor.is_none());
}

#[test]
fn parse_empty_toml() {
    let reg = ModelRegistry::from_toml("").unwrap();
    assert!(reg.models.is_empty());
}

#[test]
fn parse_malformed_toml() {
    let result = ModelRegistry::from_toml("{{{{ not valid");
    assert!(result.is_err());
}

#[test]
fn parse_toml_with_missing_fields() {
    let toml = r#"
[models.minimal]
description = "Minimal"
provider = "test"
"#;
    let reg = ModelRegistry::from_toml(toml).unwrap();
    let entry = &reg.models["minimal"];
    assert!(entry.claude.is_none());
    assert!(entry.codex.is_none());
}

// ─── Merge behavior ──────────────────────────────────────────────

#[test]
fn merge_project_overrides_canonical() {
    let canonical = ModelRegistry::from_toml(
        r#"
[models.sonnet]
description = "Claude Sonnet"
provider = "anthropic"
claude = "claude-sonnet-4-5-20250929"
"#,
    )
    .unwrap();

    let project = ModelRegistry::from_toml(
        r#"
[models.sonnet]
description = "Custom Sonnet"
provider = "anthropic"
claude = "my-custom-sonnet"
"#,
    )
    .unwrap();

    let merged = canonical.merge(&project);
    let entry = &merged.models["sonnet"];
    assert_eq!(entry.claude.as_deref(), Some("my-custom-sonnet"));
    assert_eq!(entry.description, "Custom Sonnet");
}

#[test]
fn merge_disjoint_registries() {
    let a = ModelRegistry::from_toml(
        r#"
[models.a]
description = "A"
provider = "test"
claude = "a-id"
"#,
    )
    .unwrap();

    let b = ModelRegistry::from_toml(
        r#"
[models.b]
description = "B"
provider = "test"
codex = "b-id"
"#,
    )
    .unwrap();

    let merged = a.merge(&b);
    assert!(merged.models.contains_key("a"));
    assert!(merged.models.contains_key("b"));
}

#[test]
fn merge_preserves_base_when_override_empty() {
    let base = ModelRegistry::builtin();
    let empty = ModelRegistry::default();
    let merged = base.merge(&empty);
    assert_eq!(merged.models.len(), base.models.len());
}

// ─── Resolution chain ────────────────────────────────────────────

#[test]
fn resolve_known_model_known_agent() {
    let reg = ModelRegistry::builtin();
    let res = reg.resolve("opus", AgentKind::Claude);
    assert!(matches!(res, ModelResolution::Resolved { .. }));
    assert_eq!(res.model_id(), "claude-opus-4-6");
}

#[test]
fn resolve_known_model_no_mapping() {
    let reg = ModelRegistry::builtin();
    let res = reg.resolve("opus", AgentKind::Codex);
    assert!(matches!(res, ModelResolution::NoAgentMapping { .. }));
    assert_eq!(res.model_id(), "opus");
}

#[test]
fn resolve_unknown_model() {
    let reg = ModelRegistry::builtin();
    let res = reg.resolve("nonexistent", AgentKind::Claude);
    assert!(matches!(res, ModelResolution::Passthrough { .. }));
    assert_eq!(res.model_id(), "nonexistent");
}

#[test]
fn resolve_raw_model_id_passthrough() {
    let reg = ModelRegistry::builtin();
    // A raw model ID that isn't a canonical name passes through.
    let res = reg.resolve("claude-opus-4-6", AgentKind::Claude);
    assert!(matches!(res, ModelResolution::Passthrough { .. }));
    assert_eq!(res.model_id(), "claude-opus-4-6");
}

#[test]
fn resolve_opus_for_opencode() {
    let reg = ModelRegistry::builtin();
    let res = reg.resolve("opus", AgentKind::OpenCode);
    assert!(matches!(res, ModelResolution::Resolved { .. }));
    assert_eq!(res.model_id(), "anthropic/claude-opus-4-6");
}

#[test]
fn resolve_opus_for_cursor() {
    let reg = ModelRegistry::builtin();
    let res = reg.resolve("opus", AgentKind::Cursor);
    assert!(matches!(res, ModelResolution::Resolved { .. }));
    assert_eq!(res.model_id(), "claude-opus-4-6");
}

#[test]
fn resolve_with_project_override() {
    let canonical = ModelRegistry::builtin();
    let project = ModelRegistry::from_toml(
        r#"
[models.sonnet]
description = "Custom Sonnet"
provider = "anthropic"
claude = "my-sonnet"
"#,
    )
    .unwrap();

    let merged = canonical.merge(&project);
    let res = merged.resolve("sonnet", AgentKind::Claude);
    assert_eq!(res.model_id(), "my-sonnet");
}

// ─── ModelEntry methods ──────────────────────────────────────────

#[test]
fn model_entry_agent_model() {
    let entry = ModelEntry {
        description: "test".into(),
        provider: "test".into(),
        claude: Some("c-id".into()),
        codex: Some("x-id".into()),
        opencode: None,
        cursor: None,
    };
    assert_eq!(entry.agent_model(AgentKind::Claude), Some("c-id"));
    assert_eq!(entry.agent_model(AgentKind::Codex), Some("x-id"));
    assert_eq!(entry.agent_model(AgentKind::OpenCode), None);
    assert_eq!(entry.agent_model(AgentKind::Cursor), None);
}

#[test]
fn model_entry_supported_agents() {
    let entry = ModelEntry {
        description: "test".into(),
        provider: "test".into(),
        claude: Some("c".into()),
        codex: None,
        opencode: Some("o".into()),
        cursor: Some("u".into()),
    };
    let agents = entry.supported_agents();
    assert_eq!(agents.len(), 3);
    assert!(agents.contains(&AgentKind::Claude));
    assert!(agents.contains(&AgentKind::OpenCode));
    assert!(agents.contains(&AgentKind::Cursor));
    assert!(!agents.contains(&AgentKind::Codex));
}

// ─── Registry utility methods ────────────────────────────────────

#[test]
fn names_returns_sorted() {
    let reg = ModelRegistry::builtin();
    let names = reg.names();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn models_for_agent_claude() {
    let reg = ModelRegistry::builtin();
    let models = reg.models_for_agent(AgentKind::Claude);
    let names: Vec<&str> = models.iter().map(|(n, _)| *n).collect();
    assert!(names.contains(&"opus"));
}

#[test]
fn models_for_agent_codex() {
    let reg = ModelRegistry::builtin();
    let models = reg.models_for_agent(AgentKind::Codex);
    // No models mapped for codex in the registry
    assert!(models.is_empty());
}

// ─── CLI tests ───────────────────────────────────────────────────

use assert_cmd::Command;
use predicates::prelude::*;

fn harness_cmd() -> Command {
    Command::cargo_bin("harness").unwrap()
}

#[test]
fn models_list_succeeds() {
    harness_cmd()
        .args(["models", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("opus"));
}

#[test]
fn models_list_json() {
    harness_cmd()
        .args(["models", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("["));
}

#[test]
fn models_list_filter_agent() {
    let result = harness_cmd()
        .args(["models", "list", "--agent", "claude"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&result.get_output().stdout);
    assert!(output.contains("opus"));
}

#[test]
fn models_list_filter_agent_json() {
    let result = harness_cmd()
        .args(["models", "list", "--agent", "claude", "--json"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&result.get_output().stdout);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
    // Should only contain models with claude mappings.
    for entry in &parsed {
        assert!(entry.get("claude").is_some());
    }
    let names: Vec<&str> = parsed
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"opus"));
}

#[test]
fn models_list_invalid_agent() {
    harness_cmd()
        .args(["models", "list", "--agent", "foobar"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown agent"));
}

#[test]
fn models_resolve_known() {
    harness_cmd()
        .args(["models", "resolve", "opus", "--agent", "claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains("claude-opus-4-6"));
}

#[test]
fn models_resolve_no_mapping() {
    let result = harness_cmd()
        .args(["models", "resolve", "opus", "--agent", "codex"])
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("no mapping") || stderr.contains("passing through"));
}

#[test]
fn models_resolve_unknown() {
    let result = harness_cmd()
        .args(["models", "resolve", "nonexistent", "--agent", "claude"])
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(stderr.contains("not found") || stderr.contains("passing through"));
}

#[test]
fn models_resolve_invalid_agent() {
    harness_cmd()
        .args(["models", "resolve", "opus", "--agent", "foobar"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown agent"));
}

#[test]
fn models_path_succeeds() {
    harness_cmd()
        .args(["models", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("models.toml"));
}

#[test]
fn models_help() {
    harness_cmd()
        .args(["models", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("model registry"));
}

// ─── Dry-run with model resolution ──────────────────────────────

#[test]
fn dry_run_resolves_model() {
    // With a known model alias, dry-run should show the resolved model ID.
    let result = harness_cmd()
        .args([
            "run",
            "--agent", "claude",
            "--model", "opus",
            "--prompt", "hello",
            "--dry-run",
        ])
        .assert();
    let output = result.get_output().clone();
    let combined =
        String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr);
    // The resolved model ID should appear in the Args line.
    if combined.contains("Args:") {
        assert!(
            combined.contains("claude-opus-4-6"),
            "dry-run should show resolved model ID, got: {combined}"
        );
    }
}

#[test]
fn dry_run_passthrough_raw_model() {
    // A raw model ID that doesn't match a registry name should pass through.
    let result = harness_cmd()
        .args([
            "run",
            "--agent", "claude",
            "--model", "claude-opus-4-6",
            "--prompt", "hello",
            "--dry-run",
        ])
        .assert();
    let output = result.get_output().clone();
    let combined =
        String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr);
    if combined.contains("Args:") {
        assert!(
            combined.contains("claude-opus-4-6"),
            "dry-run should pass through raw model ID, got: {combined}"
        );
    }
}

// ─── Config init (harness.toml) ─────────────────────────────────

#[test]
fn config_init_creates_harness_toml() {
    let tmp = tempfile::tempdir().unwrap();
    harness_cmd()
        .args(["config", "init"])
        .current_dir(tmp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("harness.toml"));

    let path = tmp.path().join("harness.toml");
    assert!(path.exists());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("default_agent"));
    assert!(content.contains("[models."));
}

#[test]
fn config_init_rejects_existing() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("harness.toml"), "# existing").unwrap();
    harness_cmd()
        .args(["config", "init"])
        .current_dir(tmp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

// ─── Project config walk-up ─────────────────────────────────────

#[test]
fn project_config_walkup_integration() {
    use harness::settings::ProjectConfig;

    let tmp = tempfile::tempdir().unwrap();
    let deep = tmp.path().join("a").join("b").join("c");
    std::fs::create_dir_all(&deep).unwrap();

    std::fs::write(
        tmp.path().join("a").join("harness.toml"),
        r#"
default_agent = "claude"
default_model = "opus"

[models.custom]
description = "Custom"
provider = "test"
claude = "custom-id"
"#,
    )
    .unwrap();

    let config = ProjectConfig::load(&deep).unwrap();
    assert_eq!(config.default_agent, Some("claude".into()));
    assert_eq!(config.default_model, Some("opus".into()));
    let reg = config.model_registry();
    assert!(reg.models.contains_key("custom"));
}
