#!/usr/bin/env node
'use strict';

/**
 * Fail when Cargo.toml, root package.json, optionalDependencies,
 * npm/platforms/<platform>/package.json, npm/lib/platform.js PLATFORMS,
 * and release.yml matrix / publish loop / residual_oidc soft-fail list drift apart.
 *
 * Also gates release asset names + rustc targets against PLATFORMS so the
 * GitHub Releases download fallback (bin/run.js) cannot silently break —
 * critical for residual_oidc platforms that are not yet on the npm registry.
 *
 * Matrix include keys and the publish `for platform in …` list are checked
 * independently (not unioned): omitting a platform from only the publish loop
 * used to slip past a union-based gate.
 *
 * `runResidualNpmCheck` (see `npm run check-residual-npm`) probes the registry:
 * (1) soft-fail platforms that already exist on npm fail CI until removed from
 * `residual_oidc` — soft-fail must not linger after Trusted Publisher lands;
 * (2) publish-loop platforms that are still missing on npm but absent from
 * `residual_oidc` also fail — otherwise the next Release hard-fails OIDC;
 * (3) non-residual platforms whose package exists but lack `versions[version]`
 * fail when tag `v${version}` is published (optionalDeps pin would break);
 * soft-skip that version gate when the tag is unpublished;
 * (4) main wrapper `compendium-mcp` must also have `versions[version]` once
 * that tag exists (`npx -y compendium-mcp`); soft-skip when the tag is
 * unpublished;
 * (5) still-missing residual platforms must have a GitHub Release asset for
 * `v${version}` (bin/run.js fallback) — fail if the tag exists but the asset
 * is absent; soft-skip when the tag is not published yet or GitHub is unreachable.
 *
 * Parsers / probe helpers are exported for `check-versions-selftest.js`.
 */
const fs = require('fs');
const https = require('https');
const path = require('path');

const root = path.resolve(__dirname, '../..');
const releaseYmlPath = path.join(root, '.github', 'workflows', 'release.yml');

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function cargoVersion() {
  const toml = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
  const m = toml.match(/^version\s*=\s*"([^"]+)"/m);
  if (!m) {
    console.error('Could not parse version from Cargo.toml');
    process.exit(1);
  }
  return m[1];
}

/** Keys declared in npm/lib/platform.js `PLATFORMS`. */
function platformJsKeys() {
  // Require the module so we stay in sync with runtime mapping (not a second copy).
  const { PLATFORMS } = require(path.join(root, 'npm', 'lib', 'platform.js'));
  return { keys: Object.keys(PLATFORMS).sort(), PLATFORMS };
}

function readReleaseYml() {
  return fs.readFileSync(releaseYmlPath, 'utf8');
}

/**
 * Parse build-matrix include entries (`- target:` … `platform:` … `asset:`).
 * @returns {Map<string, { target: string, asset: string }>}
 */
function releaseMatrixSpecs(yml) {
  /** @type {Map<string, { target: string, asset: string }>} */
  const map = new Map();
  /** @type {{ target: string, platform: string | null, asset: string | null } | null} */
  let cur = null;

  function flush() {
    if (cur && cur.platform && cur.asset && cur.target) {
      map.set(cur.platform, { target: cur.target, asset: cur.asset });
    }
    cur = null;
  }

  for (const line of yml.split('\n')) {
    const start = line.match(/^\s+- target:\s*(\S+)\s*$/);
    if (start) {
      flush();
      cur = { target: start[1], platform: null, asset: null };
      continue;
    }
    if (!cur) continue;
    const plat = line.match(/^\s+platform:\s*([a-z0-9-]+)\s*$/);
    if (plat) {
      cur.platform = plat[1];
      continue;
    }
    const asset = line.match(/^\s+asset:\s*(\S+)\s*$/);
    if (asset) {
      cur.asset = asset[1];
      continue;
    }
  }
  flush();
  return map;
}

/** Platform keys in release.yml build-matrix `include` entries. */
function releaseMatrixPlatforms(yml) {
  return [...releaseMatrixSpecs(yml).keys()].sort();
}

/**
 * Platforms hardcoded in the publish job (`for platform in …; do`).
 * @returns {string[]}
 */
function releasePublishLoopPlatforms(yml) {
  // Prefer `…; do` (bash for-loop). Require at least one platform token.
  // Critical: must match `for platform in a b; do` — older regexes that
  // stopped before `;` returned null and hid publish-loop omissions.
  const loop = yml.match(
    /for platform in ([a-z0-9-]+(?:\s+[a-z0-9-]+)*)\s*;/
  );
  if (!loop) return [];
  return loop[1]
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .sort();
}

/**
 * Soft-fail allowlist in release.yml (`residual_oidc="…"`).
 * Empty / missing is OK once Trusted Publisher covers all platforms.
 */
function residualOidcPlatforms(yml) {
  const m = yml.match(/^\s*residual_oidc="([^"]*)"\s*$/m);
  if (!m) return [];
  return m[1]
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .sort();
}

function runCheck() {
  const expected = cargoVersion();
  const rootPkg = readJson(path.join(root, 'package.json'));
  const errors = [];

  if (rootPkg.version !== expected) {
    errors.push(`package.json version ${rootPkg.version} != Cargo.toml ${expected}`);
  }

  const optional = rootPkg.optionalDependencies || {};
  for (const [name, ver] of Object.entries(optional)) {
    if (ver !== expected) {
      errors.push(`optionalDependencies.${name} ${ver} != ${expected}`);
    }
  }

  const platformsDir = path.join(root, 'npm', 'platforms');
  /** @type {Map<string, { name: string, version: string }>} */
  const platformPkgs = new Map();

  for (const entry of fs.readdirSync(platformsDir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const pkgPath = path.join(platformsDir, entry.name, 'package.json');
    if (!fs.existsSync(pkgPath)) {
      errors.push(`npm/platforms/${entry.name}/ missing package.json`);
      continue;
    }
    const pkg = readJson(pkgPath);
    const expectedName = `compendium-mcp-${entry.name}`;
    if (pkg.version !== expected) {
      errors.push(`${path.relative(root, pkgPath)} version ${pkg.version} != ${expected}`);
    }
    if (pkg.name !== expectedName) {
      errors.push(
        `${path.relative(root, pkgPath)} name ${pkg.name} != expected ${expectedName}`
      );
    }
    platformPkgs.set(entry.name, { name: pkg.name, version: pkg.version });
  }

  const dirKeys = [...platformPkgs.keys()].sort();
  const optionalNames = Object.keys(optional).sort();
  const platformNames = [...platformPkgs.values()].map((p) => p.name).sort();

  for (const name of optionalNames) {
    if (!platformNames.includes(name)) {
      errors.push(`optionalDependencies.${name} has no matching npm/platforms/*/package.json`);
    }
  }
  for (const name of platformNames) {
    if (!optionalNames.includes(name)) {
      errors.push(`platform package ${name} missing from package.json optionalDependencies`);
    }
  }

  const { keys: jsKeys, PLATFORMS } = platformJsKeys();
  if (jsKeys.join(',') !== dirKeys.join(',')) {
    errors.push(
      `npm/lib/platform.js PLATFORMS keys [${jsKeys.join(', ')}] != npm/platforms/ [${dirKeys.join(', ')}]`
    );
  }

  for (const key of dirKeys) {
    const spec = PLATFORMS[key];
    if (!spec) continue;
    const expectedPkg = `compendium-mcp-${key}`;
    if (spec.pkg !== expectedPkg) {
      errors.push(`PLATFORMS[${key}].pkg ${spec.pkg} != ${expectedPkg}`);
    }
    const onDisk = platformPkgs.get(key);
    if (onDisk && onDisk.name !== spec.pkg) {
      errors.push(`PLATFORMS[${key}].pkg ${spec.pkg} != ${onDisk.name} on disk`);
    }
  }

  const releaseYml = readReleaseYml();
  const matrixSpecs = releaseMatrixSpecs(releaseYml);
  const matrixKeys = releaseMatrixPlatforms(releaseYml);
  if (matrixKeys.length && matrixKeys.join(',') !== dirKeys.join(',')) {
    errors.push(
      `release.yml matrix platforms [${matrixKeys.join(', ')}] != npm/platforms/ [${dirKeys.join(', ')}]`
    );
  }

  const publishLoopKeys = releasePublishLoopPlatforms(releaseYml);
  if (publishLoopKeys.length && publishLoopKeys.join(',') !== dirKeys.join(',')) {
    errors.push(
      `release.yml publish loop [${publishLoopKeys.join(', ')}] != npm/platforms/ [${dirKeys.join(', ')}]`
    );
  }
  if (!publishLoopKeys.length && dirKeys.length) {
    errors.push(
      'release.yml missing `for platform in …;` publish loop (expected all npm/platforms keys)'
    );
  }

  for (const key of dirKeys) {
    const spec = PLATFORMS[key];
    const matrix = matrixSpecs.get(key);
    if (!spec || !matrix) continue;
    if (spec.asset !== matrix.asset) {
      errors.push(
        `release.yml asset for ${key} is ${matrix.asset} != PLATFORMS[${key}].asset ${spec.asset} (Releases fallback)`
      );
    }
    if (spec.rustTarget !== matrix.target) {
      errors.push(
        `release.yml target for ${key} is ${matrix.target} != PLATFORMS[${key}].rustTarget ${spec.rustTarget}`
      );
    }
  }

  const residualKeys = residualOidcPlatforms(releaseYml);
  const dirKeySet = new Set(dirKeys);
  for (const key of residualKeys) {
    if (!dirKeySet.has(key)) {
      errors.push(
        `release.yml residual_oidc lists unknown platform ${key} (not in npm/platforms/)`
      );
    }
  }

  if (errors.length) {
    console.error('Version / platform alignment check failed:');
    for (const e of errors) console.error(`  - ${e}`);
    process.exit(1);
  }

  const residualNote =
    residualKeys.length > 0
      ? `; residual_oidc=[${residualKeys.join(', ')}]`
      : '; residual_oidc=[]';
  console.log(
    `Version + platform alignment OK (${expected}; ${dirKeys.length} platforms${residualNote})`
  );
}

/**
 * Classify residual_oidc probe results (keys already on the allowlist).
 * - `exists` → stale soft-fail (must remove from residual_oidc)
 * - `missing` → still residual (OK)
 * - `error` → network/registry issue (caller may soft-skip)
 *
 * @param {{ key: string, status: 'exists' | 'missing' | 'error', detail?: string }[]} results
 * @returns {{ stale: string[], stillMissing: string[], errors: string[] }}
 */
function evaluateResidualProbe(results) {
  /** @type {string[]} */
  const stale = [];
  /** @type {string[]} */
  const stillMissing = [];
  /** @type {string[]} */
  const errors = [];
  for (const r of results) {
    if (r.status === 'exists') stale.push(r.key);
    else if (r.status === 'missing') stillMissing.push(r.key);
    else errors.push(`${r.key}: ${r.detail || 'probe failed'}`);
  }
  return { stale, stillMissing, errors };
}

/**
 * Classify publish-loop probes against the residual_oidc allowlist.
 * - in residual + exists → stale
 * - in residual + missing → stillMissing (soft-fail OK)
 * - not in residual + missing → uncovered (Release would hard-fail OIDC)
 * - probe error → errors (caller may soft-skip that key)
 *
 * @param {{ key: string, status: 'exists' | 'missing' | 'error', detail?: string }[]} results
 * @param {Iterable<string>} residualKeys
 * @returns {{ stale: string[], stillMissing: string[], uncovered: string[], errors: string[] }}
 */
function evaluateResidualCoverage(results, residualKeys) {
  const residualSet = new Set(residualKeys);
  /** @type {string[]} */
  const stale = [];
  /** @type {string[]} */
  const stillMissing = [];
  /** @type {string[]} */
  const uncovered = [];
  /** @type {string[]} */
  const errors = [];
  for (const r of results) {
    if (r.status === 'error') {
      errors.push(`${r.key}: ${r.detail || 'probe failed'}`);
      continue;
    }
    const inResidual = residualSet.has(r.key);
    if (inResidual) {
      if (r.status === 'exists') stale.push(r.key);
      else if (r.status === 'missing') stillMissing.push(r.key);
    } else if (r.status === 'missing') {
      uncovered.push(r.key);
    }
  }
  return { stale, stillMissing, uncovered, errors };
}

/**
 * GET registry.npmjs.org/<pkg> → { status: exists|missing|error }.
 * When `opts.version` is set, `exists` means that version is present in
 * `versions` (package 200 but missing version → `missing`).
 * @param {string} pkgName
 * @param {{ timeoutMs?: number, version?: string }} [opts]
 * @returns {Promise<{ status: 'exists' | 'missing' | 'error', detail?: string }>}
 */
function probeNpmPackageStatus(pkgName, opts = {}) {
  const timeoutMs = opts.timeoutMs ?? 15000;
  const wantVersion = opts.version;
  return new Promise((resolve) => {
    const url = `https://registry.npmjs.org/${encodeURIComponent(pkgName)}`;
    const req = https.get(
      url,
      {
        headers: {
          Accept: 'application/json',
          'User-Agent': 'compendium-mcp-check-residual',
        },
        timeout: timeoutMs,
      },
      (res) => {
        if (res.statusCode === 404) {
          res.resume();
          resolve({ status: 'missing' });
          return;
        }
        if (res.statusCode !== 200) {
          res.resume();
          resolve({ status: 'error', detail: `HTTP ${res.statusCode}` });
          return;
        }
        if (!wantVersion) {
          res.resume();
          resolve({ status: 'exists' });
          return;
        }
        /** @type {Buffer[]} */
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => {
          try {
            const body = Buffer.concat(chunks).toString('utf8');
            const json = JSON.parse(body);
            const versions = json && json.versions;
            if (versions && Object.prototype.hasOwnProperty.call(versions, wantVersion)) {
              resolve({ status: 'exists' });
            } else {
              resolve({
                status: 'missing',
                detail: `version ${wantVersion} not on registry`,
              });
            }
          } catch (err) {
            resolve({
              status: 'error',
              detail: err instanceof Error ? err.message : String(err),
            });
          }
        });
        res.on('error', (err) => {
          resolve({ status: 'error', detail: err.message });
        });
      }
    );
    req.on('timeout', () => {
      req.destroy();
      resolve({ status: 'error', detail: 'timeout' });
    });
    req.on('error', (err) => {
      resolve({ status: 'error', detail: err.message });
    });
  });
}

/**
 * Classify version probes for non-residual packages that already exist on npm.
 * - `exists` → optionalDeps version OK
 * - `missing` → versionAbsent (fail when release tag is published)
 * - `error` → soft-skip candidate
 *
 * @param {{ key: string, status: 'exists' | 'missing' | 'error', detail?: string }[]} results
 * @returns {{ ok: string[], versionAbsent: string[], errors: string[] }}
 */
function evaluatePublishedVersionPresence(results) {
  /** @type {string[]} */
  const ok = [];
  /** @type {string[]} */
  const versionAbsent = [];
  /** @type {string[]} */
  const errors = [];
  for (const r of results) {
    if (r.status === 'exists') ok.push(r.key);
    else if (r.status === 'missing') versionAbsent.push(r.key);
    else errors.push(`${r.key}: ${r.detail || 'probe failed'}`);
  }
  return { ok, versionAbsent, errors };
}

/**
 * HEAD a URL; treat 2xx/3xx as exists, 404 as missing, else error.
 * Does not follow redirects (GitHub release downloads respond 302 when present).
 * @param {string} url
 * @param {{ timeoutMs?: number, headers?: Record<string, string> }} [opts]
 * @returns {Promise<{ status: 'exists' | 'missing' | 'error', detail?: string }>}
 */
function probeHttpHead(url, opts = {}) {
  const timeoutMs = opts.timeoutMs ?? 15000;
  return new Promise((resolve) => {
    const req = https.request(
      url,
      {
        method: 'HEAD',
        headers: {
          'User-Agent': 'compendium-mcp-check-residual',
          ...(opts.headers || {}),
        },
        timeout: timeoutMs,
      },
      (res) => {
        res.resume();
        const code = res.statusCode || 0;
        if (code >= 200 && code < 400) resolve({ status: 'exists' });
        else if (code === 404) resolve({ status: 'missing' });
        else resolve({ status: 'error', detail: `HTTP ${code}` });
      }
    );
    req.on('timeout', () => {
      req.destroy();
      resolve({ status: 'error', detail: 'timeout' });
    });
    req.on('error', (err) => {
      resolve({ status: 'error', detail: err.message });
    });
    req.end();
  });
}

/**
 * HEAD github.com/<repo>/releases/tag/<tag> → exists|missing|error.
 * @param {string} repo
 * @param {string} tag
 * @param {{ timeoutMs?: number }} [opts]
 */
function probeGithubReleaseTag(repo, tag, opts = {}) {
  const url = `https://github.com/${repo}/releases/tag/${encodeURIComponent(tag)}`;
  return probeHttpHead(url, opts);
}

/**
 * HEAD github.com/<repo>/releases/download/<tag>/<asset> → exists|missing|error.
 * Present assets typically 302; missing assets 404.
 * @param {string} repo
 * @param {string} tag
 * @param {string} asset
 * @param {{ timeoutMs?: number }} [opts]
 */
function probeGithubReleaseAsset(repo, tag, asset, opts = {}) {
  const url = `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(asset)}`;
  return probeHttpHead(url, opts);
}

/**
 * Classify GitHub Release asset probes for residual (npm-missing) platforms.
 * - `exists` → fallback OK
 * - `missing` → broken fallback (fail)
 * - `error` → soft-skip candidate
 *
 * @param {{ key: string, asset: string, status: 'exists' | 'missing' | 'error', detail?: string }[]} results
 * @returns {{ ok: string[], missingAssets: string[], errors: string[] }}
 */
function evaluateResidualReleaseAssets(results) {
  /** @type {string[]} */
  const ok = [];
  /** @type {string[]} */
  const missingAssets = [];
  /** @type {string[]} */
  const errors = [];
  for (const r of results) {
    if (r.status === 'exists') ok.push(r.key);
    else if (r.status === 'missing') missingAssets.push(r.key);
    else errors.push(`${r.key} (${r.asset}): ${r.detail || 'probe failed'}`);
  }
  return { ok, missingAssets, errors };
}

function defaultGithubRepo() {
  return (
    process.env.COMPENDIUM_GITHUB_REPO ||
    process.env.GITHUB_REPOSITORY ||
    'hocestnonsatis/Compendium'
  );
}

/**
 * Fail when residual_oidc is stale (package already on npm) or incomplete
 * (publish-loop package still missing on npm but not soft-failed).
 * For still-missing residuals, also require GitHub Release assets for the
 * current package version (bin/run.js download fallback).
 * For non-residual packages that exist on npm, require `versions[version]`
 * when tag `v${version}` is published (optionalDeps pin). Also require the
 * main wrapper `compendium-mcp@${version}` for the same tag (`npx`).
 * Network / unpublished-tag probe errors warn and soft-skip.
 * @param {{
 *   fetchStatus?: (pkg: string) => Promise<{ status: string, detail?: string }>,
 *   fetchVersionStatus?: (pkg: string, version: string) => Promise<{ status: string, detail?: string }>,
 *   fetchReleaseTag?: (repo: string, tag: string) => Promise<{ status: string, detail?: string }>,
 *   fetchReleaseAsset?: (repo: string, tag: string, asset: string) => Promise<{ status: string, detail?: string }>,
 *   residualKeys?: string[],
 *   publishKeys?: string[],
 *   version?: string,
 *   githubRepo?: string,
 *   platforms?: Record<string, { asset: string }>,
 *   skipReleaseAssets?: boolean,
 *   skipVersionCheck?: boolean,
 * }} [opts]
 */
async function runResidualNpmCheck(opts = {}) {
  const fetchStatus = opts.fetchStatus || ((pkg) => probeNpmPackageStatus(pkg));
  const yml = readReleaseYml();
  const residualKeys = opts.residualKeys || residualOidcPlatforms(yml);
  const publishKeys =
    opts.publishKeys || releasePublishLoopPlatforms(yml);
  const residualSet = new Set(residualKeys);
  const keysToProbe = [...new Set([...publishKeys, ...residualKeys])].sort();

  if (!keysToProbe.length) {
    console.log('residual_oidc empty — no publish platforms to probe');
    return;
  }

  if (!residualKeys.length) {
    console.log(
      'residual_oidc empty — probing publish loop for uncovered missing packages'
    );
  }

  /** @type {{ key: string, status: string, detail?: string }[]} */
  const results = [];
  for (const key of keysToProbe) {
    const pkg = `compendium-mcp-${key}`;
    const probed = await fetchStatus(pkg);
    results.push({ key, status: probed.status, detail: probed.detail });
  }

  const { stale, stillMissing, uncovered, errors } = evaluateResidualCoverage(
    results,
    residualSet
  );

  if (stillMissing.length) {
    console.log(
      `residual_oidc still missing on npm: ${stillMissing.join(', ')} (soft-fail OK)`
    );
  }
  for (const e of errors) {
    console.warn(`residual_oidc probe warning: ${e}`);
  }
  if (stale.length) {
    console.error('residual_oidc soft-fail is stale — packages already on npm:');
    for (const key of stale) {
      console.error(
        `  - compendium-mcp-${key} exists; remove "${key}" from residual_oidc in release.yml`
      );
    }
    process.exit(1);
  }
  if (uncovered.length) {
    console.error(
      'publish platforms missing on npm but not in residual_oidc (Release would hard-fail OIDC):'
    );
    for (const key of uncovered) {
      console.error(
        `  - add "${key}" to residual_oidc in release.yml (or publish via Trusted Publisher)`
      );
    }
    process.exit(1);
  }
  if (errors.length && !stillMissing.length && !uncovered.length) {
    console.warn(
      'residual_oidc probe: lookups errored (registry unreachable?); soft-skipping'
    );
  }

  const version = opts.version || cargoVersion();
  const tag = `v${version}`;
  const repo = opts.githubRepo || defaultGithubRepo();
  const fetchTag =
    opts.fetchReleaseTag ||
    ((r, t) => probeGithubReleaseTag(r, t));

  // Version pin check: non-residual packages that exist must publish the
  // Cargo/package.json version once the GitHub release tag exists. Also gate
  // the main wrapper (npx -y compendium-mcp). Custom fetchStatus without
  // fetchVersionStatus skips (unit tests stay focused).
  // Release *pre-publish* sets COMPENDIUM_SKIP_PUBLISHED_VERSION_GATE=1 (or
  // opts.skipVersionCheck) — the tag exists before npm publish runs.
  const doVersionCheck =
    !opts.skipVersionCheck &&
    process.env.COMPENDIUM_SKIP_PUBLISHED_VERSION_GATE !== '1' &&
    (opts.fetchVersionStatus != null || opts.fetchStatus == null);
  /** @type {{ status: string, detail?: string } | null} */
  let cachedTagStatus = null;
  async function getTagStatus() {
    if (!cachedTagStatus) {
      cachedTagStatus = await fetchTag(repo, tag);
    }
    return cachedTagStatus;
  }
  if (doVersionCheck) {
    const fetchVersionStatus =
      opts.fetchVersionStatus ||
      ((pkg, ver) => probeNpmPackageStatus(pkg, { version: ver }));
    const publishedKeys = results
      .filter((r) => r.status === 'exists' && !residualSet.has(r.key))
      .map((r) => r.key);
    if (publishedKeys.length) {
      /** @type {{ key: string, status: string, detail?: string }[]} */
      const versionResults = [];
      for (const key of publishedKeys) {
        const pkg = `compendium-mcp-${key}`;
        const probed = await fetchVersionStatus(pkg, version);
        versionResults.push({
          key,
          status: probed.status,
          detail: probed.detail,
        });
      }
      const versionEval = evaluatePublishedVersionPresence(versionResults);
      for (const e of versionEval.errors) {
        console.warn(`optionalDeps version probe warning: ${e}`);
      }
      if (versionEval.versionAbsent.length) {
        const tagStatus = await getTagStatus();
        if (tagStatus.status === 'missing') {
          console.warn(
            `optionalDeps version ${version} missing on npm for ${versionEval.versionAbsent.join(', ')}; tag ${tag} not published yet — soft-skipping`
          );
        } else if (tagStatus.status === 'error') {
          console.warn(
            `optionalDeps version probe: tag ${tag} lookup failed (${tagStatus.detail || 'error'}); soft-skipping`
          );
        } else {
          console.error(
            `optionalDeps pin ${version} missing on npm after tag ${tag}:`
          );
          for (const key of versionEval.versionAbsent) {
            console.error(
              `  - compendium-mcp-${key}@${version} not on registry (publish or bump optionalDependencies)`
            );
          }
          process.exit(1);
        }
      } else if (versionEval.ok.length) {
        console.log(
          `optionalDeps version ${version} OK: ${versionEval.ok.join(', ')}`
        );
      } else if (versionEval.errors.length) {
        console.warn(
          'optionalDeps version probe: lookups errored; soft-skipping'
        );
      }
    }

    // Main wrapper: npx installs compendium-mcp@version, not only platform pkgs.
    const wrapperProbed = await fetchVersionStatus('compendium-mcp', version);
    if (wrapperProbed.status === 'missing') {
      const tagStatus = await getTagStatus();
      if (tagStatus.status === 'missing') {
        console.warn(
          `main wrapper compendium-mcp@${version} missing on npm; tag ${tag} not published yet — soft-skipping`
        );
      } else if (tagStatus.status === 'error') {
        console.warn(
          `main wrapper version probe: tag ${tag} lookup failed (${tagStatus.detail || 'error'}); soft-skipping`
        );
      } else {
        console.error(
          `main wrapper missing on npm after tag ${tag}: compendium-mcp@${version} (npx would break)`
        );
        process.exit(1);
      }
    } else if (wrapperProbed.status === 'exists') {
      console.log(`main wrapper compendium-mcp@${version} OK`);
    } else {
      console.warn(
        `main wrapper version probe warning: ${wrapperProbed.detail || 'probe failed'}; soft-skipping`
      );
    }
  }

  if (opts.skipReleaseAssets || !stillMissing.length) {
    return;
  }

  const { PLATFORMS } = opts.platforms
    ? { PLATFORMS: opts.platforms }
    : platformJsKeys();
  const fetchAsset =
    opts.fetchReleaseAsset ||
    ((r, t, asset) => probeGithubReleaseAsset(r, t, asset));

  const tagStatus = await fetchTag(repo, tag);
  if (tagStatus.status === 'missing') {
    console.warn(
      `residual Releases fallback: tag ${tag} not published yet; soft-skipping asset probe`
    );
    return;
  }
  if (tagStatus.status === 'error') {
    console.warn(
      `residual Releases fallback: tag ${tag} probe failed (${tagStatus.detail || 'error'}); soft-skipping`
    );
    return;
  }

  /** @type {{ key: string, asset: string, status: string, detail?: string }[]} */
  const assetResults = [];
  for (const key of stillMissing) {
    const spec = PLATFORMS[key];
    if (!spec || !spec.asset) {
      assetResults.push({
        key,
        asset: '(unknown)',
        status: 'error',
        detail: `no PLATFORMS[${key}].asset`,
      });
      continue;
    }
    const probed = await fetchAsset(repo, tag, spec.asset);
    assetResults.push({
      key,
      asset: spec.asset,
      status: probed.status,
      detail: probed.detail,
    });
  }

  const releaseEval = evaluateResidualReleaseAssets(assetResults);
  if (releaseEval.ok.length) {
    console.log(
      `residual Releases fallback OK (${tag}): ${releaseEval.ok.join(', ')}`
    );
  }
  for (const e of releaseEval.errors) {
    console.warn(`residual Releases asset probe warning: ${e}`);
  }
  if (releaseEval.missingAssets.length) {
    console.error(
      `residual Releases fallback broken — tag ${tag} exists but assets missing:`
    );
    for (const key of releaseEval.missingAssets) {
      const asset = PLATFORMS[key]?.asset || '(unknown)';
      console.error(
        `  - ${asset} (platform ${key}); upload the release asset or fix PLATFORMS.asset`
      );
    }
    process.exit(1);
  }
  if (
    releaseEval.errors.length &&
    !releaseEval.ok.length &&
    !releaseEval.missingAssets.length
  ) {
    console.warn(
      'residual Releases asset probe: lookups errored; soft-skipping'
    );
  }
}

module.exports = {
  releaseMatrixSpecs,
  releaseMatrixPlatforms,
  releasePublishLoopPlatforms,
  residualOidcPlatforms,
  evaluateResidualProbe,
  evaluateResidualCoverage,
  evaluateResidualReleaseAssets,
  evaluatePublishedVersionPresence,
  probeNpmPackageStatus,
  probeGithubReleaseTag,
  probeGithubReleaseAsset,
  runCheck,
  runResidualNpmCheck,
};

if (require.main === module) {
  runCheck();
}
