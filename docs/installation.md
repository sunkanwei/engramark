# Install and Upgrade

**[English](installation.md) | [简体中文](安装指南.md)**

This guide is for Engramark users. It covers supported systems, trusted
installation, upgrades, paths, and uninstall behavior. Maintainers building
release candidates should use the [Maintainer Release Guide](release-guide.md).

## Supported platforms

Current release targets:

| System | Architecture | Native validation |
|---|---|---|
| macOS 14/15 | Apple Silicon | macOS 14 and 15 runners |
| macOS 15 | Intel | macOS 15 Intel runner |
| Ubuntu 22.04/24.04 | x86_64 | glibc 2.35 baseline and Ubuntu 24.04 revalidation |
| Windows Server 2022/2025 | x86_64 | installation compatible with Windows PowerShell 5.1 and PowerShell 7 |

An artifact is supported only after its native CI job completes the full test
suite, capability probe, package build, and installation lifecycle. A
cross-compiled binary or a declared minimum OS version is not a substitute for
native execution evidence.

## Install

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/sunkanwei/engramark/main/install.sh -o /tmp/engramark-install.sh
sh /tmp/engramark-install.sh
```

Windows x86_64, using either the built-in Windows PowerShell 5.1 or PowerShell
7:

```powershell
$script = Join-Path $env:TEMP "engramark-install.ps1"
Invoke-WebRequest https://raw.githubusercontent.com/sunkanwei/engramark/main/install.ps1 -OutFile $script
& $script
```

The archive contains one native executable with embedded SQLite. Python,
Homebrew, a database server, and a package manager are not required.

## What the installer does

The installer:

1. Selects the artifact for the current operating system and architecture.
2. Verifies its release SHA-256.
3. Rejects absolute paths, traversal, links, special files, case-folding
   collisions, and oversized archives.
4. Verifies the entry allowlist, size, and hash of every extracted file against
   `MANIFEST.tsv`.
5. Runs the SQLite capability self-check and preflights host configuration.
6. Acquires an installation lock, atomically switches the program directory,
   and keeps the old version for failure recovery.
7. Migrates legacy cards, rebuilds the derived cache, and runs full diagnosis.
8. Connects detected Codex and OpenCode installations through rollback-safe
   edits.

If a step fails, the installer attempts to restore the previous program and
host configuration. Private memory data is not deleted after an installation
failure.

## File locations

macOS and Linux:

```text
~/.local/share/engramark/   replaceable program
~/engramark/                private memories, state, configuration, and cache
```

Windows:

```text
%LOCALAPPDATA%\Engramark\   replaceable program
%USERPROFILE%\engramark\    private memories, state, configuration, and cache
```

Program and private data are deliberately separate. Reinstalling replaces the
program without overwriting memories.

## Download provenance and system warnings

Install only from this repository's GitHub Releases or the official scripts on
this page. Current public packages provide:

- `checksums.txt` on the release, to detect download corruption or mismatch;
- `MANIFEST.tsv` inside the archive, for verification after extraction;
- an SBOM and collected dependency licenses;
- GitHub build provenance, showing that the artifact came from this
  repository's workflow.

Current public packages do not have Apple Developer ID signing, notarization,
or a Windows code-signing certificate. Windows may show an “Unknown publisher”
or SmartScreen warning. Integrity and provenance metadata do not replace
operating-system code signing. Do not bypass a warning if you cannot verify the
download source.

## Upgrade and reinstall

Run the same installation command to upgrade or reinstall. The installer
validates the new program before switching the fixed program directory. If
data preparation, smoke testing, or host wiring then fails, it restores the
old program and host wiring.

Restart Codex and OpenCode after an upgrade. An already-running process may
continue to reference the replaced program path until the host restarts.

An upgrade does not overwrite:

- the memory source in `~/engramark/cards/`;
- durable state such as the ID high-water mark and pending transactions;
- the user configuration in `engramark.json`.

If the cache schema or runtime capability fingerprint changes, Engramark
rebuilds the derived cache from the text source.

## Verify an installation

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark diagnose --full
```

Windows:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" diagnose --full
```

Full diagnosis checks cards, the ID high-water mark, SQLite, both full-text
indexes, semantic hashes, the complete source-set hash, and runtime
capabilities.

## Uninstall

macOS and Linux:

```sh
~/.local/share/engramark/bin/uninstall
```

Windows:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\uninstall.ps1"
```

Uninstall removes the program and Engramark-managed host wiring while always
preserving private memories. There is intentionally no automatic
delete-memories option. If the data is no longer needed, the user must remove
the data directory manually after confirming a backup.
