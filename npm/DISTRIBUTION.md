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
| Linux x64 (glibc) | `linux-x64` | `compendium` |
| Linux x64 (musl / Alpine) | `linux-x64-musl` | `compendium` |
| Linux arm64 | `linux-arm64` | `compendium` |
| Windows x64 | `win32-x64` | `compendium.exe` |
| Windows ARM64 | `win32-arm64` | `compendium.exe` |

Force a key with `COMPENDIUM_PLATFORM` (e.g. `linux-x64-musl`) when auto-detection is wrong.

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

## Publish (automated — Trusted Publishing / OIDC)

No `NPM_TOKEN` secret. CI uses [npm Trusted Publishing](https://docs.npmjs.com/trusted-publishers) (OIDC).

**Residual (v0.5.0 / v0.6.0):** `compendium-mcp-linux-x64-musl` and `compendium-mcp-win32-arm64` are not yet on the registry. Until Trusted Publisher is configured (or one interactive `npm publish` creates the package), the Release workflow **soft-fails** those two platforms (`residual_oidc` allowlist in `release.yml`) with a warning — binaries still upload to GitHub Releases and the wrapper + other platforms publish successfully. Clients obtain the residual binaries via the Releases download fallback in `bin/run.js`. `npm run check-versions` rejects unknown keys in `residual_oidc`, keeps `release.yml` matrix `asset`/`target` aligned with `npm/lib/platform.js` `PLATFORMS`, and requires the publish `for platform in …` list to match `npm/platforms/` independently of the build matrix so a publish-loop omission cannot hide behind a union. `npm run check-versions-selftest` (CI) locks the publish-loop / matrix / `residual_oidc` parsers against silent regex regressions. `npm run check-residual-npm` (CI + Release) probes the registry and **fails** when a residual package already exists (clear soft-fail) **or** when a publish-loop package is still missing but absent from `residual_oidc` (would hard-fail OIDC on the next Release). For non-residual packages already on npm it also requires `versions[version]` once tag `v${version}` exists (soft-skip pre-tag) so optionalDeps cannot pin an unpublished version after a release. The same post-tag gate applies to the main wrapper `compendium-mcp@${version}` (`npx -y compendium-mcp`). For still-missing residuals it also HEAD-probes GitHub Release assets for `v${version}` and **fails** if the tag exists but an asset is absent (wrapper fallback broken); soft-skips when the tag is not published yet.

### 1. Dashboard (once per package)

On each package’s **Settings → Trusted Publisher → GitHub Actions**:

| Field | Value |
|-------|--------|
| Organization or user | `hocestnonsatis` |
| Repository | `Compendium` |
| Workflow filename | `release.yml` |
| Environment | *(leave empty unless you add a GitHub Environment)* |
| Allowed actions | `npm publish` |

Packages to configure (same values on each):

- https://www.npmjs.com/package/compendium-mcp/access
- https://www.npmjs.com/package/compendium-mcp-darwin-arm64/access
- https://www.npmjs.com/package/compendium-mcp-darwin-x64/access
- https://www.npmjs.com/package/compendium-mcp-linux-x64/access
- https://www.npmjs.com/package/compendium-mcp-linux-arm64/access
- https://www.npmjs.com/package/compendium-mcp-linux-x64-musl/access *(brand-new: create Trusted Publisher before first OIDC publish, or one interactive publish first)*
- https://www.npmjs.com/package/compendium-mcp-win32-x64/access
- https://www.npmjs.com/package/compendium-mcp-win32-arm64/access *(brand-new: same as musl)*

**Residual human checklist (blocker):** for the two brand-new packages only (`linux-x64-musl`, `win32-arm64`):

1. Open each `…/access` link above (or npm → package → Settings → Trusted Publisher). If the package does not exist yet, use npm’s Trusted Publisher UI to create the package binding, **or** run one interactive `npm publish` from `npm/platforms/<platform>/` to create it.
2. Set the GitHub Actions publisher fields to the table above (`hocestnonsatis` / `Compendium` / `release.yml`).
3. Re-run Release (`workflow_dispatch` with tag `v0.6.0` or the next release) and confirm OIDC publish succeeds for those platforms (no soft-fail warning).
4. Remove `linux-x64-musl` and `win32-arm64` from `residual_oidc` in `.github/workflows/release.yml`. `npm run check-residual-npm` fails if you forget after the packages exist.

After OIDC works, optionally set **Publishing access** → “Require 2FA and disallow tokens”.

### 2. Version alignment

Keep versions aligned in `Cargo.toml`, root `package.json` (+ optionalDependencies), and `npm/platforms/*/package.json`.

### 3. Tag + GitHub Release

```bash
git tag v0.1.1
git push origin v0.1.1
gh release create v0.1.1 --generate-notes
```

The `Release` workflow (`release.yml`) cross-compiles with `--features real-tokens,http`, uploads `compendium-<platform>` assets, then `npm publish`es each platform package and the main wrapper via OIDC (automatic provenance).

Requires Node ≥22.14 / npm ≥11.5.1 on the publish job (workflow uses Node 24).

## Environment overrides

| Variable | Purpose |
|----------|---------|
| `COMPENDIUM_BINARY` | Force a local executable (skips resolve) |
| `COMPENDIUM_GITHUB_REPO` | `owner/repo` for download fallback |
| `COMPENDIUM_CACHE_DIR` | Override download cache root |
