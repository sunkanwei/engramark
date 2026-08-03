# 第三方组件说明

**[English](THIRD_PARTY_NOTICES.md) | [简体中文](THIRD_PARTY_NOTICES.zh-CN.md)**

Engramark 是原生 Rust 可执行文件，不再携带 CPython。发布包包含：

- 由 `rust/Cargo.lock` 锁定的 Rust 依赖。`SBOM.json` 记录准确名称、版本、
  许可证表达式和目标平台；构建安装包时会把上游许可证与声明文件复制到
  `licenses/crates/`。
- 由 `libsqlite3-sys` 编译进可执行文件的 SQLite。SQLite 属于公有领域，
  该 crate 随附的上游声明保存在 `licenses/crates/libsqlite3-sys-*/`。
- Unicode 16.0.0 大小写折叠数据，其 Unicode 数据文件与软件许可证保存在
  `licenses/unicode-license.txt`。

根目录 `LICENSE` 只适用于 Engramark 自身；每项依赖仍受各自许可证约束。
构建门禁会拒绝许可证字段缺失或未获允许的依赖，安装器会按逐文件清单复验
每个发布包。
