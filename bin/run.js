#!/usr/bin/env node
'use strict';

/**
 * Compendium MCP launcher.
 *
 * Finds (or downloads once) the native `compendium` binary for this platform
 * and runs it with inherited stdio, so MCP clients talk JSON-RPC over stdin/stdout.
 */

const { spawn } = require('child_process');
const fs = require('fs');
const https = require('https');
const path = require('path');
const { pipeline } = require('stream/promises');

const {
  packageVersion,
  githubRepo,
  currentPlatform,
  binaryName,
  cacheDir,
  platformKey,
} = require('../npm/lib/platform');

function isExecutable(file) {
  try {
    fs.accessSync(file, fs.constants.F_OK);
    if (process.platform !== 'win32') fs.accessSync(file, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function devBinary() {
  const repoRoot = path.resolve(__dirname, '..');
  for (const profile of ['release', 'debug']) {
    const candidate = path.join(repoRoot, 'target', profile, binaryName());
    if (isExecutable(candidate)) return candidate;
  }
  return null;
}

function download(url, dest, hops = 0) {
  return new Promise((resolve, reject) => {
    const req = https.get(
      url,
      {
        headers: {
          'User-Agent': `compendium-mcp/${packageVersion()}`,
          Accept: 'application/octet-stream',
        },
      },
      (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          res.resume();
          if (hops >= 5) return reject(new Error('too many redirects'));
          download(res.headers.location, dest, hops + 1).then(resolve, reject);
          return;
        }
        if (res.statusCode !== 200) {
          res.resume();
          reject(
            new Error(
              `Download failed (${res.statusCode}) for ${url}. ` +
                `No release asset for ${platformKey()}? Build from source and set COMPENDIUM_BINARY.`
            )
          );
          return;
        }
        const tmp = `${dest}.partial`;
        pipeline(res, fs.createWriteStream(tmp))
          .then(() => {
            fs.renameSync(tmp, dest);
            if (process.platform !== 'win32') fs.chmodSync(dest, 0o755);
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

async function downloadedBinary() {
  const spec = currentPlatform();
  const version = packageVersion();
  const dir = cacheDir(version);
  fs.mkdirSync(dir, { recursive: true });
  const dest = path.join(dir, binaryName());
  if (isExecutable(dest)) return dest;

  const url = `https://github.com/${githubRepo()}/releases/download/v${version}/${spec.asset}`;
  process.stderr.write(`[compendium-mcp] downloading ${spec.asset} v${version}…\n`);
  await download(url, dest);
  return dest;
}

async function resolveBinary() {
  if (process.env.COMPENDIUM_BINARY) {
    const forced = path.resolve(process.env.COMPENDIUM_BINARY);
    if (!isExecutable(forced)) {
      throw new Error(`COMPENDIUM_BINARY not executable: ${forced}`);
    }
    return forced;
  }
  return devBinary() || downloadedBinary();
}

async function main() {
  let binary;
  try {
    binary = await resolveBinary();
  } catch (err) {
    process.stderr.write(`[compendium-mcp] ${err.message}\n`);
    process.exit(1);
  }

  const child = spawn(binary, process.argv.slice(2), {
    stdio: 'inherit',
    windowsHide: true,
  });

  child.on('error', (err) => {
    process.stderr.write(`[compendium-mcp] failed to start ${binary}: ${err.message}\n`);
    process.exit(1);
  });

  for (const signal of ['SIGINT', 'SIGTERM']) {
    process.on(signal, () => {
      if (!child.killed) child.kill(signal);
    });
  }

  child.on('exit', (code, signal) => {
    if (signal) process.exit(signal === 'SIGINT' ? 130 : 1);
    process.exit(code ?? 1);
  });
}

main();
