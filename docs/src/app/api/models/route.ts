import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { parse } from 'smol-toml';

interface ModelEntry {
  description?: string;
  provider?: string;
  claude?: string;
  codex?: string;
  opencode?: string;
  cursor?: string;
}

interface ModelsToml {
  models: Record<string, ModelEntry>;
}

function loadRegistry() {
  const tomlPath = resolve(process.cwd(), '../models.toml');
  const content = readFileSync(tomlPath, 'utf-8');
  const parsed = parse(content) as unknown as ModelsToml;

  return Object.entries(parsed.models ?? {})
    .map(([name, entry]) => ({
      name,
      description: entry.description ?? '',
      provider: entry.provider ?? '',
      ...(entry.claude && { claude: entry.claude }),
      ...(entry.codex && { codex: entry.codex }),
      ...(entry.opencode && { opencode: entry.opencode }),
      ...(entry.cursor && { cursor: entry.cursor }),
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

export function GET() {
  const models = loadRegistry();
  return Response.json(models);
}
