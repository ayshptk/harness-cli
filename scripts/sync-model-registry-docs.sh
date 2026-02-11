#!/usr/bin/env bash
set -euo pipefail

# Regenerates the "Built-in aliases" table in model-registry.mdx from models.toml.
#
# Usage: ./scripts/sync-model-registry-docs.sh
#
# Requires: cargo (builds+runs harness to get authoritative JSON output)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$SCRIPT_DIR/.."
MODELS_TOML="$ROOT/models.toml"
MDX_FILE="$ROOT/docs/content/docs/configuration/model-registry.mdx"

if [[ ! -f "$MODELS_TOML" ]]; then
  echo "error: models.toml not found at $MODELS_TOML" >&2
  exit 1
fi

if [[ ! -f "$MDX_FILE" ]]; then
  echo "error: model-registry.mdx not found at $MDX_FILE" >&2
  exit 1
fi

# Build harness and get the JSON model list.
echo "Building harness..." >&2
cargo build --manifest-path "$ROOT/Cargo.toml" --quiet

HARNESS="$ROOT/target/debug/harness"

echo "Reading model registry..." >&2
MODELS_JSON=$("$HARNESS" models list --json 2>/dev/null)

# Parse JSON and build the markdown table.
# Shape: [{ "name": "opus", "description": "...", "provider": "...",
#           "claude": "id", "opencode": "id", ... }]
TABLE=$(python3 -c "
import json, sys

data = json.loads(sys.stdin.read())
data.sort(key=lambda m: m['name'])

agents = ['claude', 'codex', 'opencode', 'cursor']
header = '| Alias | Description | Claude | Codex | OpenCode | Cursor |'
sep    = '|-------|-------------|--------|-------|----------|--------|'

rows = []
for m in data:
    name = m['name']
    desc = m.get('description', '')
    cells = []
    for a in agents:
        model_id = m.get(a)
        cells.append(f'\`{model_id}\`' if model_id else '—')
    row = f'| \`{name}\` | {desc} | {\" | \".join(cells)} |'
    rows.append(row)

print(header)
print(sep)
for r in rows:
    print(r)
" <<< "$MODELS_JSON")

if [[ -z "$TABLE" ]]; then
  echo "error: failed to generate table from models JSON" >&2
  exit 1
fi

# Also regenerate the "Registry format" example block from models.toml.
# Take the first two entries (or all if fewer) as examples.
EXAMPLE_TOML=$(python3 -c "
import json, sys

data = json.loads(sys.stdin.read())
data.sort(key=lambda m: m['name'])

agents = ['claude', 'codex', 'opencode', 'cursor']
blocks = []
for m in data[:2]:
    lines = [f'[models.{m[\"name\"]}]']
    lines.append(f'description = \"{m.get(\"description\", \"\")}\"')
    lines.append(f'provider = \"{m.get(\"provider\", \"\")}\"')
    for a in agents:
        if a in m:
            lines.append(f'{a} = \"{m[a]}\"')
    blocks.append('\n'.join(lines))

print('\n\n'.join(blocks))
" <<< "$MODELS_JSON")

# Replace the table between "## Built-in aliases" and the next "##" heading.
# Use python for reliable multiline replacement.
python3 -c "
import sys

mdx = open(sys.argv[1]).read()
table = sys.argv[2]
example = sys.argv[3]

# Replace the aliases table.
import re

# Match from the table header to the line before the next ## heading.
pattern = r'(\| Alias \|.*?\n(?:\|.*\n)*)'
mdx = re.sub(pattern, table + '\n', mdx)

# Replace the registry format code block.
fence_pattern = r'(## Registry format\n\n.*?\n\`\`\`toml\n)(.*?)(\`\`\`)'
mdx = re.sub(fence_pattern, r'\g<1>' + example + '\n' + r'\g<3>', mdx, flags=re.DOTALL)

open(sys.argv[1], 'w').write(mdx)
" "$MDX_FILE" "$TABLE" "$EXAMPLE_TOML"

echo "Updated $MDX_FILE" >&2

# Show a summary of what's in the table.
MODEL_COUNT=$(echo "$MODELS_JSON" | python3 -c "import json,sys; print(len(json.loads(sys.stdin.read())))")
echo "  $MODEL_COUNT model(s) in registry" >&2
