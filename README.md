# harness

[![CI](https://github.com/ayshptk/harness-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/ayshptk/harness-cli/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/harnesscli.svg)](https://crates.io/crates/harnesscli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Run any coding agent from a single CLI. Harness spawns the selected agent as a subprocess, translates its native streaming output into a unified NDJSON event stream, and outputs it to stdout.

Write one integration, and it works with any supported agent backend.

## Supported agents

| Agent | Binary | Status |
|-------|--------|--------|
| [Claude Code](https://docs.anthropic.com/en/docs/claude-code) | `claude` | Supported |
| [OpenAI Codex](https://github.com/openai/codex) | `codex` | Supported |
| [OpenCode](https://github.com/opencode-ai/opencode) | `opencode` | Supported |
| [Cursor](https://cursor.com) | `agent` | Supported |

## Install

**Recommended — curl installer:**

```bash
curl -fsSL https://harness.lol/install.sh | sh
```

**Via crates.io:**

```bash
cargo install harnesscli
```

**Build from source:**

```bash
git clone https://github.com/ayshptk/harness-cli.git
cd harness
cargo build --release
# Binary at target/release/harness
```

## Quick start

```bash
# Run Claude Code with a prompt
harness run --agent claude --prompt "explain this codebase"

# Use a model alias
harness run --agent claude --model sonnet --prompt "fix the bug"

# Dry-run — see the resolved command without executing
harness run --agent claude --model sonnet --prompt "hello" --dry-run

# Pipe prompt from stdin
echo "explain this codebase" | harness run --agent claude

# List available agents
harness list

# Check if an agent is installed
harness check claude --capabilities
```

## Model registry

Harness maps human-friendly model names to the exact IDs each agent expects:

```bash
# List all known models
harness models list

# Resolve an alias for a specific agent
harness models resolve opus --agent claude
# → claude-opus-4-6

# Update the registry cache
harness models update
```

Built-in aliases include `opus`. You can add your own in `harness.toml`, and the cached registry at `~/.harness/models.toml` is auto-updated from GitHub.

## Configuration

Place a `harness.toml` in your project root (or any parent directory):

```toml
default_agent = "claude"
default_model = "sonnet"
default_permissions = "full-access"
default_timeout_secs = 300

[agents.claude]
model = "opus"
extra_args = ["--verbose"]

[models.my-model]
description = "My custom model"
provider = "anthropic"
claude = "my-custom-model-id"
```

## Unified event stream

Every agent's output is translated into a common NDJSON format with 8 event types:

- `SessionStart` — session initialized
- `TextDelta` — streaming text chunk
- `Message` — complete message
- `ToolStart` — tool invocation beginning
- `ToolEnd` — tool invocation complete
- `UsageDelta` — incremental token usage and cost update
- `Result` — run finished
- `Error` — error occurred

## Documentation

Full docs at **[harness.lol](https://harness.lol)**

## License

MIT
