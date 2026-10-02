#!/usr/bin/env node
/**
 * No fork's pull request can run on the self-hosted Mac runners.
 *
 * This repository is public and the Mac (`apple-48gb-metal`) keeps its state
 * between jobs. A job may name that pool only through the one runs-on
 * expression in mac-runner-routing.mjs beside this file: opt-in variable,
 * same-repo guard, pool array, hosted fallback. Anything else that names it
 * fails here, before a fork tries it.
 *
 * Run from the repository root: `node .github/scripts/check-mac-runner-is-fork-safe.mjs`.
 */
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { judge } from './mac-runner-routing.mjs';

const DIR = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'workflows');
const workflows = readdirSync(DIR)
  .filter((f) => /\.ya?ml$/.test(f))
  .map((name) => ({ name, source: readFileSync(join(DIR, name), 'utf8') }));
const { problems, jobs, routed } = judge(workflows);
if (jobs === 0) problems.push('no jobs found in any workflow; the parser no longer matches the files');

if (problems.length) {
  console.error('\n  A job could put untrusted code on the self-hosted Mac:\n');
  for (const p of problems) console.error(`::error::.github/workflows/${p}`);
  console.error('\n  Route a job there only with the expression in .github/scripts/mac-runner-routing.mjs.\n');
  process.exit(1);
}
console.log(`  Mac runner fork guard: ${jobs} job(s) in ${workflows.length} workflow(s), ${routed.length} routed to the Mac, every one guarded  ok`);
