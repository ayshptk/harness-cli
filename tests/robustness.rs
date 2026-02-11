// Robustness tests: malformed input, process cleanup, config validation,
// concurrent session logging, event aggregation.

use futures::StreamExt;
use harness::config::{AgentKind, TaskConfig};
use harness::event::*;
use harness::runner::AgentRunner;

// ─── Helpers ────────────────────────────────────────────────────

/// Create a mock binary that outputs the given lines on stdout.
///
/// Writes to a temp file, sets permissions, then atomically renames into place
/// to avoid ETXTBSY on Linux CI (the target path is never opened for writing,
/// so exec() cannot race with a lingering write fd).
fn create_mock_binary(dir: &std::path::Path, name: &str, script: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let tmp = dir.join(format!(".{}.tmp", name));
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).unwrap();
        f.write_all(script.as_bytes()).unwrap();
        f.sync_all().unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::rename(&tmp, &path).unwrap();
    path
}

// ─── Malformed input tests ──────────────────────────────────────

/// Truncated JSON should produce a parse error, not panic.
#[tokio::test]
async fn malformed_truncated_json() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "claude",
        "#!/bin/bash\necho '{\"type\":\"result\",\"subtype\":\"su'\n",
    );
    let mut config = TaskConfig::new("test", AgentKind::Claude);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(result) = stream.next().await {
        events.push(result);
    }
    // Should have at least one parse error (from the truncated JSON),
    // and should NOT panic.
    let has_err = events.iter().any(|r| r.is_err());
    assert!(has_err, "expected parse error for truncated JSON");
}

/// Empty JSON object should be silently ignored.
#[tokio::test]
async fn malformed_empty_json_object() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "opencode",
        "#!/bin/bash\necho '{}'\necho '{\"type\":\"step_finish\",\"sessionID\":\"s1\",\"part\":{\"type\":\"step-finish\",\"reason\":\"stop\",\"cost\":0,\"tokens\":{\"input\":1,\"output\":1,\"reasoning\":0,\"cache\":{\"read\":0,\"write\":0}}}}'\n",
    );
    let mut config = TaskConfig::new("test", AgentKind::OpenCode);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(Ok(event)) = stream.next().await {
        events.push(event);
    }
    // Should have at least the Result event; empty object should be ignored.
    assert!(
        events.iter().any(|e| matches!(e, Event::Result(_))),
        "expected Result event"
    );
}

/// Null bytes in output should not panic (Codex).
#[tokio::test]
async fn malformed_null_bytes_codex() {
    let dir = tempfile::tempdir().unwrap();
    // printf sends null bytes; the binary also outputs a valid completion event.
    let binary = create_mock_binary(
        dir.path(),
        "codex",
        "#!/bin/bash\nprintf 'foo\\x00bar\\n'\necho '{\"type\":\"thread.completed\",\"thread_id\":\"t1\",\"summary\":\"ok\"}'\n",
    );
    let mut config = TaskConfig::new("test", AgentKind::Codex);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(result) = stream.next().await {
        events.push(result);
    }
    // Should not panic. May have errors but that's fine.
    // The thread.completed line should still parse.
}

/// Missing type field in Cursor JSON should be silently ignored.
#[tokio::test]
async fn malformed_cursor_no_type() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "agent",
        "#!/bin/bash\necho '{\"subtype\":\"init\",\"session_id\":\"s1\"}'\necho '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"done\",\"session_id\":\"s1\"}'\n",
    );
    let mut config = TaskConfig::new("test", AgentKind::Cursor);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(Ok(event)) = stream.next().await {
        events.push(event);
    }
    // The first line (no type) should be ignored; the result should parse.
    assert!(
        events.iter().any(|e| matches!(e, Event::Result(_))),
        "expected Result event"
    );
}

/// A very long single line (1MB) should not panic.
#[tokio::test]
async fn malformed_very_long_line() {
    let dir = tempfile::tempdir().unwrap();
    // Generate a 1MB line of 'x' characters, then a valid result.
    let binary = create_mock_binary(
        dir.path(),
        "opencode",
        "#!/bin/bash\nhead -c 1048576 /dev/zero | tr '\\0' 'x'\necho\necho '{\"result\":\"done\",\"session_id\":\"s1\"}'\n",
    );
    let mut config = TaskConfig::new("test", AgentKind::OpenCode);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut count = 0;
    while let Some(_result) = stream.next().await {
        count += 1;
        if count > 100 {
            break; // Safety: don't loop forever.
        }
    }
    // Just verifying we didn't panic or hang.
    assert!(count > 0, "expected at least one event");
}

/// Binary garbage input to Claude parser should produce errors, not panic.
#[tokio::test]
async fn malformed_binary_garbage_claude() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "claude",
        "#!/bin/bash\nhead -c 256 /dev/urandom | base64\n",
    );
    let mut config = TaskConfig::new("test", AgentKind::Claude);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut count = 0;
    while let Some(_result) = stream.next().await {
        count += 1;
        if count > 200 {
            break;
        }
    }
    // No panic is the assertion.
}

/// Tool call without subtype in Cursor should be silently ignored.
#[tokio::test]
async fn cursor_tool_call_no_subtype() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "agent",
        "#!/bin/bash\necho '{\"type\":\"tool_call\",\"call_id\":\"c1\",\"tool_call\":{\"readToolCall\":{\"args\":{\"path\":\"a.rs\"}}}}'\necho '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"ok\",\"session_id\":\"s1\"}'\n",
    );
    let mut config = TaskConfig::new("test", AgentKind::Cursor);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(Ok(event)) = stream.next().await {
        events.push(event);
    }
    // First line should be ignored (no subtype → empty match arm).
    assert!(events.iter().any(|e| matches!(e, Event::Result(_))));
}

/// Claude: empty content array should produce no message event.
#[tokio::test]
async fn claude_empty_content_array() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "claude",
        r#"#!/bin/bash
echo '{"type":"assistant","message":{"role":"assistant","content":[]}}'
echo '{"type":"result","subtype":"success","result":"done","session_id":"s1"}'
"#,
    );
    let mut config = TaskConfig::new("test", AgentKind::Claude);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(Ok(event)) = stream.next().await {
        events.push(event);
    }
    // Empty content array → no assistant Message event.
    // The normalizer may inject a synthetic user Message from the prompt.
    assert!(
        !events.iter().any(|e| matches!(e, Event::Message(m) if m.role == Role::Assistant)),
        "empty content array should not produce an assistant Message event"
    );
    assert!(events.iter().any(|e| matches!(e, Event::Result(_))));
}

/// Claude: unknown block type in content should be silently ignored.
#[tokio::test]
async fn claude_unknown_block_type() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "claude",
        r#"#!/bin/bash
echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"image","source":"img.png"},{"type":"text","text":"Hi"}]}}'
echo '{"type":"result","subtype":"success","result":"done","session_id":"s1"}'
"#,
    );
    let mut config = TaskConfig::new("test", AgentKind::Claude);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(Ok(event)) = stream.next().await {
        events.push(event);
    }
    // The "image" block should be ignored, but "text" block produces a Message.
    assert!(events.iter().any(|e| matches!(e, Event::Message(m) if m.text == "Hi")));
}

/// Codex: missing thread_id should default to "unknown".
#[tokio::test]
async fn codex_missing_thread_id() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "codex",
        r#"#!/bin/bash
echo '{"type":"thread.started","model":"gpt-5"}'
echo '{"type":"thread.completed","summary":"ok"}'
"#,
    );
    let mut config = TaskConfig::new("test", AgentKind::Codex);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(Ok(event)) = stream.next().await {
        events.push(event);
    }
    assert!(events.iter().any(|e| matches!(e, Event::SessionStart(s) if s.session_id == "unknown")));
}

/// Codex: empty message text should be filtered out.
#[tokio::test]
async fn codex_empty_message_text() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "codex",
        r#"#!/bin/bash
echo '{"type":"item.created","item":{"type":"message","role":"assistant","content":""}}'
echo '{"type":"thread.completed","thread_id":"t1","summary":"done"}'
"#,
    );
    let mut config = TaskConfig::new("test", AgentKind::Codex);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(Ok(event)) = stream.next().await {
        events.push(event);
    }
    // Empty message text should be filtered out.
    assert!(
        !events.iter().any(|e| matches!(e, Event::Message(m) if m.text.is_empty())),
        "empty message should be filtered"
    );
}

/// OpenCode: empty text event should be filtered out.
#[tokio::test]
async fn opencode_empty_text_event() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "opencode",
        r#"#!/bin/bash
echo '{"type":"text","sessionID":"s1","part":{"type":"text","text":""}}'
echo '{"type":"step_finish","sessionID":"s1","part":{"type":"step-finish","reason":"stop","cost":0,"tokens":{"input":1,"output":1,"reasoning":0,"cache":{"read":0,"write":0}}}}'
"#,
    );
    let mut config = TaskConfig::new("test", AgentKind::OpenCode);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let mut stream = harness::run_task(&config).await.unwrap();
    let mut events = Vec::new();
    while let Some(Ok(event)) = stream.next().await {
        events.push(event);
    }
    // Empty text event → no message emitted, but Result should exist.
    assert!(!events.iter().any(|e| matches!(e, Event::Message(m) if m.text.is_empty())));
    assert!(events.iter().any(|e| matches!(e, Event::Result(_))));
}

// ─── Config validation tests ────────────────────────────────────

#[test]
fn validate_config_codex_no_budget() {
    let mut config = TaskConfig::new("task", AgentKind::Codex);
    config.max_budget_usd = Some(5.0);
    let runner = harness::agents::codex::CodexRunner;
    let warnings = runner.validate_config(&config);
    assert!(warnings.iter().any(|w| w.message.contains("budget")));
}

#[test]
fn validate_config_cursor_no_system_prompt() {
    let mut config = TaskConfig::new("task", AgentKind::Cursor);
    config.system_prompt = Some("be helpful".into());
    let runner = harness::agents::cursor::CursorRunner;
    let warnings = runner.validate_config(&config);
    assert!(warnings.iter().any(|w| w.message.contains("system-prompt")));
}

#[test]
fn validate_config_opencode_no_system_prompt() {
    let mut config = TaskConfig::new("task", AgentKind::OpenCode);
    config.system_prompt = Some("be concise".into());
    let runner = harness::agents::opencode::OpenCodeRunner;
    let warnings = runner.validate_config(&config);
    assert!(warnings.iter().any(|w| w.message.contains("system-prompt")));
}

#[test]
fn validate_config_opencode_no_append_system_prompt() {
    let mut config = TaskConfig::new("task", AgentKind::OpenCode);
    config.append_system_prompt = Some("extra".into());
    let runner = harness::agents::opencode::OpenCodeRunner;
    let warnings = runner.validate_config(&config);
    assert!(
        warnings
            .iter()
            .any(|w| w.message.contains("append-system-prompt"))
    );
}

#[test]
fn validate_config_claude_all_supported() {
    // Claude supports everything, so there should be no warnings even with all fields set.
    let mut config = TaskConfig::new("task", AgentKind::Claude);
    config.system_prompt = Some("sp".into());
    config.append_system_prompt = Some("asp".into());
    config.max_budget_usd = Some(1.0);
    config.max_turns = Some(5);
    config.model = Some("opus".into());
    let runner = harness::agents::claude::ClaudeRunner;
    let warnings = runner.validate_config(&config);
    assert!(
        warnings.is_empty(),
        "Claude should support all features, got warnings: {:?}",
        warnings.iter().map(|w| &w.message).collect::<Vec<_>>()
    );
}

// ─── Event aggregation tests ────────────────────────────────────

#[test]
fn sum_costs_empty() {
    assert_eq!(sum_costs(&[]), 0.0);
}

#[test]
fn sum_costs_usage_deltas() {
    let events = vec![
        Event::UsageDelta(UsageDeltaEvent {
            usage: UsageData {
                cost_usd: Some(0.01),
                ..Default::default()
            },
            timestamp_ms: 0,
        }),
        Event::UsageDelta(UsageDeltaEvent {
            usage: UsageData {
                cost_usd: Some(0.02),
                ..Default::default()
            },
            timestamp_ms: 0,
        }),
        Event::Result(ResultEvent {
            success: true,
            text: "done".into(),
            session_id: "s".into(),
            duration_ms: None,
            total_cost_usd: Some(0.05),
            usage: None,
            timestamp_ms: 0,
        }),
    ];
    let total = sum_costs(&events);
    assert!((total - 0.08).abs() < 1e-10);
}

#[test]
fn total_tokens_counts() {
    let events = vec![
        Event::UsageDelta(UsageDeltaEvent {
            usage: UsageData {
                input_tokens: Some(100),
                output_tokens: Some(50),
                ..Default::default()
            },
            timestamp_ms: 0,
        }),
        Event::UsageDelta(UsageDeltaEvent {
            usage: UsageData {
                input_tokens: Some(200),
                output_tokens: Some(150),
                ..Default::default()
            },
            timestamp_ms: 0,
        }),
    ];
    let (input, output) = total_tokens(&events);
    assert_eq!(input, 300);
    assert_eq!(output, 200);
}

#[test]
fn extract_tool_calls_pairs() {
    let events = vec![
        Event::ToolStart(ToolStartEvent {
            call_id: "c1".into(),
            tool_name: "bash".into(),
            input: None,
            timestamp_ms: 0,
        }),
        Event::ToolStart(ToolStartEvent {
            call_id: "c2".into(),
            tool_name: "read".into(),
            input: None,
            timestamp_ms: 0,
        }),
        Event::ToolEnd(ToolEndEvent {
            call_id: "c1".into(),
            tool_name: "bash".into(),
            success: true,
            output: None,
            usage: None,
            timestamp_ms: 0,
        }),
    ];
    let pairs = extract_tool_calls(&events);
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0.call_id, "c1");
    assert!(pairs[0].1.is_some());
    assert_eq!(pairs[1].0.call_id, "c2");
    assert!(pairs[1].1.is_none()); // c2 never completed
}

// ─── Cancellation tests ────────────────────────────────────────

/// Cancelling the token should stop the stream.
#[tokio::test]
async fn cancel_stops_stream() {
    let dir = tempfile::tempdir().unwrap();
    // A binary that outputs events slowly (one per second).
    let binary = create_mock_binary(
        dir.path(),
        "claude",
        r#"#!/bin/bash
echo '{"type":"system","subtype":"init","session_id":"s1","model":"test"}'
sleep 10
echo '{"type":"result","subtype":"success","result":"never reached","session_id":"s1"}'
"#,
    );
    let mut config = TaskConfig::new("test", AgentKind::Claude);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let token = tokio_util::sync::CancellationToken::new();
    let handle = harness::run_task_with_cancel(&config, Some(token.clone()))
        .await
        .unwrap();
    let mut stream = handle.stream;

    // Read the first event.
    let first = stream.next().await;
    assert!(first.is_some(), "expected at least one event");

    // Cancel after getting the first event.
    token.cancel();

    // The stream should terminate quickly (not wait for the sleep 10).
    let start = std::time::Instant::now();
    while stream.next().await.is_some() {}
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "stream should stop quickly after cancel, took {:?}",
        elapsed
    );
}

/// Timeout-based cancellation should work.
#[tokio::test]
async fn timeout_via_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let binary = create_mock_binary(
        dir.path(),
        "claude",
        r#"#!/bin/bash
echo '{"type":"system","subtype":"init","session_id":"s1","model":"test"}'
sleep 30
"#,
    );
    let mut config = TaskConfig::new("test", AgentKind::Claude);
    config.binary_path = Some(binary);
    config.cwd = Some(dir.path().to_path_buf());

    let token = tokio_util::sync::CancellationToken::new();
    let handle = harness::run_task_with_cancel(&config, Some(token.clone()))
        .await
        .unwrap();
    let mut stream = handle.stream;

    // Read the first event (the init event) before cancelling.
    let first = stream.next().await;
    assert!(first.is_some(), "should get init event");

    // Now cancel (simulating a timeout).
    token.cancel();

    let start = std::time::Instant::now();
    // Drain remaining events — stream should close quickly.
    while stream.next().await.is_some() {}
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "should have stopped within 5s after cancel, took {:?}",
        elapsed
    );
}

// ─── Error Codes (Phase 9) ──────────────────────────────────────

#[test]
fn error_codes_stable() {
    use harness::Error;

    let cases = vec![
        (
            Error::BinaryNotFound {
                agent: "test".into(),
                binary: "test".into(),
            },
            "E001",
        ),
        (
            Error::SpawnFailed(std::io::Error::new(std::io::ErrorKind::NotFound, "test")),
            "E002",
        ),
        (
            Error::ProcessFailed {
                code: 1,
                stderr: "test".into(),
            },
            "E003",
        ),
        (Error::ParseError("test".into()), "E004"),
        (Error::Timeout(30), "E005"),
        (
            Error::InvalidWorkDir(std::path::PathBuf::from("/tmp")),
            "E006",
        ),
        (
            Error::Io(std::io::Error::new(std::io::ErrorKind::Other, "test")),
            "E007",
        ),
        (Error::Other("test".into()), "E999"),
    ];

    for (err, expected_code) in cases {
        assert_eq!(
            err.code(),
            expected_code,
            "Error {:?} should have code {}",
            err,
            expected_code
        );
    }
}

#[test]
fn error_codes_all_unique() {
    use harness::Error;
    use std::collections::HashSet;

    let errors: Vec<Box<dyn std::any::Any>> = vec![
        Box::new(Error::BinaryNotFound {
            agent: "a".into(),
            binary: "b".into(),
        }),
        Box::new(Error::SpawnFailed(std::io::Error::new(
            std::io::ErrorKind::Other,
            "x",
        ))),
        Box::new(Error::ProcessFailed {
            code: 1,
            stderr: "x".into(),
        }),
        Box::new(Error::ParseError("x".into())),
        Box::new(Error::Timeout(1)),
        Box::new(Error::InvalidWorkDir(std::path::PathBuf::from("/tmp"))),
        Box::new(Error::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            "x",
        ))),
        Box::new(Error::Other("x".into())),
    ];

    let codes: Vec<&str> = errors
        .iter()
        .map(|e| {
            if let Some(err) = e.downcast_ref::<Error>() {
                err.code()
            } else {
                ""
            }
        })
        .collect();

    let unique: HashSet<&&str> = codes.iter().collect();
    // All codes should be unique (note: E008 for Json is not tested because
    // constructing serde_json::Error directly is awkward, but the others
    // should all be unique).
    assert_eq!(unique.len(), codes.len(), "Error codes should be unique");
}
