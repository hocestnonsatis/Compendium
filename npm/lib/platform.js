'use strict';

/**
 * Shared platform → package / release-asset mapping for Compendium npm distribution.
 *
 * Distribution strategy (hybrid):
 * 1. Prefer optionalDependency platform packages (esbuild-style) — offline, fast.
 * 2. Fall back to downloading the matching GitHub Release asset into a local cache.
 * 3. Dev override: COMPENDIUM_BINARY or ./target/release/compendium.
 */

const os = require('os');
const path = require('path');

/** @typedef {{ pkg: string, asset: string, rustTarget: string }} PlatformSpec */

/** @type {Record<string, PlatformSpec>} */
const PLATFORMS = {
  'darwin-arm64': {
    pkg: 'compendium-mcp-darwin-arm64',
    asset: 'compendium-darwin-arm64',
    rustTarget: 'aarch64-apple-darwin',
  },
  'darwin-x64': {
    pkg: 'compendium-mcp-darwin-x64',
    asset: 'compendium-darwin-x64',
    rustTarget: 'x86_64-apple-darwin',
  },
  'linux-x64': {
    pkg: 'compendium-mcp-linux-x64',
    asset: 'compendium-linux-x64',
    rustTarget: 'x86_64-unknown-linux-gnu',
  },
  'linux-arm64': {
    pkg: 'compendium-mcp-linux-arm64',
    asset: 'compendium-linux-arm64',
    rustTarget: 'aarch64-unknown-linux-gnu',
  },
  'win32-x64': {
    pkg: 'compendium-mcp-win32-x64',
    asset: 'compendium-win32-x64.exe',
    rustTarget: 'x86_64-pc-windows-msvc',
  },
};

function packageVersion() {
  // Keep in sync with root package.json / Cargo.toml at publish time.
  return require('../../package.json').version;
}

function githubRepo() {
  return (
    process.env.COMPENDIUM_GITHUB_REPO ||
    process.env.GITHUB_REPOSITORY ||
    'hocestnonsatis/Compendium'
  );
}

/**
 * Normalize Node's process.platform + process.arch into our key.
 * @returns {string}
 */
function platformKey() {
  const platform = process.platform;
  let arch = process.arch;
  // Rosetta / rare aliases
  if (arch === 'ia32') arch = 'x64';
  return `${platform}-${arch}`;
}

/**
 * @returns {PlatformSpec}
 */
function currentPlatform() {
  const key = platformKey();
  const spec = PLATFORMS[key];
  if (!spec) {
    const supported = Object.keys(PLATFORMS).join(', ');
    throw new Error(
      `Unsupported platform "${key}". Supported: ${supported}.\n` +
        `Set COMPENDIUM_BINARY to a local build, or open an issue for this target.`
    );
  }
  return spec;
}

/**
 * Binary filename inside a platform package / cache.
 * @param {PlatformSpec} [spec]
 */
function binaryName(spec = currentPlatform()) {
  return process.platform === 'win32' ? 'compendium.exe' : 'compendium';
}

/**
 * Cache directory for lazily downloaded binaries.
 * @param {string} version
 */
function cacheDir(version = packageVersion()) {
  const base =
    process.env.COMPENDIUM_CACHE_DIR ||
    process.env.XDG_CACHE_HOME ||
    path.join(os.homedir(), '.cache');
  return path.join(base, 'compendium-mcp', version);
}

module.exports = {
  PLATFORMS,
  packageVersion,
  githubRepo,
  platformKey,
  currentPlatform,
  binaryName,
  cacheDir,
};
