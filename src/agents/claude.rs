use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::{PermissionMode, TaskConfig};
use crate::error::{Error, Result};
use crate::event::*;
use crate::process::{spawn_and_stream, StreamHandle};
use crate::runner::AgentRunner;

/// Adapter for Claude Code (`claude` CLI).
///
/// Headless invocation:
///   claude -p "<prompt>" --output-format stream-json --verbose
///
/// Stream format: NDJSON with event types:
///   - { type: "system", subtype: "init", session_id, model, cwd }
///   - { type: "assistant", message: { role, content: [{ type: "text", text }, { type: "tool_use", ... }] } }
///   - { type: "user", message: { role, content: [{ type: "tool_result", ... }] } }
///   - { type: "result", subtype: "success"|"error_*", result, session_id, duration_ms, ... }
pub struct ClaudeRunner;

#[async_trait]
impl AgentRunner for ClaudeRunner {
    fn name(&self) -> &str {
        "claude"
    }

    fn is_available(&self) -> bool {
        crate::runner::is_any_binary_available(crate::config::AgentKind::Claude)
    }

    fn binary_path(&self, config: &TaskConfig) -> Result<PathBuf> {
        crate::runner::resolve_binary(crate::config::AgentKind::Claude, config)
    }

    fn build_args(&self, config: &TaskConfig) -> Vec<String> {
        let mut args = vec![
            "-p".to_string(),
            config.prompt.clone(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
        ];

        if let Some(ref model) = config.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        match config.permission_mode {
            PermissionMode::FullAccess => {
                args.push("--dangerously-skip-permissions".to_string());
            }
            PermissionMode::ReadOnly => {
                args.push("--permission-mode".to_string());
                args.push("plan".to_string());
            }
        }

        if let Some(turns) = config.max_turns {
            args.push("--max-turns".to_string());
            args.push(turns.to_string());
        }

        if let Some(budget) = config.max_budget_usd {
            args.push("--max-budget-usd".to_string());
            args.push(budget.to_string());
        }

        if let Some(ref sp) = config.system_prompt {
            args.push("--system-prompt".to_string());
            args.push(sp.clone());
        }

        if let Some(ref asp) = config.append_system_prompt {
            args.push("--append-system-prompt".to_string());
            args.push(asp.clone());
        }

        args.extend(config.extra_args.iter().cloned());
        args
    }

    fn build_env(&self, _config: &TaskConfig) -> Vec<(String, String)> {
        // Claude Code reads ANTHROPIC_API_KEY from the environment directly.
        vec![]
    }

    async fn run(
        &self,
        config: &TaskConfig,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<StreamHandle> {
        spawn_and_stream(self, config, parse_claude_line, cancel_token).await
    }

    fn capabilities(&self) -> crate::runner::AgentCapabilities {
        crate::runner::AgentCapabilities {
            supports_system_prompt: true,
            supports_budget: true,
            supports_model: true,
            supports_max_turns: true,
            supports_append_system_prompt: true,
        }
    }
}

fn parse_claude_line(line: &str) -> Vec<Result<Event>> {
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return vec![Err(Error::ParseError(format!("invalid JSON: {e}: {line}")))],
    };

    let event_type = match value.get("type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return vec![],
    };

    match event_type {
        "system" => {
            let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            if subtype == "init" {
                vec![Ok(Event::SessionStart(SessionStartEvent {
                    session_id: value
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    agent: "claude".to_string(),
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
            } else {
                vec![]
            }
        }

        "assistant" => {
            // An assistant message can contain both text blocks and tool_use blocks.
            // We emit a Message event for the text and ToolStart events for each tool_use.
            let mut events = Vec::new();

            let content = value.pointer("/message/content").and_then(|v| v.as_array());
            if let Some(blocks) = content {
                let mut text_parts = Vec::new();

                for block in blocks {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                                text_parts.push(t);
                            }
                        }
                        "tool_use" => {
                            let call_id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let tool_name = block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let input = block.get("input").cloned();
                            events.push(Ok(Event::ToolStart(ToolStartEvent {
                                call_id,
                                tool_name,
                                input,
                                timestamp_ms: 0,
                            })));
                        }
                        _ => {}
                    }
                }

                let text = text_parts.join("");
                if !text.is_empty() {
                    // Insert text message before tool events.
                    events.insert(
                        0,
                        Ok(Event::Message(MessageEvent {
                            role: Role::Assistant,
                            text,
                            usage: None,
                            timestamp_ms: 0,
                        })),
                    );
                }
            }

            events
        }

        "user" => {
            // User messages contain tool_result blocks — emit ToolEnd events.
            let mut events = Vec::new();

            let content = value.pointer("/message/content").and_then(|v| v.as_array());
            if let Some(blocks) = content {
                for block in blocks {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if block_type == "tool_result" {
                        let call_id = block
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let is_error = block
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let output = block
                            .get("content")
                            .map(|v| {
                                if let Some(s) = v.as_str() {
                                    s.to_string()
                                } else if let Some(arr) = v.as_array() {
                                    arr.iter()
                                        .filter_map(|item| {
                                            item.get("text").and_then(|t| t.as_str())
                                        })
                                        .collect::<Vec<_>>()
                                        .join("")
                                } else {
                                    v.to_string()
                                }
                            });
                        events.push(Ok(Event::ToolEnd(ToolEndEvent {
                            call_id,
                            tool_name: "unknown".to_string(),
                            success: !is_error,
                            output,
                            usage: None,
                            timestamp_ms: 0,
                        })));
                    }
                }
            }

            events
        }

        "stream_event" => {
            let mut events = Vec::new();

            // Partial streaming delta.
            let delta_text = value
                .pointer("/event/delta/text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !delta_text.is_empty() {
                events.push(Ok(Event::TextDelta(TextDeltaEvent {
                    text: delta_text.to_string(),
                    timestamp_ms: 0,
                })));
            }

            // Parse usage data from stream_event if present.
            if let Some(usage_val) = value.pointer("/event/usage").or_else(|| value.get("usage")) {
                let usage = parse_usage_data(usage_val);
                if usage.input_tokens.is_some() || usage.output_tokens.is_some() || usage.cost_usd.is_some() {
                    events.push(Ok(Event::UsageDelta(UsageDeltaEvent {
                        usage,
                        timestamp_ms: 0,
                    })));
                }
            }

            events
        }

        "result" => {
            let subtype = value
                .get("subtype")
                .and_then(|v| v.as_str())
                .unwrap_or("success");
            let success = subtype == "success";
            let result_text = value
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let session_id = value
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let duration_ms = value.get("duration_ms").and_then(|v| v.as_u64());
            let total_cost_usd = value.get("total_cost_usd").and_then(|v| v.as_f64());

            let usage = value.get("usage").map(parse_usage_data);

            vec![Ok(Event::Result(ResultEvent {
                success,
                text: result_text,
                session_id,
                duration_ms,
                total_cost_usd,
                usage,
                timestamp_ms: 0,
            }))]
        }

        _ => vec![],
    }
}

fn parse_usage_data(value: &serde_json::Value) -> UsageData {
    UsageData {
        input_tokens: value.get("input_tokens").and_then(|v| v.as_u64()),
        output_tokens: value.get("output_tokens").and_then(|v| v.as_u64()),
        cache_read_tokens: value
            .get("cache_read_input_tokens")
            .or_else(|| value.get("cache_read_tokens"))
            .and_then(|v| v.as_u64()),
        cache_creation_tokens: value
            .get("cache_creation_input_tokens")
            .or_else(|| value.get("cache_creation_tokens"))
            .and_then(|v| v.as_u64()),
        cost_usd: value
            .get("cost_usd")
            .or_else(|| value.get("cost"))
            .and_then(|v| v.as_f64()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_init_event() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc-123","model":"opus","cwd":"/tmp"}"#;
        let events = parse_claude_line(line);
        assert_eq!(events.len(), 1);
        let event = events.into_iter().next().unwrap().unwrap();
        match event {
            Event::SessionStart(s) => {
                assert_eq!(s.session_id, "abc-123");
                assert_eq!(s.agent, "claude");
                assert_eq!(s.model, Some("opus".into()));
                assert_eq!(s.cwd, Some("/tmp".into()));
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn parse_assistant_message() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello world"}]}}"#;
        let events = parse_claude_line(line);
        assert_eq!(events.len(), 1);
        let event = events.into_iter().next().unwrap().unwrap();
        match event {
            Event::Message(m) => {
                assert_eq!(m.role, Role::Assistant);
                assert_eq!(m.text, "Hello world");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn parse_assistant_with_tool_use() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Let me check"},{"type":"tool_use","id":"tu-1","name":"bash","input":{"command":"ls"}}]}}"#;
        let events = parse_claude_line(line);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], Ok(Event::Message(m)) if m.text == "Let me check"));
        assert!(matches!(&events[1], Ok(Event::ToolStart(t)) if t.tool_name == "bash" && t.call_id == "tu-1"));
    }

    #[test]
    fn parse_user_tool_result() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu-1","content":"file.txt\nREADME.md"}]}}"#;
        let events = parse_claude_line(line);
        assert_eq!(events.len(), 1);
        match events.into_iter().next().unwrap().unwrap() {
            Event::ToolEnd(t) => {
                assert_eq!(t.call_id, "tu-1");
                assert!(t.success);
                assert_eq!(t.output, Some("file.txt\nREADME.md".into()));
            }
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn parse_user_tool_result_error() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu-2","is_error":true,"content":"command not found"}]}}"#;
        let events = parse_claude_line(line);
        assert_eq!(events.len(), 1);
        match events.into_iter().next().unwrap().unwrap() {
            Event::ToolEnd(t) => {
                assert_eq!(t.call_id, "tu-2");
                assert!(!t.success);
            }
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn parse_stream_delta() {
        let line = r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"Hi"}}}"#;
        let events = parse_claude_line(line);
        assert_eq!(events.len(), 1);
        let event = events.into_iter().next().unwrap().unwrap();
        match event {
            Event::TextDelta(d) => assert_eq!(d.text, "Hi"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_result_success() {
        let line = r#"{"type":"result","subtype":"success","result":"Done","session_id":"s1","duration_ms":1234,"total_cost_usd":0.05}"#;
        let events = parse_claude_line(line);
        assert_eq!(events.len(), 1);
        let event = events.into_iter().next().unwrap().unwrap();
        match event {
            Event::Result(r) => {
                assert!(r.success);
                assert_eq!(r.text, "Done");
                assert_eq!(r.session_id, "s1");
                assert_eq!(r.duration_ms, Some(1234));
                assert_eq!(r.total_cost_usd, Some(0.05));
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn parse_result_error() {
        let line =
            r#"{"type":"result","subtype":"error_max_turns","result":"","session_id":"s1"}"#;
        let events = parse_claude_line(line);
        assert_eq!(events.len(), 1);
        match events.into_iter().next().unwrap().unwrap() {
            Event::Result(r) => assert!(!r.success),
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn build_args_defaults() {
        let config = TaskConfig::new("fix the bug", crate::config::AgentKind::Claude);
        let runner = ClaudeRunner;
        let args = runner.build_args(&config);
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"fix the bug".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
    }

    #[test]
    fn build_args_with_model_and_full_access() {
        let mut config = TaskConfig::new("do it", crate::config::AgentKind::Claude);
        config.model = Some("opus".into());
        config.max_turns = Some(10);

        let runner = ClaudeRunner;
        let args = runner.build_args(&config);
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"opus".to_string()));
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(args.contains(&"--max-turns".to_string()));
        assert!(args.contains(&"10".to_string()));
    }
}
