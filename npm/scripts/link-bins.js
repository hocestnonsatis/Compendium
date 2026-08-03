#!/usr/bin/env node
'use strict';

/**
 * Link this package's bins into ./node_modules/.bin when developing from the
 * repo root. Skips when the package is installed as a dependency (npm already
 * links bins correctly in that case).
 *
 * Without these links, `npx -y compendium-mcp` from the repo root fails with
 * `compendium: command not found` because npm resolves the local package but
 * does not create the shim.
 */

const fs = require('fs');
const path = require('path');

const root = path.resolve(__dirname, '..');
const initCwd = process.env.INIT_CWD ? path.resolve(process.env.INIT_CWD) : null;

// Dependency install: prepare runs inside node_modules/<pkg>; leave alone.
if (initCwd && initCwd !== root) {
  process.exit(0);
}
if (root.split(path.sep).includes('node_modules')) {
  process.exit(0);
}

const binDir = path.join(root, 'node_modules', '.bin');
const target = path.join(root, 'bin', 'run.js');
if (!fs.existsSync(target)) {
  process.exit(0);
}

fs.mkdirSync(binDir, { recursive: true });
for (const name of ['compendium', 'compendium-mcp']) {
  const link = path.join(binDir, name);
  const rel = path.relative(binDir, target);
  try {
    fs.rmSync(link, { force: true });
  } catch {
    // ignore
  }
  fs.symlinkSync(rel, link);
}
