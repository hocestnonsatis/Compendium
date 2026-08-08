# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| `0.4.x` (npm `compendium-mcp` / git tags `v0.4.x`) | ✅ |
| Unreleased `master` | ✅ (best effort) |
| Older tags | ❌ |

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security problems.

Prefer one of:

1. **[GitHub Private Vulnerability Reporting](https://github.com/hocestnonsatis/Compendium/security/advisories/new)** (recommended)
2. Contact the maintainer via GitHub: [@hocestnonsatis](https://github.com/hocestnonsatis)

Include:

- Affected version / commit SHA
- Impact (data exfiltration, SSRF, secret leakage, DoS, etc.)
- Minimal reproduction steps
- Whether a fix or workaround is already known

We aim to acknowledge reports within **7 days** and share a remediation plan or status update as soon as practical.

## Security notes for this project

Compendium is an MCP server that processes untrusted tool/agent text:

- Local LLM URLs must be **loopback only** (`127.0.0.1`, `::1`, `localhost`) when `COMPENDIUM_LOCAL_LLM_URL` is set.
- Prefer `action=sanitize` / `sanitize_input` for untrusted payloads before they re-enter an agent context.
- Playbook bodies and briefings are sanitized by default before return.
- `pack` / `unpack` treat archives as **untrusted**: compressed/uncompressed size and file-count caps apply; **scripts inside archives are never executed**.
- Do not paste production secrets into issues, PRs, or sample fixtures.
