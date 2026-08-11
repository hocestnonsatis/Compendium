#!/usr/bin/env node
'use strict';

/**
 * Fixture tests for release.yml parsers in check-versions.js.
 * Guards the publish-loop regex regression (`for platform in …; do`
 * previously failed to match and returned []).
 * Also locks residual_oidc stale + uncovered coverage policy
 * (evaluateResidualProbe / evaluateResidualCoverage), the
 * GitHub Releases fallback gate for still-missing residuals
 * (evaluateResidualReleaseAssets), optionalDeps version presence for
 * non-residual packages once tag `v${version}` exists
 * (evaluatePublishedVersionPresence), and main wrapper
 * `compendium-mcp@${version}` for the same tag (`npx`).
 */
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const {
  releaseMatrixSpecs,
  releaseMatrixPlatforms,
  releasePublishLoopPlatforms,
  residualOidcPlatforms,
  evaluateResidualProbe,
  evaluateResidualCoverage,
  evaluateResidualReleaseAssets,
  evaluatePublishedVersionPresence,
  runCheck,
} = require('./check-versions.js');

let failed = 0;

function check(name, fn) {
  try {
    fn();
    console.log(`  ok  ${name}`);
  } catch (err) {
    failed += 1;
    console.error(`  FAIL ${name}`);
    console.error(`       ${err.message}`);
  }
}

const sampleYml = `
jobs:
  build:
    strategy:
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            platform: linux-x64
            asset: compendium-linux-x64.tar.gz
          - target: aarch64-pc-windows-msvc
            platform: win32-arm64
            asset: compendium-win32-arm64.zip
          - target: x86_64-unknown-linux-musl
            platform: linux-x64-musl
            asset: compendium-linux-x64-musl.tar.gz
  publish:
    steps:
      - name: Publish platform packages (OIDC)
        run: |
          residual_oidc="linux-x64-musl win32-arm64"
          for platform in linux-x64 linux-x64-musl win32-arm64; do
            echo "$platform"
          done
`;

check('publish loop matches `for platform in …; do`', () => {
  const keys = releasePublishLoopPlatforms(sampleYml);
  assert.deepStrictEqual(keys, [
    'linux-x64',
    'linux-x64-musl',
    'win32-arm64',
  ]);
});

check('publish loop returns [] when loop missing', () => {
  assert.deepStrictEqual(releasePublishLoopPlatforms('echo hi\n'), []);
});

check('publish loop single platform', () => {
  const yml = 'for platform in linux-x64; do\n  true\ndone\n';
  assert.deepStrictEqual(releasePublishLoopPlatforms(yml), ['linux-x64']);
});

check('matrix specs parse target/platform/asset', () => {
  const specs = releaseMatrixSpecs(sampleYml);
  assert.strictEqual(specs.size, 3);
  assert.deepStrictEqual(specs.get('linux-x64'), {
    target: 'x86_64-unknown-linux-gnu',
    asset: 'compendium-linux-x64.tar.gz',
  });
  assert.deepStrictEqual(specs.get('win32-arm64'), {
    target: 'aarch64-pc-windows-msvc',
    asset: 'compendium-win32-arm64.zip',
  });
  assert.deepStrictEqual(specs.get('linux-x64-musl'), {
    target: 'x86_64-unknown-linux-musl',
    asset: 'compendium-linux-x64-musl.tar.gz',
  });
});

check('matrix platforms sorted keys', () => {
  assert.deepStrictEqual(releaseMatrixPlatforms(sampleYml), [
    'linux-x64',
    'linux-x64-musl',
    'win32-arm64',
  ]);
});

check('residual_oidc parses allowlist', () => {
  assert.deepStrictEqual(residualOidcPlatforms(sampleYml), [
    'linux-x64-musl',
    'win32-arm64',
  ]);
});

check('residual_oidc empty / missing → []', () => {
  assert.deepStrictEqual(residualOidcPlatforms('residual_oidc=""\n'), []);
  assert.deepStrictEqual(residualOidcPlatforms('no allowlist here\n'), []);
});

check('evaluateResidualProbe flags stale exists', () => {
  const out = evaluateResidualProbe([
    { key: 'linux-x64-musl', status: 'exists' },
    { key: 'win32-arm64', status: 'missing' },
  ]);
  assert.deepStrictEqual(out.stale, ['linux-x64-musl']);
  assert.deepStrictEqual(out.stillMissing, ['win32-arm64']);
  assert.deepStrictEqual(out.errors, []);
});

check('evaluateResidualProbe collects probe errors', () => {
  const out = evaluateResidualProbe([
    { key: 'linux-x64-musl', status: 'error', detail: 'timeout' },
    { key: 'win32-arm64', status: 'missing' },
  ]);
  assert.deepStrictEqual(out.stale, []);
  assert.deepStrictEqual(out.stillMissing, ['win32-arm64']);
  assert.deepStrictEqual(out.errors, ['linux-x64-musl: timeout']);
});

check('evaluateResidualCoverage flags uncovered missing platforms', () => {
  const out = evaluateResidualCoverage(
    [
      { key: 'linux-x64', status: 'exists' },
      { key: 'linux-x64-musl', status: 'missing' },
      { key: 'win32-arm64', status: 'missing' },
    ],
    ['win32-arm64']
  );
  assert.deepStrictEqual(out.stale, []);
  assert.deepStrictEqual(out.stillMissing, ['win32-arm64']);
  assert.deepStrictEqual(out.uncovered, ['linux-x64-musl']);
  assert.deepStrictEqual(out.errors, []);
});

check('evaluateResidualCoverage flags stale while covering residuals', () => {
  const out = evaluateResidualCoverage(
    [
      { key: 'linux-x64-musl', status: 'exists' },
      { key: 'win32-arm64', status: 'missing' },
    ],
    ['linux-x64-musl', 'win32-arm64']
  );
  assert.deepStrictEqual(out.stale, ['linux-x64-musl']);
  assert.deepStrictEqual(out.stillMissing, ['win32-arm64']);
  assert.deepStrictEqual(out.uncovered, []);
});

check('evaluateResidualReleaseAssets flags missing assets', () => {
  const out = evaluateResidualReleaseAssets([
    {
      key: 'linux-x64-musl',
      asset: 'compendium-linux-x64-musl',
      status: 'exists',
    },
    {
      key: 'win32-arm64',
      asset: 'compendium-win32-arm64.exe',
      status: 'missing',
    },
  ]);
  assert.deepStrictEqual(out.ok, ['linux-x64-musl']);
  assert.deepStrictEqual(out.missingAssets, ['win32-arm64']);
  assert.deepStrictEqual(out.errors, []);
});

check('evaluateResidualReleaseAssets collects probe errors', () => {
  const out = evaluateResidualReleaseAssets([
    {
      key: 'linux-x64-musl',
      asset: 'compendium-linux-x64-musl',
      status: 'error',
      detail: 'timeout',
    },
  ]);
  assert.deepStrictEqual(out.ok, []);
  assert.deepStrictEqual(out.missingAssets, []);
  assert.deepStrictEqual(out.errors, [
    'linux-x64-musl (compendium-linux-x64-musl): timeout',
  ]);
});

check('evaluatePublishedVersionPresence flags absent versions', () => {
  const out = evaluatePublishedVersionPresence([
    { key: 'linux-x64', status: 'exists' },
    {
      key: 'darwin-arm64',
      status: 'missing',
      detail: 'version 0.6.0 not on registry',
    },
    { key: 'linux-arm64', status: 'error', detail: 'timeout' },
  ]);
  assert.deepStrictEqual(out.ok, ['linux-x64']);
  assert.deepStrictEqual(out.versionAbsent, ['darwin-arm64']);
  assert.deepStrictEqual(out.errors, ['linux-arm64: timeout']);
});

check('live release.yml publish loop is non-empty', () => {
  const yml = fs.readFileSync(
    path.join(__dirname, '../../.github/workflows/release.yml'),
    'utf8'
  );
  const keys = releasePublishLoopPlatforms(yml);
  assert.ok(keys.length >= 2, `expected ≥2 publish platforms, got ${keys.length}`);
  assert.ok(
    keys.includes('linux-x64-musl') && keys.includes('win32-arm64'),
    `residual platforms must stay in publish loop: ${keys.join(', ')}`
  );
});

check('live release.yml residual_oidc ⊆ matrix platforms', () => {
  const yml = fs.readFileSync(
    path.join(__dirname, '../../.github/workflows/release.yml'),
    'utf8'
  );
  const residual = residualOidcPlatforms(yml);
  const matrix = new Set(releaseMatrixPlatforms(yml));
  for (const key of residual) {
    assert.ok(matrix.has(key), `residual_oidc ${key} missing from matrix`);
  }
});

check('live release.yml matrix assets/targets match PLATFORMS', () => {
  const { PLATFORMS } = require('../lib/platform.js');
  const yml = fs.readFileSync(
    path.join(__dirname, '../../.github/workflows/release.yml'),
    'utf8'
  );
  const specs = releaseMatrixSpecs(yml);
  assert.ok(specs.size >= 2, 'expected live matrix specs');
  for (const [key, matrix] of specs) {
    const plat = PLATFORMS[key];
    assert.ok(plat, `PLATFORMS missing ${key}`);
    assert.strictEqual(
      matrix.asset,
      plat.asset,
      `${key} asset ${matrix.asset} != PLATFORMS ${plat.asset}`
    );
    assert.strictEqual(
      matrix.target,
      plat.rustTarget,
      `${key} target ${matrix.target} != PLATFORMS ${plat.rustTarget}`
    );
  }
});

check('live tree runCheck() passes', () => {
  // Alignment gate must stay green; catch process.exit from runCheck.
  const exit = process.exit;
  let code = 0;
  process.exit = (c) => {
    code = c ?? 0;
    throw new Error(`runCheck exited ${code}`);
  };
  try {
    runCheck();
    assert.strictEqual(code, 0);
  } finally {
    process.exit = exit;
  }
});

async function runAsyncChecks() {
  const { runResidualNpmCheck } = require('./check-versions.js');

  const platformsStub = {
    'linux-x64': { asset: 'compendium-linux-x64' },
    'linux-x64-musl': { asset: 'compendium-linux-x64-musl' },
    'win32-arm64': { asset: 'compendium-win32-arm64.exe' },
  };

  // Real async assertions (check() is sync; drive manually).
  {
    const name = 'runResidualNpmCheck fails when residual package exists';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        fetchStatus: async (pkg) =>
          pkg.includes('win32-arm64')
            ? { status: 'exists' }
            : { status: 'missing' },
      });
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error('       expected process.exit(1) for stale residual');
    } catch (err) {
      if (exited === 1) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       ${err.message}`);
      }
    } finally {
      process.exit = exit;
    }
  }

  {
    const name = 'runResidualNpmCheck soft-skips probe errors';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        fetchStatus: async () => ({ status: 'error', detail: 'offline' }),
      });
      if (exited === null) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       unexpected exit ${exited}`);
      }
    } catch (err) {
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(`       ${err.message}`);
    } finally {
      process.exit = exit;
    }
  }

  {
    const name =
      'runResidualNpmCheck fails when missing package not in residual_oidc';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        residualKeys: ['win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        fetchStatus: async (pkg) =>
          pkg.includes('linux-x64-musl') || pkg.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
      });
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error('       expected process.exit(1) for uncovered residual');
    } catch (err) {
      if (exited === 1) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       ${err.message}`);
      }
    } finally {
      process.exit = exit;
    }
  }

  {
    const name =
      'runResidualNpmCheck passes when residuals cover all missing packages';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        residualKeys: ['linux-x64-musl', 'win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        fetchStatus: async (pkg) =>
          pkg.includes('linux-x64-musl') || pkg.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
      });
      if (exited === null) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       unexpected exit ${exited}`);
      }
    } catch (err) {
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(`       ${err.message}`);
    } finally {
      process.exit = exit;
    }
  }

  {
    const name =
      'runResidualNpmCheck fails when residual Release asset is missing';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        residualKeys: ['linux-x64-musl', 'win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        version: '0.6.0',
        platforms: platformsStub,
        fetchStatus: async (pkg) =>
          pkg.includes('linux-x64-musl') || pkg.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
        fetchReleaseTag: async () => ({ status: 'exists' }),
        fetchReleaseAsset: async (_repo, _tag, asset) =>
          asset.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
      });
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error('       expected process.exit(1) for missing release asset');
    } catch (err) {
      if (exited === 1) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       ${err.message}`);
      }
    } finally {
      process.exit = exit;
    }
  }

  {
    const name =
      'runResidualNpmCheck soft-skips when Release tag is unpublished';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        residualKeys: ['linux-x64-musl', 'win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        version: '9.9.9',
        platforms: platformsStub,
        fetchStatus: async (pkg) =>
          pkg.includes('linux-x64-musl') || pkg.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
        fetchReleaseTag: async () => ({ status: 'missing' }),
        fetchReleaseAsset: async () => {
          throw new Error('should not probe assets when tag missing');
        },
      });
      if (exited === null) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       unexpected exit ${exited}`);
      }
    } catch (err) {
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(`       ${err.message}`);
    } finally {
      process.exit = exit;
    }
  }

  {
    const name =
      'runResidualNpmCheck passes when residual Release assets exist';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        residualKeys: ['linux-x64-musl', 'win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        version: '0.6.0',
        platforms: platformsStub,
        fetchStatus: async (pkg) =>
          pkg.includes('linux-x64-musl') || pkg.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
        fetchReleaseTag: async () => ({ status: 'exists' }),
        fetchReleaseAsset: async () => ({ status: 'exists' }),
      });
      if (exited === null) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       unexpected exit ${exited}`);
      }
    } catch (err) {
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(`       ${err.message}`);
    } finally {
      process.exit = exit;
    }
  }

  {
    const name =
      'runResidualNpmCheck soft-skips version gate when COMPENDIUM_SKIP_PUBLISHED_VERSION_GATE=1';
    const exit = process.exit;
    const prev = process.env.COMPENDIUM_SKIP_PUBLISHED_VERSION_GATE;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    process.env.COMPENDIUM_SKIP_PUBLISHED_VERSION_GATE = '1';
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        residualKeys: [],
        publishKeys: ['linux-x64'],
        version: '0.6.1',
        fetchStatus: async () => ({ status: 'exists' }),
        fetchVersionStatus: async () => ({ status: 'missing' }),
        fetchReleaseTag: async () => ({ status: 'exists' }),
      });
      if (exited === null) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       unexpected exit ${exited}`);
      }
    } catch (err) {
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(`       ${err.message}`);
    } finally {
      process.exit = exit;
      if (prev === undefined) {
        delete process.env.COMPENDIUM_SKIP_PUBLISHED_VERSION_GATE;
      } else {
        process.env.COMPENDIUM_SKIP_PUBLISHED_VERSION_GATE = prev;
      }
    }
  }

  {
    const name =
      'runResidualNpmCheck fails when optionalDeps version missing after tag';
    // Ensure release pre-publish env cannot leak into this assertion.
    delete process.env.COMPENDIUM_SKIP_PUBLISHED_VERSION_GATE;
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        residualKeys: ['linux-x64-musl', 'win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        version: '0.6.0',
        fetchStatus: async (pkg) =>
          pkg.includes('linux-x64-musl') || pkg.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
        fetchVersionStatus: async () => ({
          status: 'missing',
          detail: 'version 0.6.0 not on registry',
        }),
        fetchReleaseTag: async () => ({ status: 'exists' }),
      });
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(
        '       expected process.exit(1) for missing optionalDeps version'
      );
    } catch (err) {
      if (exited === 1) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       ${err.message}`);
      }
    } finally {
      process.exit = exit;
    }
  }

  {
    const name =
      'runResidualNpmCheck soft-skips optionalDeps version gap when tag unpublished';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        residualKeys: ['linux-x64-musl', 'win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        version: '9.9.9',
        fetchStatus: async (pkg) =>
          pkg.includes('linux-x64-musl') || pkg.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
        fetchVersionStatus: async () => ({
          status: 'missing',
          detail: 'version 9.9.9 not on registry',
        }),
        fetchReleaseTag: async () => ({ status: 'missing' }),
      });
      if (exited === null) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       unexpected exit ${exited}`);
      }
    } catch (err) {
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(`       ${err.message}`);
    } finally {
      process.exit = exit;
    }
  }

  {
    const name = 'runResidualNpmCheck passes when optionalDeps versions exist';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        residualKeys: ['linux-x64-musl', 'win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        version: '0.6.0',
        fetchStatus: async (pkg) =>
          pkg.includes('linux-x64-musl') || pkg.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
        fetchVersionStatus: async () => ({ status: 'exists' }),
        fetchReleaseTag: async () => ({ status: 'exists' }),
      });
      if (exited === null) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       unexpected exit ${exited}`);
      }
    } catch (err) {
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(`       ${err.message}`);
    } finally {
      process.exit = exit;
    }
  }

  {
    const name =
      'runResidualNpmCheck fails when main wrapper version missing after tag';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        residualKeys: ['linux-x64-musl', 'win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        version: '0.6.0',
        fetchStatus: async (pkg) =>
          pkg.includes('linux-x64-musl') || pkg.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
        fetchVersionStatus: async (pkg) =>
          pkg === 'compendium-mcp'
            ? { status: 'missing', detail: 'version 0.6.0 not on registry' }
            : { status: 'exists' },
        fetchReleaseTag: async () => ({ status: 'exists' }),
      });
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(
        '       expected process.exit(1) for missing main wrapper version'
      );
    } catch (err) {
      if (exited === 1) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       ${err.message}`);
      }
    } finally {
      process.exit = exit;
    }
  }

  {
    const name =
      'runResidualNpmCheck soft-skips main wrapper version gap when tag unpublished';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        residualKeys: ['linux-x64-musl', 'win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        version: '9.9.9',
        fetchStatus: async (pkg) =>
          pkg.includes('linux-x64-musl') || pkg.includes('win32-arm64')
            ? { status: 'missing' }
            : { status: 'exists' },
        fetchVersionStatus: async (pkg) =>
          pkg === 'compendium-mcp'
            ? { status: 'missing', detail: 'version 9.9.9 not on registry' }
            : { status: 'exists' },
        fetchReleaseTag: async () => ({ status: 'missing' }),
      });
      if (exited === null) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       unexpected exit ${exited}`);
      }
    } catch (err) {
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(`       ${err.message}`);
    } finally {
      process.exit = exit;
    }
  }

  {
    const name =
      'runResidualNpmCheck passes when main wrapper version exists (no optionalDeps published)';
    const exit = process.exit;
    let exited = null;
    process.exit = (c) => {
      exited = c ?? 0;
      throw new Error(`exit ${exited}`);
    };
    try {
      await runResidualNpmCheck({
        skipReleaseAssets: true,
        residualKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        publishKeys: ['linux-x64', 'linux-x64-musl', 'win32-arm64'],
        version: '0.6.0',
        fetchStatus: async () => ({ status: 'missing' }),
        fetchVersionStatus: async (pkg) =>
          pkg === 'compendium-mcp'
            ? { status: 'exists' }
            : { status: 'missing' },
        fetchReleaseTag: async () => ({ status: 'exists' }),
      });
      if (exited === null) {
        console.log(`  ok  ${name}`);
      } else {
        failed += 1;
        console.error(`  FAIL ${name}`);
        console.error(`       unexpected exit ${exited}`);
      }
    } catch (err) {
      failed += 1;
      console.error(`  FAIL ${name}`);
      console.error(`       ${err.message}`);
    } finally {
      process.exit = exit;
    }
  }

  if (failed) {
    console.error(`check-versions selftest: ${failed} failure(s)`);
    process.exit(1);
  }
  console.log('check-versions selftest OK');
}

runAsyncChecks().catch((err) => {
  console.error(err);
  process.exit(1);
});
