#!/usr/bin/env node
/**
 * Keep internal tracking identifiers out of public history.
 *
 * Reads the message of every commit the pull request adds (merge-base..HEAD, so commits
 * already on the base branch are never judged) and the pull request's title and body, passed
 * in through PR_TITLE and PR_BODY, and fails naming each line that carries one.
 *
 * Run from the repository root: `node .github/scripts/check-no-tracking-ids.mjs <base ref>`.
 */
import { execFileSync } from 'node:child_process';
import { findings, parseLog } from './tracking-ids.mjs';

const base = process.argv[2];
if (!base) {
  console.error('usage: check-no-tracking-ids.mjs <base ref>');
  process.exit(2);
}
const git = (...args) => execFileSync('git', args, { encoding: 'utf8' });
const mergeBase = git('merge-base', 'HEAD', base).trim();
const sources = parseLog(git('log', '--format=%H%x00%B%x1e', `${mergeBase}..HEAD`));
for (const name of ['TITLE', 'BODY']) {
  const text = process.env[`PR_${name}`];
  if (text !== undefined) sources.push({ where: `pull request ${name.toLowerCase()}`, text });
}

const found = findings(sources);
if (found.length) {
  for (const f of found) console.error(`::error::${f}`);
  console.error('\n  Keep internal tracking identifiers out of public history: reword the commit message (or the pull request text) without them.\n');
  process.exit(1);
}
console.log(`no tracking identifier in ${sources.length} commit message(s) and pull request text(s)`);
