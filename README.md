# FlowSight (Linux)

**Privacy-first productivity intelligence — runs locally on Linux.**



[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](./LICENSE)
[![Commercial License available](https://img.shields.io/badge/Commercial%20License-available-green.svg)](./COMMERCIAL-LICENSE.md)
[![CLA required](https://img.shields.io/badge/CLA-required-orange.svg)](./CLA.md)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/Mancasvel/FlowSight.AI)

FlowSight is a desktop application that helps distributed engineering
teams understand how their work flows, without the surveillance baggage of
traditional productivity tools. **All sensitive processing happens on the
developer's machine**: screen context, git metadata, and activity summaries
are analyzed by a bundled local LLM. Cloud sync is opt-in; connecting an
external AI through MCP can also send the requested report data to that AI.

---

## Features

- **100% local inference** — local `llama.cpp` + quantized Qwen3-VL-2B-Instruct GGUF
  model. No cloud roundtrips for sensitive data.
- **Desktop-native** — Tauri 2 (Rust) shell, Vite frontend, SQLite for local
  state. Ships as `.deb` and `.AppImage` packages for x86-64 Linux.
- **Activity-oriented, not surveillance-oriented** — the agent surfaces
  meaningful work units (branches, PRs, focus windows) rather than keystroke
  counts.
- **Team analytics, with consent** — opt-in aggregation into a Supabase
  backend only for users who join a team.
- **Self-hostable backend** — the Community Edition can run against your own
  Supabase instance.

## Bring your own AI (MCP)

The installed app includes a read-only [FlowSight MCP server](docs/MCP.md).
Open Settings > Connect your AI for the exact command to use in a compatible
desktop AI client. No extra runtime is required, and activity descriptions
and ticket IDs are excluded by default.

## Status

FlowSight is in **active development**. Expect breaking changes until
v1.0. Track progress on the [Releases](../../releases) page.

---

## Quick start

### Prerequisites

- **Linux x86-64** with the Tauri/WebKitGTK and screen-capture dependencies.
- **Rust** stable (for building the Tauri shell).
- **Node.js** 18+ and **pnpm** 8+.

### Install and run

```bash
git clone https://github.com/Mancasvel/FlowSight_linux.git
cd FlowSight_linux
pnpm install
pnpm dev
```

The dev command starts the agent with hot reload. Building the app downloads
the GGUF model from the FlowSight model release (see `scripts/fetch-models.mjs`).

### Build a release installer

```bash
pnpm build
```

The `.deb` and `.AppImage` land in `apps/agent/src-tauri/target/release/bundle/`.
Both packages include the Qwen3-VL-2B-Instruct weights and visual projector.
On first use, the app downloads a pinned Linux `llama.cpp` runtime if one is
not already present. Inference runs locally after that download.

---

## Architecture (short version)

```
+-------------------------------+       +-----------------------+
|  Tauri agent (Rust)           |       |  Supabase backend     |
|   - OAuth (Google)            |<----->|   - Teams             |
|   - Context capture           |       |   - Aggregated events |
|   - Local LLM (llama.cpp)     |       |   - RLS per team      |
|   - SQLite state              |       +-----------------------+
+---------------+---------------+
                |
                v
     ~/.local/share/FlowSight/
     (logs, db, cache — local only)
```

The heavy lifting (context summarization, PII filtering, intent inference)
runs against the local `llama-server`. Only already-filtered
aggregates reach the cloud backend, and only when the user belongs to a
team.

## Repository layout

```
apps/
  agent/          Tauri desktop app (Rust + Vite frontend)
  dashboard/      Next.js team dashboard (optional)
local_llm/
  bin/            Optional local development runtime
  *.gguf          Local model weights (fetched at build time, not committed)
scripts/
  fetch-models.mjs  Prebuild hook (Node-only): downloads GGUF from GitHub Releases
.github/
  workflows/      CI (build, release, gitleaks)
```

## Configuration

All user-facing settings live in the desktop app. Local state is persisted
under:

- **Windows:** `%LOCALAPPDATA%\FlowSight\`
- Logs: `server.log`, `agent_error.log`, `crash.log`
- Database: `dev-agent.db` (SQLite)

No configuration file is expected on the user's side. The environment
variable `GITHUB_TOKEN` is only needed by developers who want to fetch
model assets from a private release.

---

## License and distribution

FlowSight is distributed under a **dual licensing model**:

| Edition | License | Intended for |
|---|---|---|
| **Community** | [GNU AGPL-3.0](./LICENSE) | Individuals, OSS projects, academic use, internal non-commercial deployments |
| **Enterprise / Commercial** | [Proprietary, per contract](./COMMERCIAL-LICENSE.md) | Closed-source redistribution, SaaS offerings, OEM, customers whose policies forbid AGPL |

> **TL;DR:** you can use, modify and self-host the Community Edition as
> long as you respect the AGPL — which, crucially, requires you to publish
> your modifications if you expose them over a network. If you can't live
> with that, buy a commercial license: **manuel@flowsight.site**.

### Contributing

Contributions are very welcome. **Every contributor must sign a CLA**
(individuals: [`CLA.md`](./CLA.md), companies: [`CLA-CORPORATE.md`](./CLA-CORPORATE.md))
so that the project can keep the dual-licensing model working. The
[`CLA Assistant`](https://cla-assistant.io/) bot handles signatures
automatically on your first PR. See [`CONTRIBUTING.md`](./CONTRIBUTING.md)
for the full flow.

### Code of Conduct

Participation is governed by the [Contributor Covenant](./CODE_OF_CONDUCT.md).

### Security

If you find a vulnerability, **please do not open a public issue**. Use
GitHub's private Security Advisory feature on this repository, or email
**manuel@flowsight.site**.

---

## Trademarks

"FlowSight" is a trademark of FlowSight. The AGPL license grants you
rights to the code but **not** to the trademark. If you
publish a fork, please pick a different name for your distribution.

---

## Links

- **Product website:** *coming soon*
- **Commercial inquiries:** manuel@flowsight.site
- **Security reports:** manuel@flowsight.site
- **Legal (CLA questions):** manuel@flowsight.site
