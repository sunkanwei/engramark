# Testing and Validation

**[English](testing.md) | [简体中文](测试与验收.md)**

This guide is for contributors and maintainers. It answers two questions: what should run after a change, and what must pass before a version becomes public. All behavioral tests use temporary directories and never read real memories.

## Prepare the test environment

Testing from source requires:

| Tool | Purpose |
|---|---|
| Python 3 | Black-box tests, packaging, and installation lifecycle |
| Rust toolchain | Pinned by `rust/rust-toolchain.toml` for core builds and tests |
| Node.js 22 | OpenCode adapter tests |
| uv, ShellCheck, cargo-deny | Static and supply-chain checks used by the complete gate |

Users installing a release package do not need these development tools.

## Choose checks by change type

| Change | Minimum check | When to expand |
|---|---|---|
| Markdown only | `python3 tests/test_documentation.py` | Also run the relevant lifecycle when commands, installation steps, or release boundaries change |
| Rust core, memory format, or retrieval | `python3 tests/run.py` | Add the scale regression when recall or performance changes |
| Codex/OpenCode integration | `python3 tests/run.py` | Add the installation lifecycle when wiring changes |
| Install, upgrade, uninstall, or packaging | Native package build + installation lifecycle | Wait for every native CI platform before publication |
| Release candidate | Complete gate + 10,006-card scale check | Four-platform build and compatibility revalidation are mandatory |

## Daily full suite

```sh
python3 tests/run.py
```

The runner builds the current debug executable, pins every black-box test to that path, then runs `cargo test --locked` and seven black-box groups. A stale binary under `target/release` cannot be tested by accident.

| Group | Main coverage |
|---|---|
| Documentation | One top-level heading, relative links, language pairs, and critical instruction consistency |
| Core | Memory lifecycle, MCP, explicit-save rules, project isolation, feedback, privacy, and concurrent writes |
| Architecture | Transaction recovery, local index, IDs, backup, and migration |
| Codex | Automatic hints, summary budgets, cooldown, no session capture, and failure without blocking |
| OpenCode automatic-hint core | Strict protocol, budgets, reservations, durable cooldown, concurrency, negative samples, and performance gates |
| OpenCode adapter | Text provenance, command skipping, message events, failure cancellation, and real-core end to end |
| Host wiring | Idempotent Codex/OpenCode wiring, legacy upgrade, project `cwd`, plugin, and uninstall |

Repository privacy depends on real Git metadata, so CI and pre-release validation also run:

```sh
python3 tests/test_repository_privacy.py
```

## Golden compatibility contracts

`tests/golden/` is the compatibility contract frozen when the native port was completed. It covers normalization, memory format, hashes, query planning, retrieval, automatic hints, scanning, and error semantics.

Rust tests validate the golden file set and SHA-256 values against `manifest.json` before asserting behavior. A contract change requires explicit review and coordinated updates to the golden files, manifest, implementation, and user-facing documentation. The retired implementation and golden generator are not kept, so a changed implementation cannot redefine its own contract.

## Scale regression

The default scale regression uses 2,000 synthetic memories to approximate years of manual accumulation:

```sh
python3 tests/test_retrieval_scale.py
```

It checks target recall, unrelated-query rejection, project isolation, false automatic hints, retrieval and hint latency, and injected byte counts. Performance figures vary by machine; the script's gates are the acceptance criteria.

The non-CI pre-release check uses the same script with 10,006 cards:

```sh
ENGRAMARK_SCALE_CARDS=10006 python3 tests/test_retrieval_scale.py
```

## Package and installation lifecycle

Always select a target that the current machine can run natively. Do not rely on the packaging script's default.

| Current machine | Target argument |
|---|---|
| macOS Apple Silicon | `macos-arm64` |
| macOS Intel | `macos-x86_64` |
| Linux x86_64 | `linux-x86_64` |
| Windows x86_64 | `windows-x86_64` |

```sh
python3 packaging/build_release.py --target <current-native-target>
python3 tests/test_install_lifecycle.py
```

The lifecycle uses an isolated user directory to verify: malicious-archive rejection → installation → Codex/OpenCode wiring → MCP → automatic hints → retrieval → write → backup → upgrade reinstall → uninstall, with private data still intact after uninstall.

## Complete local gate

These commands mirror the current CI checks:

```sh
python3 tests/test_repository_privacy.py
python3 tests/test_documentation.py
uvx ruff==0.15.4 check packaging tests
shellcheck install.sh bin/install.sh bin/uninstall bin/uninstall.sh
git diff --check
```

```sh
cd rust
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cd ..
cargo deny --manifest-path rust/Cargo.toml check advisories licenses sources
```

## CI and release acceptance

Continuous integration runs the complete Rust and black-box suite, final-binary capability self-check, package build, and installation lifecycle in these native environments:

- macOS 14/15 arm64;
- macOS 15 Intel;
- Ubuntu 22.04/24.04 x86_64;
- Windows Server 2022/2025 x86_64, including Windows PowerShell 5.1 revalidation.

Linux release artifacts are built on Ubuntu 22.04 and may not require symbols above glibc 2.35; the same candidate is revalidated on Ubuntu 24.04. A candidate is not ready for public release until all four platform paths complete.
