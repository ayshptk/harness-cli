use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::{PermissionMode, TaskConfig};
use crate::error::Result;
use crate::event::*;
use crate::process::{spawn_and_stream, StreamHandle};
use crate::runner::AgentRunner;

/// Adapter for OpenCode CLI (`opencode` binary).
///
/// Headless invocation:
///   opencode run --format json "<prompt>"
///
/// The `run` subcommand is non-interactive: all permissions are auto-approved,
/// and the process exits after the task completes.
///
/// With `--format json`, output is NDJSON with event types:
///   - { type: "step_start", sessionID, part: { type: "step-start", ... } }
///   - { type: "text", sessionID, part: { type: "text", text, ... } }
///   - { type: "tool_use", sessionID, part: { type: "tool", callID, tool, state: { status, input, output, ... } } }
///   - { type: "step_finish", sessionID, part: { type: "step-finish", reason, cost, tokens: { input, output, cache: { read, write } } } }
pub struct OpenCodeRunner;

#[async_trait]
impl AgentRunner for OpenCodeRunner {
    fn name(&self) -> &str {
        "opencode"
    }

    fn is_available(&self) -> bool {
        crate::runner::is_any_binary_available(crate::config::AgentKind::OpenCode)
    }

    fn binary_path(&self, config: &TaskConfig) -> Result<PathBuf> {
        crate::runner::resolve_binary(crate::config::AgentKind::OpenCode, config)
    }

    fn build_args(&self, config: &TaskConfig) -> Vec<String> {
        let mut args = vec![
            "run".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ];

        if let Some(ref model) = config.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        // OpenCode `run` auto-approves all permissions by default.
        // For read-only, use a plan agent.
        match config.permission_mode {
            PermissionMode::FullAccess => {}
            PermissionMode::ReadOnly => {
                args.push("--agent".to_string());
                args.push("plan".to_string());
            }
        }

        args.extend(config.extra_args.iter().cloned());

        // Prompt is the final positional argument(s).
        args.push(config.prompt.clone());
        args
    }

    fn build_env(&self, _config: &TaskConfig) -> Vec<(String, String)> {
        // OpenCode reads provider API keys from environment (ANTHROPIC_API_KEY,
        // OPENAI_API_KEY, etc.) or from its config files.
        vec![]
    }

    async fn run(
        &self,
        config: &TaskConfig,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<StreamHandle> {
        spawn_and_stream(self, config, parse_opencode_line, cancel_token).await
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

fn parse_opencode_line(line: &str) -> Vec<Result<Event>> {
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => {
            // OpenCode may emit non-JSON progress text. Treat it as a text delta.
            return vec![Ok(Event::TextDelta(TextDeltaEvent {
                text: line.to_string(),
                timestamp_ms: 0,
            }))];
        }
    };

    if let Some(event_type) = value.get("type").and_then(|v| v.as_str()) {
        return parse_typed_event(event_type, &value);
    }

    vec![]
}

/// Extract usage data from OpenCode's `part.tokens` object.
fn extract_opencode_usage(part: &serde_json::Value) -> Option<UsageData> {
    let tokens = part.get("tokens")?;
    let input = tokens.get("input").and_then(|v| v.as_u64());
    let output = tokens.get("output").and_then(|v| v.as_u64());
    let cache_read = tokens
        .get("cache")
        .and_then(|c| c.get("read"))
        .and_then(|v| v.as_u64());
    let cache_write = tokens
        .get("cache")
        .and_then(|c| c.get("write"))
        .and_then(|v| v.as_u64());
    Some(UsageData {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_write,
        cost_usd: part.get("cost").and_then(|v| v.as_f64()),
    })
}

fn parse_typed_event(event_type: &str, value: &serde_json::Value) -> Vec<Result<Event>> {
    match event_type {
        // ── Current format (2025+) ──────────────────────────────────

        "step_start" => {
            // First step_start is treated as session init.
            let session_id = value
                .get("sessionID")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            vec![Ok(Event::SessionStart(SessionStartEvent {
                session_id,
                agent: "opencode".to_string(),
                model: None,
                cwd: None,
                timestamp_ms: 0,
            }))]
        }

        "text" => {
            // { type: "text", part: { text: "..." } }
            let text = value
                .pointer("/part/text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                return vec![];
            }
            vec![Ok(Event::Message(MessageEvent {
                role: Role::Assistant,
                text,
                usage: None,
                timestamp_ms: 0,
            }))]
        }

        "tool_use" => {
            // { type: "tool_use", part: { callID, tool, state: { status, input: { command, ... }, output, ... } } }
            let part = match value.get("part") {
                Some(p) => p,
                None => return vec![],
            };
            let call_id = part
                .get("callID")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tool_name = part
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let state = part.get("state");
            let input = state.and_then(|s| s.get("input")).cloned();
            let output = state
                .and_then(|s| s.get("output"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let status = state
                .and_then(|s| s.get("status"))
                .and_then(|v| v.as_str())
                .unwrap_or("completed");
            let success = status == "completed";

            // OpenCode emits tool_use with status=completed, so emit both
            // ToolStart and ToolEnd in one go.
            vec![
                Ok(Event::ToolStart(ToolStartEvent {
                    call_id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    input,
                    timestamp_ms: 0,
                })),
                Ok(Event::ToolEnd(ToolEndEvent {
                    call_id,
                    tool_name,
                    success,
                    output,
                    usage: None,
                    timestamp_ms: 0,
                })),
            ]
        }

        "step_finish" => {
            // { type: "step_finish", part: { reason: "stop"|"tool-calls", cost, tokens: { ... } } }
            let part = match value.get("part") {
                Some(p) => p,
                None => return vec![],
            };
            let reason = part
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let session_id = value
                .get("sessionID")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut events = Vec::new();

            // Always emit usage data if available.
            if let Some(usage) = extract_opencode_usage(part) {
                events.push(Ok(Event::UsageDelta(UsageDeltaEvent {
                    usage,
                    timestamp_ms: 0,
                })));
            }

            // reason=stop means the agent is done (final step).
            if reason == "stop" {
                events.push(Ok(Event::Result(ResultEvent {
                    success: true,
                    text: String::new(),
                    session_id,
                    duration_ms: None,
                    total_cost_usd: part.get("cost").and_then(|v| v.as_f64()),
                    usage: extract_opencode_usage(part),
                    timestamp_ms: 0,
                })));
            }
            // reason=tool-calls means more steps will follow — no Result yet.

            events
        }

        // ── Legacy format (kept for backward compat) ────────────────

        "session.start" | "session.init" | "init" => {
            let session_id = value
                .get("session_id")
                .or_else(|| value.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            vec![Ok(Event::SessionStart(SessionStartEvent {
                session_id,
                agent: "opencode".to_string(),
                model: value
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                cwd: value
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                timestamp_ms: 0,
            }))]
        }

        "message" | "assistant" => {
            let text = value
                .get("content")
                .or_else(|| value.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                return vec![];
            }
            vec![Ok(Event::Message(MessageEvent {
                role: Role::Assistant,
                text,
                usage: None,
                timestamp_ms: 0,
            }))]
        }

        "error" => {
            let msg = value
                .get("message")
                .or_else(|| value.get("error"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            vec![Ok(Event::Error(ErrorEvent {
                message: msg,
                code: value
                    .get("code")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                timestamp_ms: 0,
            }))]
        }

        "result" | "done" | "complete" => {
            let text = value
                .get("result")
                .or_else(|| value.get("content"))
                .or_else(|| value.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let session_id = value
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let success = value
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            vec![Ok(Event::Result(ResultEvent {
                success,
                text,
                session_id,
                duration_ms: value.get("duration_ms").and_then(|v| v.as_u64()),
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 0,
            }))]
        }

        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Current format tests ────────────────────────────────────

    #[test]
    fn parse_step_start() {
        let line = r#"{"type":"step_start","timestamp":1770612126829,"sessionID":"ses_abc123","part":{"type":"step-start","snapshot":"abc"}}"#;
        let events = parse_opencode_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::SessionStart(s)) => {
                assert_eq!(s.session_id, "ses_abc123");
                assert_eq!(s.agent, "opencode");
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn parse_text_event() {
        let line = r#"{"type":"text","sessionID":"ses_abc","part":{"type":"text","text":"Hello world","time":{"start":1,"end":2}}}"#;
        let events = parse_opencode_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::Message(m)) => {
                assert_eq!(m.role, Role::Assistant);
                assert_eq!(m.text, "Hello world");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_use_event() {
        let line = r#"{"type":"tool_use","sessionID":"ses_abc","part":{"type":"tool","callID":"toolu_01","tool":"bash","state":{"status":"completed","input":{"command":"ls"},"output":"file.txt\n"}}}"#;
        let events = parse_opencode_line(line);
        assert_eq!(events.len(), 2, "expected ToolStart + ToolEnd");
        assert!(matches!(&events[0], Ok(Event::ToolStart(t)) if t.tool_name == "bash" && t.call_id == "toolu_01"));
        assert!(matches!(&events[1], Ok(Event::ToolEnd(t)) if t.tool_name == "bash" && t.success && t.output == Some("file.txt\n".into())));
    }

    #[test]
    fn parse_step_finish_stop() {
        let line = r#"{"type":"step_finish","sessionID":"ses_abc","part":{"type":"step-finish","reason":"stop","cost":0.05,"tokens":{"input":100,"output":50,"reasoning":0,"cache":{"read":500,"write":100}}}}"#;
        let events = parse_opencode_line(line);
        // Should emit UsageDelta + Result.
        assert!(events.len() >= 2);
        assert!(events.iter().any(|e| matches!(e, Ok(Event::UsageDelta(_)))));
        assert!(events.iter().any(|e| matches!(e, Ok(Event::Result(r)) if r.success)));
    }

    #[test]
    fn parse_step_finish_tool_calls() {
        let line = r#"{"type":"step_finish","sessionID":"ses_abc","part":{"type":"step-finish","reason":"tool-calls","cost":0,"tokens":{"input":1,"output":98,"reasoning":0,"cache":{"read":100,"write":50}}}}"#;
        let events = parse_opencode_line(line);
        // reason=tool-calls should emit UsageDelta but NOT Result.
        assert!(events.iter().any(|e| matches!(e, Ok(Event::UsageDelta(_)))));
        assert!(!events.iter().any(|e| matches!(e, Ok(Event::Result(_)))));
    }

    // ── Legacy format tests (backward compat) ───────────────────

    #[test]
    fn parse_legacy_session_init() {
        let line = r#"{"type":"init","session_id":"oc-1","model":"claude-sonnet-4-5","cwd":"/project"}"#;
        let events = parse_opencode_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::SessionStart(s)) => {
                assert_eq!(s.session_id, "oc-1");
                assert_eq!(s.model, Some("claude-sonnet-4-5".into()));
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn parse_legacy_message() {
        let line = r#"{"type":"message","content":"Here is the answer"}"#;
        let events = parse_opencode_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::Message(m)) => {
                assert_eq!(m.text, "Here is the answer");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn parse_non_json_as_text_delta() {
        let line = "Processing your request...";
        let events = parse_opencode_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::TextDelta(d)) => assert_eq!(d.text, "Processing your request..."),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_error() {
        let line = r#"{"type":"error","message":"API key invalid","code":"auth_error"}"#;
        let events = parse_opencode_line(line);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(Event::Error(e)) => {
                assert_eq!(e.message, "API key invalid");
                assert_eq!(e.code, Some("auth_error".into()));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn build_args_default() {
        let config = TaskConfig::new("explain this", crate::config::AgentKind::OpenCode);
        let runner = OpenCodeRunner;
        let args = runner.build_args(&config);
        assert_eq!(args[0], "run");
        assert!(args.contains(&"--format".to_string()));
        assert!(args.contains(&"json".to_string()));
        assert_eq!(args.last().unwrap(), "explain this");
    }

    #[test]
    fn build_args_read_only() {
        let mut config = TaskConfig::new("analyze", crate::config::AgentKind::OpenCode);
        config.permission_mode = PermissionMode::ReadOnly;
        let runner = OpenCodeRunner;
        let args = runner.build_args(&config);
        assert!(args.contains(&"--agent".to_string()));
        assert!(args.contains(&"plan".to_string()));
    }

    #[test]
    fn build_args_full_access_no_agent_flag() {
        let config = TaskConfig::new("task", crate::config::AgentKind::OpenCode);
        let runner = OpenCodeRunner;
        let args = runner.build_args(&config);
        assert!(!args.contains(&"--agent".to_string()));
    }
}
