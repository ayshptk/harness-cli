use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::config::TaskConfig;
use crate::error::{Error, Result};
use crate::event::Event;
use crate::runner::{AgentRunner, EventStream};

/// Maximum bytes we'll collect from stderr before truncating.
const MAX_STDERR_BYTES: usize = 64 * 1024;

/// Guard that kills a child process group on drop.
///
/// On Unix, we send SIGTERM to the process group, wait up to 2s, then SIGKILL.
/// The guard is wrapped in `Arc` so dropping the stream kills the child.
pub(crate) struct ChildGuard {
    pid: u32,
    killed: AtomicBool,
}

impl ChildGuard {
    fn new(pid: u32) -> Self {
        Self {
            pid,
            killed: AtomicBool::new(false),
        }
    }

    /// Actively kill the process group (SIGTERM, then SIGKILL after 2s).
    ///
    /// Safe to call multiple times — only the first call sends signals.
    #[cfg(unix)]
    pub(crate) fn kill(&self) {
        // Ensure we only send signals once.
        if self.killed.swap(true, Ordering::SeqCst) {
            return;
        }

        use nix::sys::signal::{killpg, Signal};
        use nix::unistd::Pid;

        let pgid = Pid::from_raw(self.pid as i32);
        if let Err(e) = killpg(pgid, Signal::SIGTERM) {
            tracing::debug!("SIGTERM to pgid {} failed: {e}", self.pid);
            return; // Process already gone, no need for SIGKILL.
        }

        let pid = self.pid;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let pgid = Pid::from_raw(pid as i32);
            if let Err(e) = killpg(pgid, Signal::SIGKILL) {
                tracing::debug!("SIGKILL to pgid {} failed: {e}", pid);
            }
        });
    }

    #[cfg(windows)]
    pub(crate) fn kill(&self) {
        if self.killed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Err(e) = std::process::Command::new("taskkill")
            .args(["/PID", &self.pid.to_string(), "/T", "/F"])
            .output()
        {
            tracing::debug!("taskkill for pid {} failed: {e}", self.pid);
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn kill(&self) {
        if self.killed.swap(true, Ordering::SeqCst) {
            return;
        }
        tracing::warn!("process cleanup not supported on this platform (pid={})", self.pid);
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

/// A handle that bundles an `EventStream` with a `CancellationToken`.
///
/// Cancelling the token gracefully stops the stream and kills the subprocess.
pub struct StreamHandle {
    /// The unified event stream.
    pub stream: EventStream,
    /// Cancel this to stop the agent subprocess.
    pub cancel_token: CancellationToken,
}

/// Spawns an agent subprocess and returns a `StreamHandle` containing an
/// `EventStream` and a `CancellationToken`.
///
/// This is the shared scaffolding used by every adapter — the only thing that
/// differs per-agent is arg construction and line parsing.
///
/// The parser function returns a `Vec` so that a single JSON line can produce
/// multiple events (e.g., an assistant message with both text + tool_use blocks).
///
/// If `cancel_token` is `None`, a new token is created internally.
pub async fn spawn_and_stream<F>(
    runner: &dyn AgentRunner,
    config: &TaskConfig,
    parse_line: F,
    cancel_token: Option<CancellationToken>,
) -> Result<StreamHandle>
where
    F: Fn(&str) -> Vec<Result<Event>> + Send + Sync + 'static,
{
    let binary = runner.binary_path(config)?;
    let args = runner.build_args(config);
    let env_vars = runner.build_env(config);

    let cwd = config
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    validate_cwd(&cwd)?;

    tracing::debug!(
        agent = runner.name(),
        binary = %binary.display(),
        args = ?args,
        cwd = %cwd.display(),
        "spawning agent process"
    );

    let mut cmd = Command::new(&binary);
    cmd.args(&args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // On Unix, create a new process group so we can kill the entire tree.
    #[cfg(unix)]
    cmd.process_group(0);

    for (k, v) in &env_vars {
        cmd.env(k, v);
    }

    // Forward any user-supplied env vars.
    for (k, v) in &config.env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().map_err(Error::SpawnFailed)?;

    let child_pid = child
        .id()
        .ok_or_else(|| Error::Other("failed to get child process ID".into()))?;
    let guard = Arc::new(ChildGuard::new(child_pid));

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Other("failed to capture stdout".into()))?;

    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Other("failed to capture stderr".into()))?;

    // Spawn a task to collect stderr for error reporting (capped at MAX_STDERR_BYTES).
    let stderr_handle = tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut buf = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if buf.len() >= MAX_STDERR_BYTES {
                break;
            }
            if !buf.is_empty() {
                buf.push('\n');
            }
            let remaining = MAX_STDERR_BYTES - buf.len();
            if line.len() > remaining {
                buf.push_str(&line[..remaining]);
                break;
            }
            buf.push_str(&line);
        }
        buf
    });

    // Spawn a task to wait for exit status.
    let wait_handle = tokio::spawn(async move { child.wait().await });

    let mut reader = BufReader::new(stdout).lines();

    // Create or use the provided cancellation token.
    let token = cancel_token.unwrap_or_default();
    let token_for_task = token.clone();

    // Use an mpsc channel so a spawned task can select! between line reads
    // and cancellation — this ensures cancellation is responsive even when
    // the subprocess is blocking (e.g. sleeping).
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event>>(256);

    tokio::spawn(async move {
        // Keep guard alive for the duration of this task.
        let _guard = guard;

        loop {
            tokio::select! {
                _ = token_for_task.cancelled() => {
                    // Cancel requested — kill the subprocess and stop.
                    _guard.kill();
                    break;
                }
                line_result = reader.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            if line.trim().is_empty() {
                                continue;
                            }
                            let events = parse_line(&line);
                            for result in events {
                                let stamped = result.map(|e| e.stamp());
                                if tx.send(stamped).await.is_err() {
                                    return; // receiver dropped
                                }
                            }
                        }
                        Ok(None) => break, // EOF
                        Err(e) => {
                            let _ = tx.send(Err(Error::Io(e))).await;
                            break;
                        }
                    }
                }
            }
        }

        // If we were cancelled, don't bother waiting for exit status.
        if token_for_task.is_cancelled() {
            return;
        }

        // After stdout closes, check exit status.
        match wait_handle.await {
            Ok(Ok(status)) if !status.success() => {
                let stderr_text = stderr_handle.await.unwrap_or_default();
                let code = status.code().unwrap_or(-1);
                let _ = tx
                    .send(Err(Error::ProcessFailed {
                        code,
                        stderr: stderr_text,
                    }))
                    .await;
            }
            Ok(Err(e)) => {
                let _ = tx.send(Err(Error::Io(e))).await;
            }
            Err(e) => {
                let _ = tx
                    .send(Err(Error::Other(format!("join error: {e}"))))
                    .await;
            }
            _ => {} // success — adapter should have emitted Result event
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    Ok(StreamHandle {
        stream: Box::pin(stream),
        cancel_token: token,
    })
}

fn validate_cwd(cwd: &Path) -> Result<()> {
    if !cwd.exists() {
        return Err(Error::InvalidWorkDir(cwd.to_path_buf()));
    }
    if !cwd.is_dir() {
        return Err(Error::Other(format!(
            "working directory is not a directory: {}",
            cwd.display()
        )));
    }
    Ok(())
}
