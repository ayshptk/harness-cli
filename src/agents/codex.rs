use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::{PermissionMode, TaskConfig};
use crate::error::{Error, Result};
use crate::event::*;
use crate::process::{spawn_and_stream, StreamHandle};
use crate::runner::AgentRunner;

/// Adapter for OpenAI Codex CLI (`codex` binary).
///
/// Headless invocation:
///   codex exec --json "<prompt>"
///
/// Stream format: NDJSON with event types:
///   - { type: "thread.started", thread_id }
///   - { type: "turn.started" }
///   - { type: "item.started", item: { id, type: "command_execution", command, status: "in_progress", ... } }
///   - { type: "item.completed", item: { id, type: "agent_message"|"command_execution"|"reasoning", ... } }
///   - { type: "turn.completed", usage: { input_tokens, cached_input_tokens, output_tokens } }
///   - { type: "turn.failed", error }
///   - { type: "error", message }
pub struct CodexRunner;

#[async_trait]
impl AgentRunner for CodexRunner {
    fn name(&self) -> &str {
        "codex"
    }

    fn is_available(&self) -> bool {
        crate::runner::is_any_binary_available(crate::config::AgentKind::Codex)
    }

    fn binary_path(&self, config: &TaskConfig) -> Result<PathBuf> {
        crate::runner::resolve_binary(crate::config::AgentKind::Codex, config)
    }

    fn build_args(&self, config: &TaskConfig) -> Vec<String> {
        let mut args = vec!["exec".to_string(), "--json".to_string()];

        if let Some(ref model) = config.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        // Map permission mode to Codex's --sandbox + approval flags.
        match config.permission_mode {
            PermissionMode::FullAccess => {
                args.push("--sandbox".to_string());
                args.push("danger-full-access".to_string());
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
            }
            PermissionMode::ReadOnly => {
                args.push("--sandbox".to_string());
                args.push("read-only".to_string());
            }
        }

        args.extend(config.extra_args.iter().cloned());

        // Prompt must come last.
        args.push(config.prompt.clone());
        args
    }

    fn build_env(&self, _config: &TaskConfig) -> Vec<(String, String)> {
        // Codex reads CODEX_API_KEY or OPENAI_API_KEY from environment.
        vec![]
    }

    async fn run(
        &self,
        config: &TaskConfig,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<StreamHandle> {
        spawn_and_stream(self, config, parse_codex_line, cancel_token).await
    }

    fn capabilities(&self) -> crate::runner::AgentCapabilities {
        crate::runner::AgentCapabilities {
            supports_system_prompt: false,
            supports_budget: false,
            supports_model: true,
            supports_max_turns: false,
            supports_append_system_prompt: false,
        }
    }
}

fn parse_codex_line(line: &str) -> Vec<Result<Event>> {
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return vec![Err(Error::ParseError(format!("invalid JSON: {e}: {line}")))],
    };

    let event_type = match value.get("type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return vec![],
    };

    match event_type {
        "thread.started" => {
            let thread_id = value
                .get("thread_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            vec![Ok(Event::SessionStart(SessionStartEvent {
                session_id: thread_id,
                agent: "codex".to_string(),
                model: value
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                cwd: None,
                timestamp_ms: 0,
            }))]
        }

        "item.started" => {
            let item = match value.get("item") {
                Some(i) => i,
                None => return vec![],
            };
            let item_type = match item.get("type").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => return vec![],
            };

            match item_type {
                "command_execution" => {
                    let call_id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let command = item
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    vec![Ok(Event::ToolStart(ToolStartEvent {
                        call_id,
                        tool_name: "shell".to_string(),
                        input: Some(serde_json::json!({ "command": command })),
                        timestamp_ms: 0,
                    }))]
                }
                _ => vec![],
            }
        }

        "item.completed" => {
            let item = match value.get("item") {
                Some(i) => i,
                None => return vec![],
            };
            let item_type = match item.get("type").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => return vec![],
            };

            match item_type {
                "agent_message" | "message" => {
                    // { item: { type: "agent_message", text: "..." } }
                    // Also handle legacy { item: { type: "message", content: [...] } }
                    let text = item
                        .get("text")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .or_else(|| {
                            item.get("content")
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|c| c.get("text").and_then(|v| v.as_str()))
                                        .collect::<Vec<_>>()
                                        .join("")
                                })
                        })
                        .or_else(|| {
                            item.get("content")
                                .and_then(|v| v.as_str())
                                .map(String::from)
                        })
                        .unwrap_or_default();

                    if text.is_empty() {
                        return vec![];
                    }

                    let role_str = item
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("assistant");
                    let role = match role_str {
                        "user" => Role::User,
                        "system" => Role::System,
                        _ => Role::Assistant,
                    };

                    vec![Ok(Event::Message(MessageEvent {
                        role,
                        text,
                        usage: None,
                        timestamp_ms: 0,
                    }))]
                }

                "command_execution" | "command" | "shell" => {
                    let call_id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let command = item
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let exit_code = item.get("exit_code").and_then(|v| v.as_i64());
                    let success = exit_code.map(|c| c == 0).unwrap_or(true);
                    let output = item
                        .get("aggregated_output")
                        .or_else(|| item.get("output"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    // If we already emitted ToolStart from item.started, just emit ToolEnd.
                    // If there was no item.started (shouldn't happen, but be safe), emit both.
                    vec![Ok(Event::ToolEnd(ToolEndEvent {
                        call_id: call_id.clone(),
                        tool_name: "shell".to_string(),
                        success,
                        output: output.or_else(|| Some(serde_json::json!({ "command": command }).to_string())),
                        usage: None,
                        timestamp_ms: 0,
                    }))]
                }

                "file_change" => {
                    let call_id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let path = item
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Emit both ToolStart and ToolEnd for file_change items.
                    vec![
                        Ok(Event::ToolStart(ToolStartEvent {
                            call_id: call_id.clone(),
                            tool_name: "file_change".to_string(),
                            input: Some(serde_json::json!({ "path": path })),
                            timestamp_ms: 0,
                        })),
                        Ok(Event::ToolEnd(ToolEndEvent {
                            call_id,
                            tool_name: "file_change".to_string(),
                            success: true,
                            output: None,
                            usage: None,
                            timestamp_ms: 0,
                        })),
                    ]
                }

                // "reasoning" items — skip (internal thinking).
                _ => vec![],
            }
        }

        // Legacy: item.created — same as item.completed.
        "item.created" => {
            // Delegate to item.completed logic since structure is the same.
            let mut patched = value.clone();
            patched["type"] = serde_json::json!("item.completed");
            parse_codex_line(&patched.to_string())
        }

        "turn.completed" => {
            // { type: "turn.completed", usage: { input_tokens, cached_input_tokens, output_tokens } }
            let usage = value.get("usage").map(|u| UsageData {
                input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()),
                output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()),
                cache_read_tokens: u.get("cached_input_tokens").and_then(|v| v.as_u64()),
                cache_creation_tokens: None,
                cost_usd: None,
            });

            let mut events = Vec::new();

            if let Some(ref u) = usage {
                events.push(Ok(Event::UsageDelta(UsageDeltaEvent {
                    usage: u.clone(),
                    timestamp_ms: 0,
                })));
            }

            // turn.completed is typically the last event from Codex (no thread.completed),
            // so emit a Result event.
            events.push(Ok(Event::Result(ResultEvent {
                success: true,
                text: String::new(),
                session_id: String::new(),
                duration_ms: None,
                total_cost_usd: None,
                usage,
                timestamp_ms: 0,
            })));

            events
        }

        "turn.failed" => {
            let error_msg = value
                .get("error")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("message").and_then(|v| v.as_str()))
                .unwrap_or("turn failed")
                .to_string();
            vec![Ok(Event::Error(ErrorEvent {
                message: error_msg,
                code: Some("turn_failed".into()),
                timestamp_ms: 0,
            }))]
        }

        "thread.completed" => {
            let text = value
                .get("summary")
                .or_else(|| value.get("result"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let thread_id = value
                .get("thread_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            vec![Ok(Event::Result(ResultEvent {
                success: true,
                text,
                session_id: thread_id,
                duration_ms: value.get("duration_ms").and_then(|v| v.as_u64()),
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 0,
            }))]
        }

        "error" => {
            let msg = value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            let code = value
                .get("code")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            vec![Ok(Event::Error(ErrorEvent {
                message: msg,
                code,
                timestamp_ms: 0,
            }))]
        }

        // turn.started — skip (no useful data).
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Current format tests ────────────────────────────────────

    #[test]
    fn parse_thread_started() {
        let line = r#"{"type":"thread.started","thread_id":"th-123"}"#;
        let events = parse_codex_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::SessionStart(s)) => {
                assert_eq!(s.session_id, "th-123");
                assert_eq!(s.agent, "codex");
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn parse_agent_message() {
        let line = r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Hello!"}}"#;
        let events = parse_codex_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::Message(m)) => {
                assert_eq!(m.role, Role::Assistant);
                assert_eq!(m.text, "Hello!");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn parse_command_started() {
        let line = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/bash -lc 'ls'","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
        let events = parse_codex_line(line);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Ok(Event::ToolStart(t)) if t.tool_name == "shell" && t.call_id == "item_1"));
    }

    #[test]
    fn parse_command_completed() {
        let line = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"ls","aggregated_output":"file.txt\n","exit_code":0,"status":"completed"}}"#;
        let events = parse_codex_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::ToolEnd(t)) => {
                assert_eq!(t.call_id, "item_1");
                assert_eq!(t.tool_name, "shell");
                assert!(t.success);
                assert_eq!(t.output, Some("file.txt\n".into()));
            }
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn parse_command_failed() {
        let line = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"false","aggregated_output":"","exit_code":1,"status":"completed"}}"#;
        let events = parse_codex_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::ToolEnd(t)) => {
                assert!(!t.success);
            }
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_completed() {
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":8587,"cached_input_tokens":7808,"output_tokens":24}}"#;
        let events = parse_codex_line(line);
        assert!(events.len() >= 2);
        assert!(events.iter().any(|e| matches!(e, Ok(Event::UsageDelta(u)) if u.usage.input_tokens == Some(8587))));
        assert!(events.iter().any(|e| matches!(e, Ok(Event::Result(r)) if r.success)));
    }

    #[test]
    fn parse_file_change() {
        let line = r#"{"type":"item.completed","item":{"type":"file_change","id":"fc-1","path":"src/main.rs"}}"#;
        let events = parse_codex_line(line);
        assert_eq!(events.len(), 2, "expected ToolStart + ToolEnd");
        assert!(matches!(&events[0], Ok(Event::ToolStart(t)) if t.tool_name == "file_change"));
        assert!(matches!(&events[1], Ok(Event::ToolEnd(t)) if t.tool_name == "file_change" && t.success));
    }

    #[test]
    fn parse_turn_failed() {
        let line = r#"{"type":"turn.failed","error":"rate limit exceeded"}"#;
        let events = parse_codex_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::Error(e)) => {
                assert_eq!(e.message, "rate limit exceeded");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_event() {
        let line = r#"{"type":"error","message":"rate limit exceeded","code":"rate_limit"}"#;
        let events = parse_codex_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::Error(e)) => {
                assert_eq!(e.message, "rate limit exceeded");
                assert_eq!(e.code, Some("rate_limit".into()));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn parse_reasoning_item_skipped() {
        let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"thinking..."}}"#;
        let events = parse_codex_line(line);
        assert!(events.is_empty(), "reasoning items should be skipped");
    }

    // ── Legacy format tests ─────────────────────────────────────

    #[test]
    fn parse_legacy_item_created_message() {
        let line = r#"{"type":"item.created","item":{"type":"message","role":"assistant","content":[{"text":"Hello"}]}}"#;
        let events = parse_codex_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::Message(m)) => {
                assert_eq!(m.role, Role::Assistant);
                assert_eq!(m.text, "Hello");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn parse_legacy_thread_completed() {
        let line = r#"{"type":"thread.completed","thread_id":"th-123","summary":"All done","duration_ms":5000}"#;
        let events = parse_codex_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::Result(r)) => {
                assert!(r.success);
                assert_eq!(r.text, "All done");
                assert_eq!(r.session_id, "th-123");
                assert_eq!(r.duration_ms, Some(5000));
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    // ── build_args tests ────────────────────────────────────────

    #[test]
    fn build_args_full_access() {
        let config = TaskConfig::new("do it", crate::config::AgentKind::Codex);
        let runner = CodexRunner;
        let args = runner.build_args(&config);
        assert!(args.contains(&"exec".to_string()));
        assert!(args.contains(&"--json".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"danger-full-access".to_string()));
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert_eq!(args.last().unwrap(), "do it");
    }

    #[test]
    fn build_args_read_only() {
        let mut config = TaskConfig::new("analyze", crate::config::AgentKind::Codex);
        config.permission_mode = PermissionMode::ReadOnly;
        let runner = CodexRunner;
        let args = runner.build_args(&config);
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"read-only".to_string()));
        assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    #[test]
    fn build_args_with_model() {
        let mut config = TaskConfig::new("do it", crate::config::AgentKind::Codex);
        config.model = Some("gpt-5-codex".into());
        let runner = CodexRunner;
        let args = runner.build_args(&config);
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gpt-5-codex".to_string()));
    }
}
