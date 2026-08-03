# Maintainer Release Guide

**[English](release-guide.md) | [简体中文](发布指南.md)**

This guide is for maintainers building, validating, and publishing Engramark
release candidates. Users should follow [Install and Upgrade](installation.md).

## Release model

Engramark ships one native executable per platform:

- macOS on Apple Silicon;
- macOS on Intel;
- Linux x86_64;
- Windows x86_64.

Each artifact must be built and executed on its own native runner.
Cross-compilation cannot replace the native capability probe, full test suite,
and installation lifecycle.

Program files and private data remain separate. A release archive contains
only replaceable program files, host adapters, default configuration,
documentation, licenses, and supply-chain metadata. It never contains real
memories, caches, or local state.

## Prepare the version

Before release:

1. Ensure the root `VERSION` and `rust/Cargo.toml` versions match.
2. Use the tag `v<version>`.
3. Confirm that the worktree contains only expected changes and no real
   memories, credentials, or local artifacts.
4. Synchronize user-visible behavior, supported boundaries, and documentation.
5. Explicitly review golden-contract changes and update the implementation,
   manifest, and documentation together.

## Build locally

Build only a target that the current machine can execute and validate natively:

```sh
python3 packaging/build_release.py --target macos-arm64
```

The builder performs a locked release build and capability probe, then
produces:

```text
dist/engramark-<version>-<target>.tar.gz
dist/engramark-<version>-windows-x86_64.zip
dist/checksums.txt
```

Each candidate contains:

- the native `engramark` or `engramark.exe` executable;
- default configuration, host adapters, and English and Chinese documentation;
- `SBOM.json`;
- upstream licenses for Rust dependencies and Unicode data;
- `MANIFEST.tsv` with the type, size, and SHA-256 of every file.

Linux artifacts are built on Ubuntu 22.04 and reject symbol requirements above
glibc 2.35. macOS artifacts declare a fixed 13.0 deployment target. That
declaration is not evidence of execution on a particular macOS version.

## Validate before release

Daily full suite:

```sh
python3 tests/run.py
python3 tests/test_repository_privacy.py
```

Non-CI 10,006-card scale check:

```sh
ENGRAMARK_SCALE_CARDS=10006 python3 tests/test_retrieval_scale.py
```

Static and supply-chain checks:

```sh
ruff check tests packaging
shellcheck install.sh bin/install.sh bin/uninstall bin/uninstall.sh
cargo deny --manifest-path rust/Cargo.toml check advisories licenses sources
git diff --check
```

Native installation lifecycle:

```sh
python3 packaging/build_release.py --target <current-native-target>
python3 tests/test_install_lifecycle.py
```

The lifecycle uses an isolated user directory to verify malicious-archive
rejection, installation, host wiring, MCP, hooks and radar, search, writes,
backup, upgrade reinstall, uninstall, and preservation of memory data after
uninstall.

## GitHub release candidate

Pushing a `v*` tag matching `VERSION` starts GitHub Actions, which:

1. Runs static, documentation, privacy, and supply-chain gates.
2. Runs the full suite, capability probe, package build, and installation
   lifecycle on four native runners.
3. Revalidates the same candidate archives on newer systems.
4. Produces a unified `checksums.txt`.
5. Generates GitHub build provenance for the artifacts.
6. Creates an unpublished GitHub Release draft.

Automation does not publish the version. A maintainer must confirm all four
builds, compatibility revalidation, checksums, provenance, and attachments
before manually publishing the draft.

## Integrity, provenance, and code signing

Current public distribution uses three layers:

- outer `checksums.txt` for downloaded artifacts;
- inner `MANIFEST.tsv` for every extracted file;
- GitHub build provenance showing that the artifacts were produced by this
  repository's workflow.

These measures are not operating-system code signing. Current public packages
do not have Apple Developer ID signing, notarization, or a Windows code-signing
certificate, so users may still see source or unknown-publisher warnings.
Release notes and installation documentation must state this boundary
accurately.

Platform code signing can be added later for devices governed by enterprise
policy. Until then, checksums and provenance must not be described as code
signing.

## Post-release checks

After publishing a Release, confirm that:

- default download links return the correct four artifacts and unified
  checksum file;
- installers parse the artifact names and select the current platform;
- GitHub build provenance is visible on the Release;
- `README.md` and the installation guide contain valid current commands;
- the tag, `VERSION`, Cargo package version, and artifact names match;
- no release fix remains only on `main`.

Engramark itself uses the MIT License. Every candidate must also carry the
dependency SBOM and collected upstream licenses.
