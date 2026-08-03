# Testing and Validation

**[English](testing.md) | [简体中文](测试与验收.md)**

Tests protect core behavior and high-risk data boundaries. All behavioral
tests use temporary directories and never read real memories.

## Daily suite

```sh
python3 tests/run.py
```

The runner builds the current debug executable, pins every black-box test to
that path through an environment variable, then runs `cargo test --locked` and
seven black-box groups. A stale binary that happens to exist under
`target/release` cannot be tested by accident.

| Group | Coverage |
|---|---|
| Documentation | Top-level Markdown headings, relative links, and English/Chinese file pairing |
| Core | Card lifecycle, structured MCP, multilingual explicit-save rules, project isolation, feedback, privacy, and concurrent writes |
| Architecture | Transaction recovery, cache, IDs, backup, and migration |
| Codex | Hooks, gists, complete-block budgets, per-card cooldown, SessionStart summaries, no session capture, and fail-open behavior |
| OpenCode radar core | Strict protocol, line and block budgets, temporary reservations, durable per-card cooldown, shared-anchor isolation, concurrency, 500 negative samples, and performance gates |
| OpenCode adapter | Text provenance, command skipping, system blocks, message IDs, durable events, failure cancellation, and real-core end-to-end behavior |
| Host wiring | Idempotent wiring for both hosts and Codex project `cwd`, legacy upgrade, minimal radar plugin, and uninstall |

Repository privacy depends on Git metadata and runs separately in CI:

```sh
python3 tests/test_repository_privacy.py
```

## Golden contracts

`tests/golden/` is the compatibility contract frozen when the native port was
completed. It covers normalization, card format, hashes, query planning,
search, radar, scanning, and error semantics. Rust tests first validate the
golden file set and SHA-256 values against `manifest.json`, then assert
behavior.

The retired implementation and golden generator are not kept in the
repository. A contract change requires explicit review and coordinated updates
to the golden file, manifest, implementation, and user-facing documentation.

## Optional scale regression

```sh
python3 tests/test_retrieval_scale.py
```

The default run creates 2,000 synthetic cards to approximate years of manual
accumulation. It checks target recall, unrelated-query rejection, project
isolation, false radar hints, search and radar latency, and injected byte
counts.

The non-CI 10,006-card release check uses the same script:

```sh
ENGRAMARK_SCALE_CARDS=10006 python3 tests/test_retrieval_scale.py
```

## Installation lifecycle

```sh
python3 packaging/build_release.py --target <current-native-target>
python3 tests/test_install_lifecycle.py
```

In an isolated user directory on the native platform, the lifecycle verifies:
malicious-archive rejection → installation → host wiring → MCP → hooks/radar →
search → write → backup → upgrade reinstall → uninstall, with private data
still intact after uninstall.

## Release gates

Static CI gates cover repository privacy, documentation, Python test drivers,
shell scripts, Rust formatting and Clippy, dependency advisories, licenses, and
sources.

The native matrix runs on macOS 14/15 for arm64 and x86_64, Ubuntu 22.04/24.04
x86_64, and Windows Server 2022/2025 x86_64. Each platform runs the complete
Rust and black-box suite, final-binary capability probe, package build, and
installation lifecycle. Linux release artifacts are built on Ubuntu 22.04 and
may not require glibc symbols above 2.35; the same candidate is revalidated on
Ubuntu 24.04.

Optional local static checks:

```sh
ruff check tests packaging
shellcheck install.sh bin/install.sh bin/uninstall bin/uninstall.sh
git diff --check
```
