pub mod claude;
pub mod codex;
pub mod cursor;
pub mod opencode;

use crate::config::AgentKind;
use crate::runner::AgentRunner;

/// Create the appropriate runner for the given agent kind.
pub fn create_runner(kind: AgentKind) -> Box<dyn AgentRunner> {
    match kind {
        AgentKind::Claude => Box::new(claude::ClaudeRunner),
        AgentKind::OpenCode => Box::new(opencode::OpenCodeRunner),
        AgentKind::Codex => Box::new(codex::CodexRunner),
        AgentKind::Cursor => Box::new(cursor::CursorRunner),
    }
}
