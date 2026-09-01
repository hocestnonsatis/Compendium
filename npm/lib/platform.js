'use strict';

/**
 * Platform → GitHub Release asset mapping for the `compendium-mcp` npm wrapper.
 *
 * Binary resolution (see bin/run.js):
 *   1. COMPENDIUM_BINARY env override
 *   2. Dev build: <repo>/target/release|debug/compendium
 *   3. Cached download from GitHub Releases (~/.cache/compendium-mcp/<version>/)
 */

const os = require('os');
const path = require('path');

/** @type {Record<string, { asset: string }>} */
const PLATFORMS = {
  'darwin-arm64': { asset: 'compendium-darwin-arm64' },
  'darwin-x64': { asset: 'compendium-darwin-x64' },
  'linux-x64': { asset: 'compendium-linux-x64' },
  'linux-arm64': { asset: 'compendium-linux-arm64' },
  'win32-x64': { asset: 'compendium-win32-x64.exe' },
};

function packageVersion() {
  return require('../../package.json').version;
}

function githubRepo() {
  return process.env.COMPENDIUM_GITHUB_REPO || 'hocestnonsatis/Compendium';
}

function platformKey() {
  const forced = (process.env.COMPENDIUM_PLATFORM || '').trim();
  if (forced) return forced;
  const arch = process.arch === 'ia32' ? 'x64' : process.arch;
  return `${process.platform}-${arch}`;
}

function currentPlatform() {
  const key = platformKey();
  const spec = PLATFORMS[key];
  if (!spec) {
    throw new Error(
      `Unsupported platform "${key}". Supported: ${Object.keys(PLATFORMS).join(', ')}.\n` +
        `Build from source (cargo build --release) and set COMPENDIUM_BINARY to the binary.`
    );
  }
  return spec;
}

function binaryName() {
  return process.platform === 'win32' ? 'compendium.exe' : 'compendium';
}

function cacheDir(version = packageVersion()) {
  const base = process.env.XDG_CACHE_HOME || path.join(os.homedir(), '.cache');
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
