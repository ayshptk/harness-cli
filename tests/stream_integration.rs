use std::path::PathBuf;

use futures::StreamExt;
use harness::config::{AgentKind, TaskConfig};
use harness::event::*;

/// Create a small shell script that mimics a Claude Code stream-json output.
fn write_script(path: &std::path::Path, script: &str) {
    use std::io::Write;
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(script.as_bytes()).unwrap();
    f.sync_all().unwrap();
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn create_mock_claude_binary(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("claude");
    let script = r#"#!/bin/bash
# Mock Claude Code binary that outputs stream-json events.
echo '{"type":"system","subtype":"init","session_id":"mock-session","model":"mock-model","cwd":"/tmp"}'
echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I analyzed the code."}]}}'
echo '{"type":"result","subtype":"success","result":"Analysis complete.","session_id":"mock-session","duration_ms":500,"total_cost_usd":0.01}'
"#;
    write_script(&path, script);
    path
}

/// Create a mock Codex binary that outputs JSONL events (current format).
fn create_mock_codex_binary(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("codex");
    let script = r#"#!/bin/bash
echo '{"type":"thread.started","thread_id":"th-mock","model":"gpt-5-codex"}'
echo '{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Fixed the bug."}}'
echo '{"type":"item.started","item":{"id":"cmd-1","type":"command_execution","command":"git diff","status":"in_progress"}}'
echo '{"type":"item.completed","item":{"id":"cmd-1","type":"command_execution","command":"git diff","aggregated_output":"diff output","exit_code":0,"status":"completed"}}'
echo '{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":50,"output_tokens":20}}'
"#;
    write_script(&path, script);
    path
}

/// Create a mock Cursor binary that outputs stream-json events.
fn create_mock_cursor_binary(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("agent");
    let script = r#"#!/bin/bash
echo '{"type":"system","subtype":"init","session_id":"cur-mock","model":"gpt-5.2","cwd":"/project"}'
echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Refactored the module."}]}}'
echo '{"type":"tool_call","subtype":"started","call_id":"tc-1","tool_call":{"readToolCall":{"args":{"path":"src/main.rs"}}},"session_id":"cur-mock"}'
echo '{"type":"tool_call","subtype":"completed","call_id":"tc-1","tool_call":{"readToolCall":{"result":{"success":{"content":"fn main(){}"}}}},"session_id":"cur-mock"}'
echo '{"type":"result","subtype":"success","is_error":false,"duration_ms":800,"result":"Refactoring done.","session_id":"cur-mock"}'
"#;
    write_script(&path, script);
    path
}

/// Create a mock OpenCode binary that outputs JSON events (current format).
fn create_mock_opencode_binary(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("opencode");
    let script = r#"#!/bin/bash
echo '{"type":"step_start","sessionID":"oc-mock","part":{"type":"step-start","snapshot":"snap1"}}'
echo '{"type":"text","sessionID":"oc-mock","part":{"type":"text","text":"Analyzed the architecture."}}'
echo '{"type":"step_finish","sessionID":"oc-mock","part":{"type":"step-finish","reason":"stop","cost":0.02,"tokens":{"input":200,"output":80,"reasoning":0,"cache":{"read":100,"write":50}}}}'
"#;
    write_script(&path, script);
    path
}

/// Create a mock binary that fails (exits non-zero).
fn create_failing_binary(dir: &std::path::Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let script =
        "#!/bin/bash\necho '{\"type\":\"error\",\"message\":\"auth failed\"}'\nexit 1\n";
    write_script(&path, script);
    path
}

// ─── Integration tests ───────────────────────────────────────────

#[tokio::test]
async fn claude_mock_stream() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_claude_binary(dir.path());

    let mut config = TaskConfig::new("analyze code", AgentKind::Claude);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(result) = stream.next().await {
        match result {
            Ok(event) => events.push(event),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    // Should have: SessionStart, Message(user), Message(assistant), Result
    assert!(events.len() >= 4, "expected >= 4 events, got {}", events.len());

    assert!(matches!(&events[0], Event::SessionStart(s) if s.session_id == "mock-session"));
    assert!(matches!(&events[1], Event::Message(m) if m.role == Role::User && m.text == "analyze code"));
    assert!(matches!(&events[2], Event::Message(m) if m.text == "I analyzed the code."));
    assert!(matches!(&events[3], Event::Result(r) if r.success && r.text == "Analysis complete."));

    // Verify timestamps are populated.
    if let Event::SessionStart(s) = &events[0] {
        assert!(s.timestamp_ms > 0, "expected non-zero timestamp");
    }
}

#[tokio::test]
async fn codex_mock_stream() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_codex_binary(dir.path());

    let mut config = TaskConfig::new("fix bug", AgentKind::Codex);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(result) = stream.next().await {
        match result {
            Ok(event) => events.push(event),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    // Expected: SessionStart, Message(user), Message(assistant), ToolStart, ToolEnd, UsageDelta, Result
    assert!(events.len() >= 6, "expected >= 6 events, got {}: {events:?}", events.len());

    assert!(matches!(&events[0], Event::SessionStart(s) if s.session_id == "th-mock"));
    assert!(matches!(&events[1], Event::Message(m) if m.role == Role::User && m.text == "fix bug"));
    assert!(matches!(&events[2], Event::Message(m) if m.text == "Fixed the bug."));
    assert!(matches!(&events[3], Event::ToolStart(t) if t.tool_name == "shell"));
    assert!(matches!(&events[4], Event::ToolEnd(t) if t.tool_name == "shell" && t.success));
    assert!(events.iter().any(|e| matches!(e, Event::Result(r) if r.success)));
}

#[tokio::test]
async fn cursor_mock_stream() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_cursor_binary(dir.path());

    let mut config = TaskConfig::new("refactor", AgentKind::Cursor);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(result) = stream.next().await {
        match result {
            Ok(event) => events.push(event),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    assert!(events.len() >= 6, "expected >= 6 events, got {}", events.len());

    assert!(matches!(&events[0], Event::SessionStart(s) if s.session_id == "cur-mock"));
    assert!(matches!(&events[1], Event::Message(m) if m.role == Role::User && m.text == "refactor"));
    assert!(matches!(&events[2], Event::Message(m) if m.text == "Refactored the module."));
    assert!(matches!(&events[3], Event::ToolStart(t) if t.tool_name == "read"));
    assert!(matches!(&events[4], Event::ToolEnd(t) if t.call_id == "tc-1" && t.success));
    assert!(matches!(&events[5], Event::Result(r) if r.success));
}

#[tokio::test]
async fn opencode_mock_stream() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_opencode_binary(dir.path());

    let mut config = TaskConfig::new("review arch", AgentKind::OpenCode);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(result) = stream.next().await {
        match result {
            Ok(event) => events.push(event),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    // Expected: SessionStart, Message(user), Message(assistant), UsageDelta, Result
    assert!(events.len() >= 4, "expected >= 4 events, got {}: {events:?}", events.len());

    assert!(matches!(&events[0], Event::SessionStart(s) if s.session_id == "oc-mock"));
    assert!(matches!(&events[1], Event::Message(m) if m.role == Role::User && m.text == "review arch"));
    assert!(matches!(&events[2], Event::Message(m) if m.text == "Analyzed the architecture."));
    assert!(events.iter().any(|e| matches!(e, Event::Result(r) if r.success)));
}

#[tokio::test]
async fn failing_process_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_failing_binary(dir.path(), "claude");

    let mut config = TaskConfig::new("will fail", AgentKind::Claude);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();

    let mut had_error = false;
    while let Some(result) = stream.next().await {
        if result.is_err() {
            had_error = true;
            break;
        }
    }

    assert!(had_error, "expected an error event from failing process");
}

#[tokio::test]
async fn missing_binary_returns_error() {
    let mut config = TaskConfig::new("will fail", AgentKind::Claude);
    config.binary_path = Some(PathBuf::from("/nonexistent/path/claude_binary_xyz"));
    config.cwd = Some(std::env::temp_dir());

    let result = harness::run_task(&config).await;
    // Should fail because the binary doesn't exist (spawn error).
    // The spawn_and_stream should return Err or the stream should yield an error.
    if let Ok(mut stream) = result {
        // If we got a stream, the first item should be an error.
        let mut had_error = false;
        while let Some(item) = stream.next().await {
            if item.is_err() {
                had_error = true;
                break;
            }
        }
        assert!(had_error, "expected error for nonexistent binary");
    }
    // If result.is_err(), that's also correct — the binary wasn't found.
}

/// Test that extra_args are passed through to the agent.
#[tokio::test]
async fn extra_args_passed_through() {
    let dir = tempfile::tempdir().unwrap();

    // Create a binary that prints its arguments as JSON.
    let path = dir.path().join("claude");
    let script = r#"#!/bin/bash
# Print all args as a JSON array for inspection.
echo '{"type":"result","subtype":"success","result":"args: '"$*"'","session_id":"s1"}'
"#;
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut config = TaskConfig::new("test", AgentKind::Claude);
    config.binary_path = Some(path);
    config.cwd = Some(dir.path().to_path_buf());
    config.extra_args = vec!["--custom-flag".to_string(), "value".to_string()];

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut result_text = String::new();
    while let Some(Ok(event)) = stream.next().await {
        if let Event::Result(r) = event {
            result_text = r.text;
        }
    }

    assert!(
        result_text.contains("--custom-flag"),
        "extra args not passed through: {result_text}"
    );
    assert!(
        result_text.contains("value"),
        "extra args not passed through: {result_text}"
    );
}
