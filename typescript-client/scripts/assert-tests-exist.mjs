#!/usr/bin/env node
/**
 * Fail if there are no compiled test files to run.
 *
 * `node --test "dist/**\/*.test.js"` exits 0 when the glob matches nothing, so
 * this package's `test` script reported success while running zero tests — the
 * failure mode a reviewer caught here, and the reason the typescript-integration
 * CI job was green without asserting anything.
 *
 * Tests existing again is not enough on its own: renaming a file, changing the
 * build's output layout or excluding it from tsconfig would all restore the
 * silent pass. This makes that an error instead.
 *
 * `--print` emits the discovered paths so the test script can run exactly
 * these files. One walk, two consumers: the check and the runner cannot
 * disagree about what exists. They did -- this check reported one compiled
 * file and `node --test "dist/**\/*.test.js"` then said it could not find it,
 * because glob support in `--test` varies by Node version and the CI image is
 * node:18-slim, which has none. Reproduced at exit 1 on 18.20.4 and in
 * node:18-slim. The sibling package citadel-workspace-client-ts fixed this
 * exact failure; the fix was never propagated here.
 */

import { readdirSync, statSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const DIST = 'dist';

const PRINT = process.argv.includes('--print');

const files = [];
function collect(dir) {
  if (!existsSync(dir)) return;
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) collect(path);
    else if (entry.endsWith('.test.js')) files.push(path);
  }
}
collect(DIST);

const count = files.length;
if (PRINT && count > 0) {
  process.stdout.write(files.join(' '));
}
if (count === 0) {
  console.error(
    '\n  No compiled *.test.js under dist/.\n' +
      '  `node --test` would exit 0 having run nothing, so this fails instead.\n' +
      '  Check that the sources still exist and that tsconfig emits them.\n'
  );
  process.exit(1);
}
if (!PRINT) console.log(`  ${count} compiled test file(s) found under dist/`);
