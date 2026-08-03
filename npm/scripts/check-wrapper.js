#!/usr/bin/env node
'use strict';

/** Lightweight prepack sanity check for the npm wrapper. */
const fs = require('fs');
const path = require('path');

const root = path.resolve(__dirname, '../..');
const runJs = path.join(root, 'bin', 'run.js');
if (!fs.existsSync(runJs)) {
  console.error('Missing bin/run.js — cannot pack npm wrapper.');
  process.exit(1);
}
const pkg = require(path.join(root, 'package.json'));
if (!pkg.bin || !pkg.bin.compendium) {
  console.error('package.json missing bin.compendium mapping.');
  process.exit(1);
}
console.log(`compendium-mcp@${pkg.version} wrapper OK`);
