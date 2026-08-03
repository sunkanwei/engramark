# Third-Party Notices

**[English](THIRD_PARTY_NOTICES.md) | [简体中文](THIRD_PARTY_NOTICES.zh-CN.md)**

Engramark is a native Rust executable and does not bundle CPython. Its release
archives contain:

- Rust dependencies resolved by the committed `rust/Cargo.lock`. Exact names,
  versions, license expressions, and target information are recorded in
  `SBOM.json`; the corresponding upstream license and notice files are copied
  into `licenses/crates/` during packaging.
- SQLite, compiled into the executable by `libsqlite3-sys`. SQLite is dedicated
  to the public domain. The upstream statement shipped by that crate is kept in
  `licenses/crates/libsqlite3-sys-*/`.
- Unicode 16.0.0 case-folding data. Its Unicode Data Files and Software License
  is included as `licenses/unicode-license.txt`.

The top-level `LICENSE` applies only to Engramark itself. A dependency's own
license controls that dependency. The build rejects dependencies with a
missing or unapproved license expression, and every release package is checked
against its per-file manifest before installation.
