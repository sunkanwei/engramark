# Third-Party Notices

**[English](THIRD_PARTY_NOTICES.md) | [简体中文](THIRD_PARTY_NOTICES.zh-CN.md)**

This document describes the licensing boundary for third-party code and data in Engramark release packages. Users do not need to take additional action; the packages carry the relevant notice files.

Engramark is a native Rust executable and does not bundle CPython.

| Component | How it is included | License or notice location |
|---|---|---|
| Rust dependencies | Resolved by the committed `rust/Cargo.lock` and compiled into the native program | `SBOM.json` records names, versions, license expressions, and targets; upstream files live under `licenses/crates/` |
| SQLite | Compiled into the executable by `libsqlite3-sys` | SQLite is dedicated to the public domain; the upstream statement lives under `licenses/crates/libsqlite3-sys-*/` |
| Unicode 16.0.0 case-folding data | Provides stable local text normalization and retrieval | The Unicode Data Files and Software License is included as `licenses/unicode-license.txt` |

The top-level `LICENSE` applies only to Engramark itself. Each dependency remains governed by its own license. The build rejects dependencies with missing or unapproved license expressions, and the installer verifies every release package against its per-file manifest.
