# Engramark

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="assets/logo-light.png" />
    <img src="assets/logo-dark.png" width="140" alt="Engramark" />
  </picture>
</p>

<p align="center">
  <strong>Let coding assistants remember what matters across tasks—and retrieve it only when needed.</strong>
</p>

<p align="center">
  <a href="https://github.com/sunkanwei/engramark/releases/latest"><img src="https://img.shields.io/github/v/release/sunkanwei/engramark" alt="Latest release" /></a>
  <a href="https://github.com/sunkanwei/engramark/actions/workflows/ci.yml"><img src="https://github.com/sunkanwei/engramark/actions/workflows/ci.yml/badge.svg" alt="CI status" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT license" /></a>
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="docs/user-guide.md">User guide</a> ·
  <a href="docs/installation.md">Install and upgrade</a> ·
  <a href="docs/architecture.md">Architecture</a> ·
  <a href="docs/testing.md">Testing</a>
</p>

Engramark gives Codex and OpenCode one shared, local long-term memory. You decide what is worth keeping. When a related question comes up later, the assistant can retrieve that knowledge without placing entire conversation histories into context.

Memories remain readable text on your own computer. Engramark needs no cloud account, runs no daemon, does not depend on an LLM for retrieval, and includes no telemetry.

![Engramark retrieves short hints only when they are relevant](assets/hero-context.en.svg)

## What it solves

- **Stop repeating project conventions.** Keep technology choices, directory aliases, architecture decisions, and reusable workflows across tasks.
- **Keep context small.** Only a few relevant hints enter a request; full details are read when they are actually needed.
- **Keep projects separate.** Project memories stay in their own project, while personal preferences can use global scope.
- **Nothing is recorded implicitly.** Engramark writes only after an explicit long-term save request. Ordinary chat, temporary progress, and tool output are not collected.
- **Keep control of the data.** Readable local files are the original memory source, and the search index can be rebuilt at any time.

## How it works

1. **You decide what lasts.** Tell the assistant to remember a fact, decision, preference, path, or reusable workflow.
2. **Engramark stores and searches locally.** Codex and OpenCode can use the same collection without a separate memory cloud service.
3. **The assistant retrieves only what it needs.** A related request can receive a few short hints, followed by full details only when necessary.

There are no required wake words and no memory files to manage by hand. See the [complete user guide](docs/user-guide.md) for candidates, archiving, backup, and other maintenance tasks.

## Install

Current release targets are macOS on Apple Silicon and Intel, Linux x86_64, and Windows x86_64. The package contains one native executable with embedded SQLite. Python, Homebrew, a database server, and a package manager are not required.

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/sunkanwei/engramark/main/install.sh -o /tmp/engramark-install.sh
sh /tmp/engramark-install.sh
```

Windows PowerShell 5.1 or PowerShell 7:

```powershell
$script = Join-Path $env:TEMP "engramark-install.ps1"
Invoke-WebRequest https://raw.githubusercontent.com/sunkanwei/engramark/main/install.ps1 -OutFile $script
& "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File $script
```

> [!NOTE]
> You can paste the Windows command into PowerShell 5.1 or 7. It deliberately runs the installer with the built-in Windows PowerShell 5.1 for consistent behavior. `-ExecutionPolicy Bypass` affects only this installer process, does not persistently change policy, and cannot override organization-enforced Group Policy.

> [!WARNING]
> Current public packages do not have Apple Developer ID or Windows code signing. Install only from this repository's Releases or the official scripts above. Do not bypass a system warning if you cannot verify the download source. The installer verifies release checksums and the package file manifest.

See [Install and Upgrade](docs/installation.md) for platform details, upgrades, paths, and uninstall behavior.

## Complete your first workflow in three minutes

After installation, fully quit and reopen Codex or OpenCode, then start a task inside a project.

1. Say: “Remember that pnpm is this project's package manager. Save it as a project memory.” The assistant should confirm the save and report the memory ID.
2. Start a new task and ask: “Do you remember which package manager this project uses?” The assistant should answer `pnpm`.
3. Continue with: “Update the package-manager memory to say this project uses pnpm 10.” The existing memory should be updated instead of duplicated.
4. If this was only a test, say: “Delete that test memory.” Deleting an active memory requires another explicit confirmation.

In daily work, ask naturally: “What checks do we run before a release?”, “What database migration process did we decide on?”, or “Review my long-term memories for anything stale, conflicting, or waiting for confirmation.” The assistant decides when to use memory; users do not need to name the product or a tool.

## Codex and OpenCode

| Use | Codex | OpenCode |
|---|---|---|
| Save and retrieve through natural language | Supported | Supported |
| Automatic short hints on related requests | Available after installation | Disabled by default |
| Retrieval while automatic hints are off | Still available through natural questions | Still available through natural questions |

OpenCode automatic hints are disabled by default because the hints may be stored with conversation messages. Enable them only after understanding that behavior; see [Automatic recall and explicit lookup](docs/user-guide.md#automatic-recall-and-explicit-lookup).

## Privacy and reliability

- Program files and private memory data are separate. Reinstallation does not overwrite memories, and uninstall does not delete them.
- Private directories and files use operating-system permissions for the current user. Full memory content in the cache should still be protected with disk encryption such as FileVault or BitLocker.
- “Local” describes storage and retrieval. A retrieved memory enters the current coding assistant's context and is then handled under that assistant's service boundary.
- Retrieval reports insufficient evidence instead of presenting a weak association as a remembered fact.
- A temporary memory failure does not block Codex or OpenCode from handling the original request; automatic hints are simply skipped.
- Consistent backup does not copy a live database, and rollback first preserves the current state.

<details>
<summary><strong>View the project regression baseline</strong></summary>

These figures come from the project's synthetic regression suite. They are not a public benchmark such as LOCOMO and are not a performance guarantee for every device. Actual latency varies with hardware, operating system, and memory content.

| Metric | Current reference result |
|---|---:|
| Synthetic long-term collection | 2,000 cards |
| Full index rebuild | about 0.5 s |
| Hot query p95 | about 7 ms |
| Golden recall@5 | 1.0 |
| Unrelated-query rejection | 1.0 |
| False automatic hints | 0 |
| Project isolation | passed |

See [Testing and Validation](docs/testing.md) for the reproducible procedure and the larger 10,006-card pre-release check.

</details>

## Documentation

For users:

- [User guide](docs/user-guide.md) — first use, daily memory, scope, review, backup, and common problems.
- [Install and upgrade](docs/installation.md) — supported platforms, trusted installation, upgrades, configuration, paths, and uninstall.

For maintainers and contributors:

- [Architecture](docs/architecture.md) — data, retrieval, concurrency, recovery, security, and host integration.
- [Testing and validation](docs/testing.md) — which checks to run for each kind of change and what a release must pass.
- [Maintainer release guide](docs/release-guide.md) — version decisions, builds, four-platform validation, and GitHub Releases.
- [Third-party notices](THIRD_PARTY_NOTICES.md) — licensing for dependencies and Unicode data.

## Current boundaries

- The data directory is supported only on a local filesystem. NFS, SMB, synchronized cloud folders, and removable media are outside the consistency guarantee.
- OpenCode automatic hints are disabled by default and version-gated. Natural-language retrieval remains available.
- Retrieval currently relies mainly on deterministic text and identifier matching. Semantic retrieval remains on the roadmap.
- Release packages include checksums, a per-file manifest, dependency metadata, and GitHub build provenance, but current packages are not platform-signed.
- English documentation is complete, while most current runtime messages and MCP-facing descriptions remain Chinese.

Engramark is licensed under the [MIT License](LICENSE).
