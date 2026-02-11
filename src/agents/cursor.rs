use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::{PermissionMode, TaskConfig};
use crate::error::{Error, Result};
use crate::event::*;
use crate::process::{spawn_and_stream, StreamHandle};
use crate::runner::AgentRunner;

/// Adapter for Cursor CLI (`agent` binary).
///
/// Headless invocation:
///   agent -p --output-format stream-json "<prompt>"
///
/// Stream format: NDJSON with event types:
///   - { type: "system", subtype: "init", session_id, model, cwd, apiKeySource, permissionMode }
///   - { type: "user", message: { role, content: [...] }, session_id }
///   - { type: "assistant", message: { role, content: [...] }, session_id }
///   - { type: "tool_call", subtype: "started"|"completed", call_id, tool_call, session_id }
///   - { type: "result", subtype: "success", result, session_id, duration_ms, is_error }
pub struct CursorRunner;

#[async_trait]
impl AgentRunner for CursorRunner {
    fn name(&self) -> &str {
        "cursor"
    }

    fn is_available(&self) -> bool {
        crate::runner::is_any_binary_available(crate::config::AgentKind::Cursor)
    }

    fn binary_path(&self, config: &TaskConfig) -> Result<PathBuf> {
        crate::runner::resolve_binary(crate::config::AgentKind::Cursor, config)
    }

    fn build_args(&self, config: &TaskConfig) -> Vec<String> {
        let mut args = vec![
            "-p".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
        ];

        if let Some(ref model) = config.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        match config.permission_mode {
            PermissionMode::FullAccess => {
                args.push("--force".to_string());
            }
            PermissionMode::ReadOnly => {
                args.push("--mode".to_string());
                args.push("plan".to_string());
            }
        }

        args.extend(config.extra_args.iter().cloned());

        // Prompt must be the last positional argument.
        args.push(config.prompt.clone());
        args
    }

    fn build_env(&self, _config: &TaskConfig) -> Vec<(String, String)> {
        // Cursor reads CURSOR_API_KEY from the environment.
        vec![]
    }

    async fn run(
        &self,
        config: &TaskConfig,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<StreamHandle> {
        spawn_and_stream(self, config, parse_cursor_line, cancel_token).await
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

fn parse_cursor_line(line: &str) -> Vec<Result<Event>> {
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
                    agent: "cursor".to_string(),
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
            let text = extract_message_text(&value);
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

        "user" => {
            // User message echoed back — usually the initial prompt.
            let text = extract_message_text(&value);
            if text.is_empty() {
                return vec![];
            }
            vec![Ok(Event::Message(MessageEvent {
                role: Role::User,
                text,
                usage: None,
                timestamp_ms: 0,
            }))]
        }

        "tool_call" => {
            let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            let call_id = value
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let tool_call = value.get("tool_call");
            let (tool_name, input_or_output) = extract_tool_info(tool_call);

            match subtype {
                "started" => vec![Ok(Event::ToolStart(ToolStartEvent {
                    call_id,
                    tool_name,
                    input: input_or_output,
                    timestamp_ms: 0,
                }))],
                "completed" => vec![Ok(Event::ToolEnd(ToolEndEvent {
                    call_id,
                    tool_name,
                    success: true,
                    output: input_or_output.map(|v| v.to_string()),
                    usage: None,
                    timestamp_ms: 0,
                }))],
                _ => vec![],
            }
        }

        "result" => {
            let subtype = value
                .get("subtype")
                .and_then(|v| v.as_str())
                .unwrap_or("success");
            let is_error = value
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let success = subtype == "success" && !is_error;

            vec![Ok(Event::Result(ResultEvent {
                success,
                text: value
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                session_id: value
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                duration_ms: value.get("duration_ms").and_then(|v| v.as_u64()),
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 0,
            }))]
        }

        _ => vec![],
    }
}

fn extract_message_text(value: &serde_json::Value) -> String {
    value
        .pointer("/message/content")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    if item.get("type")?.as_str()? == "text" {
                        item.get("text").and_then(|v| v.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn extract_tool_info(
    tool_call: Option<&serde_json::Value>,
) -> (String, Option<serde_json::Value>) {
    let Some(tc) = tool_call else {
        return ("unknown".to_string(), None);
    };

    // Cursor nests tool calls under keys like "readToolCall", "writeToolCall", etc.
    if let Some(obj) = tc.as_object() {
        for (key, val) in obj {
            if key.ends_with("ToolCall") || key.ends_with("_tool_call") {
                let name = key
                    .trim_end_matches("ToolCall")
                    .trim_end_matches("_tool_call")
                    .to_string();
                let data = val
                    .get("args")
                    .or_else(|| val.get("result"))
                    .cloned();
                return (name, data);
            }
        }

        // Fallback: check for "name" and "arguments" keys.
        if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
            let args = obj.get("arguments").cloned();
            return (name.to_string(), args);
        }
    }

    ("unknown".to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_init_event() {
        let line = r#"{"type":"system","subtype":"init","session_id":"s-42","model":"gpt-5.2","cwd":"/home/user","apiKeySource":"login","permissionMode":"default"}"#;
        let events = parse_cursor_line(line);
        assert_eq!(events.len(), 1);
        let event = events.into_iter().next().unwrap().unwrap();
        match event {
            Event::SessionStart(s) => {
                assert_eq!(s.session_id, "s-42");
                assert_eq!(s.agent, "cursor");
                assert_eq!(s.model, Some("gpt-5.2".into()));
                assert_eq!(s.cwd, Some("/home/user".into()));
            }
            other => panic!("expected SessionStart, got {other:?}"),
        }
    }

    #[test]
    fn parse_assistant_message() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I found the bug"}]},"session_id":"s-42"}"#;
        let events = parse_cursor_line(line);
        assert_eq!(events.len(), 1);
        let event = events.into_iter().next().unwrap().unwrap();
        match event {
            Event::Message(m) => {
                assert_eq!(m.role, Role::Assistant);
                assert_eq!(m.text, "I found the bug");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_call_started() {
        let line = r#"{"type":"tool_call","subtype":"started","call_id":"c-1","tool_call":{"readToolCall":{"args":{"path":"src/main.rs"}}},"session_id":"s-42"}"#;
        let events = parse_cursor_line(line);
        assert_eq!(events.len(), 1);
        let event = events.into_iter().next().unwrap().unwrap();
        match event {
            Event::ToolStart(t) => {
                assert_eq!(t.call_id, "c-1");
                assert_eq!(t.tool_name, "read");
                assert_eq!(t.input, Some(serde_json::json!({"path": "src/main.rs"})));
            }
            other => panic!("expected ToolStart, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_call_completed() {
        let line = r#"{"type":"tool_call","subtype":"completed","call_id":"c-1","tool_call":{"readToolCall":{"result":{"success":{"content":"fn main(){}"}}}},"session_id":"s-42"}"#;
        let events = parse_cursor_line(line);
        assert_eq!(events.len(), 1);
        match events.into_iter().next().unwrap().unwrap() {
            Event::ToolEnd(e) => {
                assert_eq!(e.call_id, "c-1");
                assert_eq!(e.tool_name, "read");
                assert!(e.success);
            }
            other => panic!("expected ToolEnd, got {other:?}"),
        }
    }

    #[test]
    fn parse_result_success() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":2000,"result":"Task completed","session_id":"s-42"}"#;
        let events = parse_cursor_line(line);
        assert_eq!(events.len(), 1);
        let event = events.into_iter().next().unwrap().unwrap();
        match event {
            Event::Result(r) => {
                assert!(r.success);
                assert_eq!(r.text, "Task completed");
                assert_eq!(r.session_id, "s-42");
                assert_eq!(r.duration_ms, Some(2000));
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    #[test]
    fn build_args_full_access_uses_force() {
        let mut config = TaskConfig::new("fix it", crate::config::AgentKind::Cursor);
        config.model = Some("sonnet-4.5-thinking".into());

        let runner = CursorRunner;
        let args = runner.build_args(&config);
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"--force".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"sonnet-4.5-thinking".to_string()));
        assert_eq!(args.last().unwrap(), "fix it");
    }
}
