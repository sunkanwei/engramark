# Install and Upgrade

**[English](installation.md) | [简体中文](安装指南.md)**

This guide covers supported platforms, installation, upgrades, configuration, troubleshooting, and uninstall behavior. Maintainers building release candidates should use the [Maintainer Release Guide](release-guide.md).

## Supported platforms

| Release target | Architecture | Current native validation environment |
|---|---|---|
| macOS | Apple Silicon and Intel | Apple Silicon: macOS 14/15; Intel: macOS 15 |
| Linux | x86_64 with glibc 2.35 or newer | Built on Ubuntu 22.04 and revalidated on Ubuntu 24.04 |
| Windows | x86_64 | Windows Server 2022/2025 runners; PowerShell 5.1 and 7 installation lifecycles |

Windows Server in the table is the automated validation environment; it does not mean Engramark is a server application. An unlisted operating system or architecture may work but is outside the current native-validation commitment.

An artifact is supported only after its native job completes the full test suite, capability self-check, packaging, and installation lifecycle. Cross-compilation and minimum-version declarations do not replace native execution evidence.

## Install

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/sunkanwei/engramark/main/install.sh -o /tmp/engramark-install.sh
sh /tmp/engramark-install.sh
```

Windows: paste the following into PowerShell 5.1 or 7. It runs the installer with the built-in Windows PowerShell 5.1.

```powershell
$script = Join-Path $env:TEMP "engramark-install.ps1"
Invoke-WebRequest https://raw.githubusercontent.com/sunkanwei/engramark/main/install.ps1 -OutFile $script
& "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File $script
```

> [!NOTE]
> `-ExecutionPolicy Bypass` applies only to this child process and does not persistently change the user's execution policy. Organization-enforced Group Policy still takes precedence and may require administrator approval.

The archive contains one native executable with embedded SQLite. Python, Homebrew, a database server, and a package manager are not required.

## After installation

The installer connects detected Codex and OpenCode installations. When it finishes:

1. fully quit and reopen Codex or OpenCode;
2. start a task inside a project directory;
3. say: “Remember that pnpm is this project's package manager. Save it as a project memory.”

A confirmation with a memory ID shows that saving is connected. Start a new task and ask “Do you remember which package manager this project uses?” to check retrieval and project detection as well.

See the [User Guide](user-guide.md#complete-your-first-workflow-in-five-minutes) for the complete first-use workflow.

## Verify an installation

Normal use does not require command-line diagnosis. To verify cards, durable ID state, the local index, and runtime capabilities, run the full diagnosis.

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark diagnose --full
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" diagnose --full
```

A successful result without errors means that the current data and retrieval capabilities passed inspection.

<details>
<summary><strong>What the installer does internally</strong></summary>

The installer:

1. selects the artifact for the current operating system and architecture;
2. verifies its release SHA-256;
3. rejects absolute paths, traversal, links, special files, case-folding collisions, and oversized archives;
4. verifies the file allowlist, size, and hash of every extracted file against `MANIFEST.tsv`;
5. runs the SQLite capability self-check and preflights Codex/OpenCode configuration;
6. acquires an installation lock, safely switches the program directory, and retains the old version for failure recovery;
7. migrates older memories, rebuilds the local index, and runs full diagnosis;
8. connects detected Codex and OpenCode installations through rollback-safe edits.

If a step fails, the installer attempts to restore the previous program and integration files. It does not delete private memory data after an installation failure.

</details>

## Download provenance and system warnings

Install only from this repository's GitHub Releases or the official scripts on this page. Current public packages provide:

- `checksums.txt` on the release, to detect download corruption or mismatch;
- `MANIFEST.tsv` inside the archive, to verify every extracted file;
- an SBOM and collected dependency licenses;
- GitHub build provenance, showing that the artifact came from this repository's workflow.

> [!WARNING]
> Current public packages do not have Apple Developer ID signing, notarization, or a Windows code-signing certificate. Windows may show an “Unknown publisher” or SmartScreen warning. Checksums and build provenance help verify a download but do not replace operating-system code signing. Do not bypass a warning if you cannot verify the source.

## File locations

| System | Program directory | Private data directory |
|---|---|---|
| macOS and Linux | `~/.local/share/engramark/` | `~/engramark/` |
| Windows | `%LOCALAPPDATA%\Engramark\` | `%USERPROFILE%\engramark\` |

Program and private data are deliberately separate. Reinstalling replaces the program without overwriting memories.

## Upgrade and reinstall

Run the same installation command to upgrade or reinstall. The installer validates the new program before switching the fixed program directory. If data preparation, smoke testing, or host integration then fails, it restores the previous program and integration files.

Restart Codex and OpenCode after an upgrade. An already-running task may continue to use the previous integration until its host restarts.

An upgrade does not overwrite:

- original memory files under `cards/`;
- durable data such as the ID state and pending transactions;
- the user configuration in `engramark.json`.

If the local-index format or runtime capabilities change, Engramark rebuilds the index from the original memory files.

## Common configuration

Configuration file locations:

| System | Configuration file |
|---|---|
| macOS and Linux | `~/engramark/engramark.json` |
| Windows | `%USERPROFILE%\engramark\engramark.json` |

Most users need only these settings:

| Setting | Default | Purpose |
|---|---:|---|
| `radar.budget` | `3` | Maximum automatic short hints for one request |
| `radar.cooldown_ttl_seconds` | `86400` | Wait before repeating the same memory in one session |
| `opencode.request_radar_enabled` | `false` | Enable automatic OpenCode request hints |

Restart Codex or OpenCode after a configuration change so the integration reloads it.

<details>
<summary><strong>Advanced retrieval settings</strong></summary>

| Setting | Default | Purpose |
|---|---:|---|
| `search.query_timeout_ms` | `500` | Time budget for one lookup |
| `search.high_threshold` / `medium_threshold` | `0.64` / `0.34` | High, medium, and rejection boundaries |
| `search.preview_max_bytes` | `800` | Body preview limit for the top high-confidence result |

Avoid changing these values unless you are deliberately revalidating retrieval behavior.

</details>

## OpenCode automatic hints

Natural-language saving and retrieval in OpenCode need no extra setting. Automatic request hints are disabled by default because a short hint may be stored in the conversation message's `system` field and become visible to the main model, title generation, or compaction.

After accepting that boundary, set `opencode.request_radar_enabled` to `true` and restart OpenCode. The currently validated OpenCode App version is 1.18.11. Unvalidated versions keep automatic hints disabled by default.

`allow_unverified_version` is only for temporary compatibility testing after an upgrade and should return to `false` immediately afterwards.

## Troubleshooting

### The assistant cannot save or retrieve after installation

1. Confirm that Codex or OpenCode was fully restarted.
2. Run the [full diagnosis](#verify-an-installation).
3. Confirm that the task was opened from the correct project directory.
4. Include the project, component, path, or decision topic in the question.
5. Remember that natural-language retrieval still works when OpenCode automatic hints are off.

### Codex cannot identify project scope

If Codex does not provide a reliable project root, enable a project override from a trusted project directory, then start a new Codex task.

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark host-setup \
  project-enable --project "$PWD"
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" host-setup `
  project-enable --project (Get-Location).Path
```

Replace `project-enable` with `project-disable` to remove the override. The command manages only Engramark's block in the project's `.codex/config.toml`. It stops instead of overwriting an existing entry with the same name.

### Diagnosis reports a pending transaction

Recovery inspects the transaction and safely completes any remaining steps. Running it again does not reapply work that already finished.

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark recover
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" recover
```

## Uninstall

macOS and Linux:

```sh
~/.local/share/engramark/bin/uninstall
```

Windows:

```powershell
& "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Engramark\bin\uninstall.ps1"
```

Uninstall removes the program and Engramark-managed Codex/OpenCode integration while always preserving private memories. There is intentionally no automatic delete-memories option. If the data is no longer needed, remove the private data directory manually after confirming a backup.
