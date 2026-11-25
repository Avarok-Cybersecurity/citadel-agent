#!/usr/bin/env node
/**
 * The websocket-server image copies the workspace one directory at a time, and cargo refuses to
 * load a workspace whose member is missing ("failed to load manifest for workspace member").
 * A new crate added to `members` but not to the Dockerfile fails only in the integration job,
 * after a full image build. This fails in a second, naming the directory.
 *
 * Run from the repository root: `node .github/scripts/check-docker-copies-every-member.mjs`.
 */
import { readFileSync } from 'node:fs';

export function members(cargoToml) {
  const block = cargoToml.match(/^members\s*=\s*\[([\s\S]*?)\]/m);
  if (!block) throw new Error('no [workspace] members list in Cargo.toml');
  return [...block[1].matchAll(/"([^"]+)"/g)].map((m) => m[1].replace(/^\.\//, '').split('/')[0]);
}

export function copied(dockerfile) {
  return new Set(
    dockerfile
      .split('\n')
      .map((l) => l.match(/^COPY\s+(?!--from)(\S+)\s+\S+\s*$/))
      .filter(Boolean)
      .map((m) => m[1].replace(/^\.\//, '').replace(/\/$/, '').split('/')[0]),
  );
}

const dockerfilePath = process.argv[2] ?? 'ci/Dockerfile.websocket-server';
const want = [...new Set(members(readFileSync('Cargo.toml', 'utf8')))];
const have = copied(readFileSync(dockerfilePath, 'utf8'));
const missing = want.filter((m) => !have.has(m));
if (want.length === 0) {
  console.error('::error::no workspace members found; this check judged nothing');
  process.exit(1);
}
if (missing.length) {
  for (const m of missing) console.error(`::error file=${dockerfilePath}::workspace member directory '${m}' is not copied into the image`);
  process.exit(1);
}
console.log(`${dockerfilePath} copies all ${want.length} workspace member directories`);
