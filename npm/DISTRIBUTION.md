# npm distribution (compendium-mcp)

## Strategy: optional platform packages + GitHub fallback

| Approach | Pros | Cons |
|----------|------|------|
| **optionalDependencies** (primary) | Offline after install, fast MCP spawn, npm CDN | Multi-package publish |
| **GitHub Releases download** (fallback) | Works if optional dep skipped | Needs network on first run |
| Vendoring all binaries in one tarball | Single package | Huge downloads for every OS |

We use the **esbuild-style hybrid**: each OS/arch ships as `compendium-mcp-<platform>`, listed under `optionalDependencies` of `compendium-mcp`. `bin/run.js` prefers that binary, then a local Cargo build, then downloads the release asset into `~/.cache/compendium-mcp/<version>/`.

Package name is **`compendium-mcp`** (`compendium` is taken on npm). The CLI bin is still **`compendium`**.

## Layout

```
package.json                 # main wrapper
bin/run.js                   # platform dispatcher (stdio inherit)
npm/lib/platform.js          # OS/arch → package/asset map
npm/platforms/<platform>/    # optionalDependency packages (binary filled by CI)
.github/workflows/release.yml
```

## Local test (before first publish)

### 1. Build the Rust binary

```bash
cargo build --release --features real-tokens,http
```

### 2. Stage into the matching platform package

```bash
# Linux x64 example — adjust platform folder for your machine
mkdir -p npm/platforms/linux-x64/bin
cp target/release/compendium npm/platforms/linux-x64/bin/compendium
chmod +x npm/platforms/linux-x64/bin/compendium
```

| Your machine | Platform folder | Binary name |
|--------------|-----------------|-------------|
| macOS Apple Silicon | `darwin-arm64` | `compendium` |
| macOS Intel | `darwin-x64` | `compendium` |
| Linux x64 | `linux-x64` | `compendium` |
| Linux arm64 | `linux-arm64` | `compendium` |
| Windows x64 | `win32-x64` | `compendium.exe` |

### 3. Link packages locally

```bash
# From each platform dir you staged:
cd npm/platforms/linux-x64 && npm link && cd -

# Main wrapper:
npm link
npm link compendium-mcp-linux-x64   # use your platform package name

# Smoke:
compendium --help
```

Or without global link:

```bash
node bin/run.js --help
COMPENDIUM_BINARY=./target/release/compendium node bin/run.js --help
```

### 4. Pack dry-run

```bash
npm pack --dry-run
# Should list bin/run.js, npm/lib/*, README, LICENSE — not target/
```

### 5. Cursor / Claude Desktop

Until published, point at the Cargo binary or wrapper:

```json
{
  "mcpServers": {
    "compendium": {
      "command": "node",
      "args": ["/absolute/path/to/Compendium/bin/run.js"],
      "env": {
        "COMPENDIUM_BINARY": "/absolute/path/to/Compendium/target/release/compendium"
      }
    }
  }
}
```

After publish:

```json
{
  "mcpServers": {
    "compendium": {
      "command": "npx",
      "args": ["-y", "compendium-mcp"]
    }
  }
}
```

## Publish (automated)

1. Set GitHub repo secret `NPM_TOKEN` (npm automation token with publish rights).
2. Repo defaults to `hocestnonsatis/Compendium` (CI also rewrites package metadata from `GITHUB_REPOSITORY`).
3. Keep versions aligned in `Cargo.toml`, root `package.json` (+ optionalDependencies), and `npm/platforms/*/package.json`.
4. Tag and create a GitHub Release:

```bash
git tag v0.1.0
git push origin v0.1.0
gh release create v0.1.0 --generate-notes
```

The `Release` workflow cross-compiles with `--features real-tokens,http`, uploads `compendium-<platform>` assets, then `npm publish`es each platform package and the main wrapper.

## Environment overrides

| Variable | Purpose |
|----------|---------|
| `COMPENDIUM_BINARY` | Force a local executable (skips resolve) |
| `COMPENDIUM_GITHUB_REPO` | `owner/repo` for download fallback |
| `COMPENDIUM_CACHE_DIR` | Override download cache root |
