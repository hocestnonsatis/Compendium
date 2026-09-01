#!/usr/bin/env node
'use strict';

// Fail when Cargo.toml and package.json disagree on the version.

const fs = require('fs');
const path = require('path');

const root = path.resolve(__dirname, '..', '..');
const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const cargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const match = cargo.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
const cargoVersion = match && match[1];

if (!cargoVersion) {
  console.error('check-version: could not read [package].version from Cargo.toml');
  process.exit(1);
}
if (cargoVersion !== pkg.version) {
  console.error(
    `check-version: Cargo.toml ${cargoVersion} != package.json ${pkg.version}`
  );
  process.exit(1);
}
console.log(`check-version: ${pkg.version} ok`);
