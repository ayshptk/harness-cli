use serde::{Deserialize, Serialize};

/// Returns the current epoch time in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Token usage and cost data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UsageData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Unified event stream — the common language spoken by all agent adapters.
///
/// Every adapter translates its native streaming output into this enum so that
/// consumers only need to handle one set of types regardless of which backend
/// is running.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The agent session has been initialized.
    SessionStart(SessionStartEvent),

    /// A chunk of assistant text (streaming delta).
    TextDelta(TextDeltaEvent),

    /// A complete assistant message.
    Message(MessageEvent),

    /// The agent is invoking a tool.
    ToolStart(ToolStartEvent),

    /// A tool invocation has completed.
    ToolEnd(ToolEndEvent),

    /// Incremental usage/cost update.
    UsageDelta(UsageDeltaEvent),

    /// The agent run has finished.
    Result(ResultEvent),

    /// An error occurred during the run.
    Error(ErrorEvent),
}

impl Event {
    /// Stamp the event with the current wall-clock time (epoch ms).
    pub fn stamp(self) -> Self {
        let ts = now_ms();
        match self {
            Event::SessionStart(mut e) => { e.timestamp_ms = ts; Event::SessionStart(e) }
            Event::TextDelta(mut e) => { e.timestamp_ms = ts; Event::TextDelta(e) }
            Event::Message(mut e) => { e.timestamp_ms = ts; Event::Message(e) }
            Event::ToolStart(mut e) => { e.timestamp_ms = ts; Event::ToolStart(e) }
            Event::ToolEnd(mut e) => { e.timestamp_ms = ts; Event::ToolEnd(e) }
            Event::UsageDelta(mut e) => { e.timestamp_ms = ts; Event::UsageDelta(e) }
            Event::Result(mut e) => { e.timestamp_ms = ts; Event::Result(e) }
            Event::Error(mut e) => { e.timestamp_ms = ts; Event::Error(e) }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionStartEvent {
    pub session_id: String,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextDeltaEvent {
    pub text: String,
    #[serde(default)]
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageEvent {
    pub role: Role,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageData>,
    #[serde(default)]
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Assistant,
    User,
    System,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::Assistant => f.write_str("assistant"),
            Role::User => f.write_str("user"),
            Role::System => f.write_str("system"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolStartEvent {
    pub call_id: String,
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(default)]
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolEndEvent {
    pub call_id: String,
    pub tool_name: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageData>,
    #[serde(default)]
    pub timestamp_ms: u64,
}

/// Incremental usage report emitted during streaming.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageDeltaEvent {
    pub usage: UsageData,
    #[serde(default)]
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResultEvent {
    pub success: bool,
    pub text: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageData>,
    #[serde(default)]
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorEvent {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default)]
    pub timestamp_ms: u64,
}

// ─── Aggregation helpers ────────────────────────────────────────

/// Sum all cost_usd values from UsageDelta and Result events.
pub fn sum_costs(events: &[Event]) -> f64 {
    let mut total = 0.0;
    for event in events {
        match event {
            Event::UsageDelta(u) => {
                if let Some(c) = u.usage.cost_usd {
                    total += c;
                }
            }
            Event::Result(r) => {
                if let Some(c) = r.total_cost_usd {
                    total += c;
                }
            }
            _ => {}
        }
    }
    total
}

/// Sum all input and output tokens from UsageDelta events.
pub fn total_tokens(events: &[Event]) -> (u64, u64) {
    let mut input = 0u64;
    let mut output = 0u64;
    for event in events {
        if let Event::UsageDelta(u) = event {
            if let Some(i) = u.usage.input_tokens {
                input += i;
            }
            if let Some(o) = u.usage.output_tokens {
                output += o;
            }
        }
    }
    (input, output)
}

/// Extract paired ToolStart/ToolEnd events by call_id.
pub fn extract_tool_calls(events: &[Event]) -> Vec<(&ToolStartEvent, Option<&ToolEndEvent>)> {
    let mut starts: Vec<(&ToolStartEvent, Option<&ToolEndEvent>)> = Vec::new();
    for event in events {
        if let Event::ToolStart(ts) = event {
            starts.push((ts, None));
        }
    }
    for event in events {
        if let Event::ToolEnd(te) = event {
            for (ts, end) in &mut starts {
                if ts.call_id == te.call_id && end.is_none() {
                    *end = Some(te);
                    break;
                }
            }
        }
    }
    starts
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::SessionStart(e) => write!(f, "[session:{}] agent={}", e.session_id, e.agent),
            Event::TextDelta(e) => write!(f, "{}", e.text),
            Event::Message(e) => write!(f, "[{}] {}", e.role, e.text),
            Event::ToolStart(e) => write!(f, "[tool:start] {}({})", e.tool_name, e.call_id),
            Event::ToolEnd(e) => {
                let status = if e.success { "ok" } else { "fail" };
                write!(f, "[tool:{}] {}({})", status, e.tool_name, e.call_id)
            }
            Event::UsageDelta(e) => {
                let input = e.usage.input_tokens.unwrap_or(0);
                let output = e.usage.output_tokens.unwrap_or(0);
                write!(f, "[usage] {input} in / {output} out")
            }
            Event::Result(e) => {
                let status = if e.success { "success" } else { "error" };
                write!(f, "[result:{}] {}", status, e.text)
            }
            Event::Error(e) => write!(f, "[error] {}", e.message),
        }
    }
}
