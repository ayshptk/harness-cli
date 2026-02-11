use assert_cmd::Command;
use predicates::prelude::*;

fn harness_cmd() -> Command {
    Command::cargo_bin("harness").unwrap()
}

// ─── Help & Version ───────────────────────────────────────────────

#[test]
fn help_flag_shows_usage() {
    harness_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Run Claude Code, OpenCode, Codex, or Cursor"));
}

#[test]
fn version_flag() {
    harness_cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

// ─── List command ─────────────────────────────────────────────────

#[test]
fn list_command_runs() {
    // Should always succeed, listing whatever agents are installed.
    harness_cmd().arg("list").assert().success();
}

#[test]
fn list_command_json() {
    harness_cmd()
        .args(["list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("["));
}

// ─── Check command ────────────────────────────────────────────────

#[test]
fn check_unknown_agent_fails() {
    harness_cmd()
        .args(["check", "nonexistent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown agent"));
}

#[test]
fn check_accepts_all_agent_names() {
    // These should at least parse correctly (exit 0 if installed, 1 if not).
    for name in &["claude", "opencode", "codex", "cursor"] {
        let result = harness_cmd().args(["check", name]).assert();
        // The exit code depends on whether the agent is installed, but
        // the command itself should not crash or fail to parse.
        let output = result.get_output().clone();
        let combined =
            String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr);
        // Should mention the agent name somewhere.
        assert!(
            combined.contains("Claude")
                || combined.contains("OpenCode")
                || combined.contains("Codex")
                || combined.contains("Cursor"),
            "Expected agent name in output for {name}, got: {combined}"
        );
    }
}

// ─── Run command validation ───────────────────────────────────────

#[test]
fn run_rejects_unknown_agent() {
    harness_cmd()
        .args(["run", "--agent", "foobar", "--prompt", "hello"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown agent"));
}

#[test]
fn run_rejects_unknown_permission_mode() {
    harness_cmd()
        .args([
            "run",
            "--agent", "claude",
            "--prompt", "hello",
            "--permissions", "foobar",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown permission mode"));
}

#[test]
fn run_rejects_unknown_output_format() {
    harness_cmd()
        .args([
            "run",
            "--agent", "claude",
            "--prompt", "hello",
            "--output", "xml",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown output format"));
}

#[test]
fn run_accepts_all_permission_modes() {
    // These should fail because the agent binary isn't available in CI,
    // but they should get past argument parsing.
    for mode in &["full-access", "full", "yolo", "default", "read-only", "readonly", "plan"] {
        let result = harness_cmd()
            .args([
                "run",
                "--agent", "claude",
                "--prompt", "hello",
                "--permissions", mode,
            ])
            .assert();

        // If claude is not installed, we get a specific error about binary not found.
        // The point is that arg parsing succeeded.
        let output = result.get_output().clone();
        let combined =
            String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr);
        assert!(
            !combined.contains("unknown permission mode"),
            "Permission mode `{mode}` was rejected: {combined}"
        );
    }
}

#[test]
fn run_accepts_all_output_formats() {
    for fmt in &["text", "json", "stream-json", "markdown"] {
        let result = harness_cmd()
            .args([
                "run",
                "--agent", "claude",
                "--prompt", "hello",
                "--output", fmt,
            ])
            .assert();

        let output = result.get_output().clone();
        let combined =
            String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr);
        assert!(
            !combined.contains("unknown output format"),
            "Output format `{fmt}` was rejected: {combined}"
        );
    }
}

#[test]
fn run_invalid_cwd_fails() {
    harness_cmd()
        .args([
            "run",
            "--agent", "claude",
            "--prompt", "hello",
            "--cwd", "/nonexistent/path/abc123",
        ])
        .assert()
        .failure();
}

// ─── Subcommand routing ──────────────────────────────────────────

#[test]
fn no_subcommand_shows_help() {
    harness_cmd()
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage"));
}

// ─── Dry run ─────────────────────────────────────────────────────

#[test]
fn dry_run_prints_command_info() {
    // dry-run should succeed even if the agent binary isn't installed.
    let result = harness_cmd()
        .args([
            "run",
            "--agent", "claude",
            "--prompt", "hello",
            "--dry-run",
        ])
        .assert();
    let output = result.get_output().clone();
    let combined =
        String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr);
    // dry-run resolves the binary, which may fail if not installed.
    // Either way, it should not crash from arg parsing.
    assert!(
        combined.contains("Binary:") || combined.contains("not found") || combined.contains("error"),
        "dry-run produced unexpected output: {combined}"
    );
}

// ─── Verbose flag ────────────────────────────────────────────────

#[test]
fn run_accepts_verbose_flag() {
    let result = harness_cmd()
        .args([
            "run",
            "--agent", "claude",
            "--prompt", "hello",
            "--verbose",
            "--dry-run",
        ])
        .assert();

    let output = result.get_output().clone();
    let combined =
        String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr);
    // Should not fail on arg parsing.
    assert!(
        !combined.contains("unexpected argument"),
        "verbose flag was rejected: {combined}"
    );
}

// ─── Diagnose flag ──────────────────────────────────────────────

#[test]
fn check_diagnose_shows_info() {
    let result = harness_cmd()
        .args(["check", "claude", "--diagnose"])
        .assert();

    let output = result.get_output().clone();
    let combined =
        String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr);
    // Should show diagnostics info.
    assert!(
        combined.contains("Diagnostics:") || combined.contains("candidates"),
        "diagnose should show diagnostic info: {combined}"
    );
}

#[test]
fn check_diagnose_json() {
    let result = harness_cmd()
        .args(["check", "claude", "--diagnose", "--json"])
        .assert();

    let output = result.get_output().clone();
    let stdout = String::from_utf8_lossy(&output.stdout);
    // JSON output should contain diagnostics key.
    assert!(
        stdout.contains("diagnostics"),
        "JSON diagnose should have diagnostics key: {stdout}"
    );
}

// ─── Models subcommand ───────────────────────────────────────────

#[test]
fn models_subcommand_help() {
    harness_cmd()
        .args(["models", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("model registry"));
}

#[test]
fn models_list_runs() {
    harness_cmd()
        .args(["models", "list"])
        .assert()
        .success();
}

#[test]
fn models_path_runs() {
    harness_cmd()
        .args(["models", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("models.toml"));
}

// ─── Output file flag ────────────────────────────────────────────

#[test]
fn run_accepts_output_file_flag() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let result = harness_cmd()
        .args([
            "run",
            "--agent", "claude",
            "--prompt", "hello",
            "--output-file",
            tmp.path().to_str().unwrap(),
            "--dry-run",
        ])
        .assert();

    let output = result.get_output().clone();
    let combined =
        String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr);
    // Dry run with output-file should not fail on arg parsing.
    assert!(
        combined.contains("Binary:") || combined.contains("not found") || combined.contains("error"),
        "output-file flag produced unexpected output: {combined}"
    );
}
