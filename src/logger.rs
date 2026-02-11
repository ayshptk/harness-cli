use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{AgentKind, TaskConfig};
use crate::error::{Error, Result};
use crate::event::Event;

/// Metadata about a session, stored alongside the NDJSON event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionMeta {
    pub session_id: String,
    pub agent: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub start_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub success: bool,
    /// Optional human-readable name for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// User-assigned tags for filtering/searching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// Logger that tees events to an NDJSON file.
pub struct SessionLogger {
    session_id: String,
    session_dir: PathBuf,
    writer: std::io::BufWriter<std::fs::File>,
    config: LoggerConfig,
    start_secs: u64,
}

struct LoggerConfig {
    agent: AgentKind,
    prompt: String,
    model: Option<String>,
    cwd: Option<String>,
    name: Option<String>,
}

impl SessionLogger {
    /// Create a new session logger.
    ///
    /// Creates `~/.local/share/harness/sessions/<id>.ndjson` and writes events there.
    pub fn new(session_id: &str, config: &TaskConfig) -> Result<Self> {
        Self::new_with_name(session_id, config, None)
    }

    /// Create a new session logger with an optional human-readable name.
    ///
    /// Writes events to a `.ndjson.tmp` file, which is atomically renamed
    /// to `.ndjson` on [`finalize`]. If the process crashes, the `.tmp` file
    /// remains for debugging.
    pub fn new_with_name(
        session_id: &str,
        config: &TaskConfig,
        name: Option<String>,
    ) -> Result<Self> {
        let session_dir = Self::sessions_dir()?;
        std::fs::create_dir_all(&session_dir)
            .map_err(|e| Error::Other(format!("failed to create session dir: {e}")))?;

        // Write to .tmp initially, rename to final path on finalize().
        let tmp_path = session_dir.join(format!("{session_id}.ndjson.tmp"));
        let file = std::fs::File::create(&tmp_path)
            .map_err(|e| Error::Other(format!("failed to create session log: {e}")))?;

        let start_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Ok(Self {
            session_id: session_id.to_string(),
            session_dir,
            writer: std::io::BufWriter::new(file),
            config: LoggerConfig {
                agent: config.agent,
                prompt: config.prompt.clone(),
                model: config.model.clone(),
                cwd: config.cwd.as_ref().map(|p| p.display().to_string()),
                name,
            },
            start_secs,
        })
    }

    /// Log a single event to the session file.
    pub fn log_event(&mut self, event: &Event) {
        match serde_json::to_string(event) {
            Ok(json) => {
                if let Err(e) = writeln!(self.writer, "{json}") {
                    tracing::warn!("failed to write session log: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("failed to serialize event for session log: {e}");
            }
        }
    }

    /// Finalize the session: flush and atomically rename the NDJSON file,
    /// then write meta.json.
    pub fn finalize(&mut self, success: bool, duration_ms: Option<u64>) {
        if let Err(e) = self.writer.flush() {
            tracing::warn!("failed to flush session log: {e}");
        }

        // Atomic rename: .ndjson.tmp → .ndjson
        let tmp_path = self
            .session_dir
            .join(format!("{}.ndjson.tmp", self.session_id));
        let final_path = self.session_dir.join(format!("{}.ndjson", self.session_id));
        if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
            tracing::warn!("failed to rename session log: {e}");
        }

        let meta = SessionMeta {
            session_id: self.session_id.clone(),
            agent: self.config.agent.display_name().to_string(),
            prompt: self.config.prompt.clone(),
            model: self.config.model.clone(),
            cwd: self.config.cwd.clone(),
            start_time: self.start_secs.to_string(),
            duration_ms,
            success,
            name: self.config.name.clone(),
            tags: None,
        };

        let meta_path = self.session_dir.join(format!("{}.meta.json", self.session_id));
        if let Ok(json) = serde_json::to_string_pretty(&meta) {
            if let Err(e) = std::fs::write(&meta_path, json) {
                tracing::warn!("failed to write session metadata: {e}");
            }
        }
    }

    /// Whether `finalize()` has been called.
    fn is_finalized(&self) -> bool {
        // After finalize(), the .tmp file no longer exists.
        let tmp_path = self
            .session_dir
            .join(format!("{}.ndjson.tmp", self.session_id));
        !tmp_path.exists()
    }
}

impl Drop for SessionLogger {
    fn drop(&mut self) {
        // If finalize() was never called, at least flush the buffer so
        // the .tmp file has all data for post-mortem debugging.
        if !self.is_finalized() {
            if let Err(e) = self.writer.flush() {
                tracing::warn!("SessionLogger dropped without finalize, flush failed: {e}");
            }
        }
    }
}

impl SessionLogger {
    /// Default sessions directory: `~/.local/share/harness/sessions/`.
    pub fn sessions_dir() -> Result<PathBuf> {
        dirs::data_local_dir()
            .map(|d| d.join("harness").join("sessions"))
            .ok_or_else(|| Error::Other("cannot determine data directory".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;

    #[test]
    fn session_meta_round_trip() {
        let meta = SessionMeta {
            session_id: "test-123".into(),
            agent: "Claude Code".into(),
            prompt: "fix the bug".into(),
            model: Some("opus".into()),
            cwd: Some("/tmp".into()),
            start_time: "1700000000".into(),
            duration_ms: Some(5000),
            success: true,
            name: Some("fix auth bug".into()),
            tags: Some(vec!["bug-fix".into(), "auth".into()]),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: SessionMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_id, "test-123");
        assert!(parsed.success);
        assert_eq!(parsed.name, Some("fix auth bug".into()));
        assert_eq!(parsed.tags, Some(vec!["bug-fix".into(), "auth".into()]));
    }

    #[test]
    fn session_meta_backward_compat() {
        // Old metadata without name/tags fields should still deserialize.
        let json = r#"{"session_id":"old","agent":"Claude Code","prompt":"hi","start_time":"0","success":true}"#;
        let parsed: SessionMeta = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.session_id, "old");
        assert!(parsed.name.is_none());
        assert!(parsed.tags.is_none());
    }

    #[test]
    fn logger_creates_files() {
        let tmp = tempfile::tempdir().unwrap();
        let session_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&session_dir).unwrap();

        // Logger writes to .ndjson.tmp, renamed on finalize.
        let tmp_path = session_dir.join("test-session.ndjson.tmp");
        let file = std::fs::File::create(&tmp_path).unwrap();

        let config = TaskConfig::new("test prompt", AgentKind::Claude);
        let mut logger = SessionLogger {
            session_id: "test-session".into(),
            session_dir: session_dir.clone(),
            writer: std::io::BufWriter::new(file),
            config: LoggerConfig {
                agent: config.agent,
                prompt: config.prompt.clone(),
                model: config.model.clone(),
                cwd: None,
                name: None,
            },
            start_secs: 1700000000,
        };

        let event = Event::Message(MessageEvent {
            role: Role::Assistant,
            text: "Hello".into(),
            usage: None,
            timestamp_ms: 123456,
        });
        logger.log_event(&event);
        logger.finalize(true, Some(1000));

        // After finalize, the .tmp should have been renamed to .ndjson.
        let ndjson_path = session_dir.join("test-session.ndjson");
        let content = std::fs::read_to_string(&ndjson_path).unwrap();
        assert!(content.contains("Hello"));
        // The .tmp file should no longer exist.
        assert!(!tmp_path.exists());

        // Verify meta.json was written.
        let meta_path = session_dir.join("test-session.meta.json");
        assert!(meta_path.exists());
        let meta_content = std::fs::read_to_string(&meta_path).unwrap();
        assert!(meta_content.contains("test-session"));
    }
}
