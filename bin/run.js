#!/usr/bin/env node
'use strict';

/**
 * Compendium MCP dispatcher.
 *
 * Resolves the native binary and exec's it with inherited stdio so Cursor /
 * Claude Desktop / `npx compendium-mcp` talk JSON-RPC over stdin/stdout.
 *
 * Resolution order:
 *   1. COMPENDIUM_BINARY env
 *   2. Dev build: <repo>/target/release|debug/compendium(.exe)
 *   3. optionalDependency platform package (compendium-mcp-<platform>)
 *   4. Cached GitHub Release download (~/.cache/compendium-mcp/<version>/)
 */

const { spawn } = require('child_process');
const fs = require('fs');
const https = require('https');
const http = require('http');
const path = require('path');
const { pipeline } = require('stream/promises');
const { createWriteStream } = require('fs');

const {
  packageVersion,
  githubRepo,
  currentPlatform,
  binaryName,
  cacheDir,
  platformKey,
} = require('../npm/lib/platform');

function existsExecutable(file) {
  try {
    fs.accessSync(file, fs.constants.F_OK);
    // On Windows, X_OK is unreliable; presence is enough.
    if (process.platform !== 'win32') {
      fs.accessSync(file, fs.constants.X_OK);
    }
    return true;
  } catch {
    return false;
  }
}

function tryRequirePlatformBinary() {
  const spec = currentPlatform();
  try {
    // optionalDependency installs next to this package when available.
    const pkgRoot = path.dirname(require.resolve(`${spec.pkg}/package.json`));
    const candidate = path.join(pkgRoot, 'bin', binaryName(spec));
    if (existsExecutable(candidate)) return candidate;
  } catch {
    // optional dep missing — fall through
  }
  return null;
}

function tryDevBinary() {
  const repoRoot = path.resolve(__dirname, '..');
  const name = process.platform === 'win32' ? 'compendium.exe' : 'compendium';
  const release = path.join(repoRoot, 'target', 'release', name);
  const debug = path.join(repoRoot, 'target', 'debug', name);
  if (existsExecutable(release)) return release;
  if (existsExecutable(debug)) return debug;
  return null;
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const client = url.startsWith('https:') ? https : http;
    const req = client.get(
      url,
      {
        headers: {
          'User-Agent': `compendium-mcp/${packageVersion()}`,
          Accept: 'application/octet-stream',
        },
      },
      (res) => {
        // Follow one redirect hop (GitHub release assets).
        if (
          res.statusCode >= 300 &&
          res.statusCode < 400 &&
          res.headers.location
        ) {
          res.resume();
          download(res.headers.location, dest).then(resolve, reject);
          return;
        }
        if (res.statusCode !== 200) {
          res.resume();
          reject(
            new Error(
              `Download failed (${res.statusCode}) for ${url}. ` +
                `Ensure release assets exist for ${platformKey()}.`
            )
          );
          return;
        }
        const tmp = `${dest}.partial`;
        const out = createWriteStream(tmp);
        pipeline(res, out)
          .then(() => {
            fs.renameSync(tmp, dest);
            if (process.platform !== 'win32') {
              fs.chmodSync(dest, 0o755);
            }
            resolve(dest);
          })
          .catch((err) => {
            try {
              fs.unlinkSync(tmp);
            } catch {
              /* ignore */
            }
            reject(err);
          });
      }
    );
    req.on('error', reject);
  });
}

async function ensureDownloadedBinary() {
  const spec = currentPlatform();
  const version = packageVersion();
  const dir = cacheDir(version);
  fs.mkdirSync(dir, { recursive: true });
  const dest = path.join(dir, binaryName(spec));
  if (existsExecutable(dest)) return dest;

  const repo = githubRepo();
  const tag = `v${version}`;
  const url = `https://github.com/${repo}/releases/download/${tag}/${spec.asset}`;

  process.stderr.write(
    `[compendium-mcp] Downloading ${spec.asset} (${tag})…\n`
  );
  await download(url, dest);
  process.stderr.write(`[compendium-mcp] Cached at ${dest}\n`);
  return dest;
}

async function resolveBinary() {
  if (process.env.COMPENDIUM_BINARY) {
    const forced = path.resolve(process.env.COMPENDIUM_BINARY);
    if (!existsExecutable(forced)) {
      throw new Error(`COMPENDIUM_BINARY not executable: ${forced}`);
    }
    return forced;
  }

  // Prefer a local Cargo build when developing from this repo so `cargo build
  // --release` is picked up immediately (optional platform packages can lag).
  const fromDev = tryDevBinary();
  if (fromDev) return fromDev;

  const fromOptional = tryRequirePlatformBinary();
  if (fromOptional) return fromOptional;

  return ensureDownloadedBinary();
}

async function main() {
  let binary;
  try {
    binary = await resolveBinary();
  } catch (err) {
    process.stderr.write(`[compendium-mcp] ${err.message}\n`);
    process.exit(1);
  }

  process.stderr.write(`[compendium-mcp] using ${binary}\n`);

  const child = spawn(binary, process.argv.slice(2), {
    stdio: 'inherit',
    windowsHide: true,
  });

  child.on('error', (err) => {
    process.stderr.write(`[compendium-mcp] failed to spawn ${binary}: ${err.message}\n`);
    process.exit(1);
  });

  const forward = (signal) => {
    if (!child.killed) child.kill(signal);
  };
  process.on('SIGINT', () => forward('SIGINT'));
  process.on('SIGTERM', () => forward('SIGTERM'));

  child.on('exit', (code, signal) => {
    if (signal) {
      process.exit(signal === 'SIGINT' ? 130 : 1);
    }
    process.exit(code ?? 1);
  });
}

main();
