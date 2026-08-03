# 第三方组件说明

**[English](THIRD_PARTY_NOTICES.md) | [简体中文](THIRD_PARTY_NOTICES.zh-CN.md)**

本文说明 Engramark 发布包中第三方代码和数据的许可证边界。普通使用者不需要进行额外操作；发布包已经携带相应声明文件。

Engramark 是原生 Rust 可执行文件，不携带 CPython。

| 组件 | 如何包含 | 许可证或声明位置 |
|---|---|---|
| Rust 依赖 | 由已提交的 `rust/Cargo.lock` 锁定，并编译进原生程序 | `SBOM.json` 记录名称、版本、许可证表达式和目标平台；上游文件位于 `licenses/crates/` |
| SQLite | 由 `libsqlite3-sys` 编译进可执行文件 | SQLite 属于公有领域；上游声明位于 `licenses/crates/libsqlite3-sys-*/` |
| Unicode 16.0.0 大小写折叠数据 | 用于稳定的本机文字规范化和检索 | Unicode 数据文件与软件许可证位于 `licenses/unicode-license.txt` |

根目录 `LICENSE` 只适用于 Engramark 自身，每项依赖仍受各自许可证约束。构建门禁会拒绝许可证字段缺失或未获允许的依赖，安装器会按逐文件清单检查每个发布包。
