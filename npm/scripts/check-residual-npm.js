#!/usr/bin/env node
'use strict';

/**
 * Probe npm (+ GitHub Releases) for residual OIDC soft-fail coverage.
 * Fails when: (1) a residual_oidc package already exists (stale allowlist),
 * (2) a publish-loop package is still missing but not listed in residual_oidc
 * (Release would hard-fail), (3) a non-residual package lacks versions[version]
 * after tag v${version} exists (optionalDeps pin broken), (4) main wrapper
 * compendium-mcp lacks versions[version] after that tag (npx broken), or
 * (5) a still-missing residual lacks its GitHub Release asset for v${version}
 * (wrapper fallback broken). Network / unpublished-tag errors soft-skip with
 * a warning.
 */
const { runResidualNpmCheck } = require('./check-versions.js');

runResidualNpmCheck().catch((err) => {
  console.error(err);
  process.exit(1);
});
