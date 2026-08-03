# Engramark

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="assets/logo-light.png" />
    <img src="assets/logo-dark.png" width="140" alt="Engramark" />
  </picture>
</p>

<p align="center">
  <strong>编码助手的本地长期记忆：多年积累数千条，只占几行上下文。</strong>
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="docs/使用指南.md">使用指南</a> ·
  <a href="docs/安装指南.md">安装与升级</a> ·
  <a href="docs/架构设计.md">架构设计</a> ·
  <a href="docs/测试与验收.md">测试与验收</a>
</p>

Engramark 为 Codex、OpenCode 等编码助手提供一份共用的本地长期记忆。持久内容以可读文本保存，检索使用可随时重建的本机索引，并通过 MCP 与宿主适配器接入不同工具。它没有常驻服务，检索不依赖 LLM，不需要云端账户，也不包含遥测。

![Engramark 只在相关时取回简短记忆提示](assets/hero-context.svg)

## 为什么选择 Engramark

- **上下文占用很小。** 本机雷达命中时，每次请求最多注入 3 条简短提示；不命中就是 0 token，正文只在确有需要时读取。
- **记忆始终由用户掌控。** 可读的文本卡片是唯一事实源，SQLite 只是派生缓存，损坏或过期后可以从真源重建。
- **不会暗中记录。** 只有用户明确要求长期保存时才写入。普通聊天、临时进度、工具输出和命令历史都不会被收集。
- **证据不足时会拒答。** 多路确定性检索经过融合与证据评分；弱匹配会被过滤，不会伪装成已经记住的事实。
- **崩溃后可以恢复。** 真源和索引更新使用可恢复、可幂等重放的事务，并通过跨进程锁和耐久替换保持一致。
- **项目记忆严格隔离。** 项目范围内容只在所属项目中可见；项目上下文不可靠时，绝不会悄悄降级为全局记忆。

## 它如何工作

1. **由你决定什么值得长期保留。** 直接告诉助手记住某个事实、决策、偏好、路径或可复用流程。Engramark 不依赖固定唤醒词，而是理解整句话的保存意图。
2. **相关记忆以短索引出现。** 本机雷达扫描当前请求，只注入符合严格字节预算的简短提示。
3. **正文按需渐进披露。** 助手先搜索，再通过 MCP 读取少量已确认相关的记忆详情。

候选记忆只用于用户明确要求“先存为候选”的场景。在用户确认前，它们不会进入日常搜索和雷达。重要内容可以锁定，避免被自动反馈降低可信度。

## 安装

当前发布目标包括 macOS（Apple Silicon 与 Intel）、Linux x86_64 和 Windows x86_64。只安装已经在对应原生平台完成 CI、能力探针和安装生命周期验证的产物。

macOS 与 Linux：

```sh
curl -fsSL https://raw.githubusercontent.com/sunkanwei/engramark/main/install.sh -o /tmp/engramark-install.sh
sh /tmp/engramark-install.sh
```

Windows PowerShell 5.1 或 PowerShell 7：

```powershell
$script = Join-Path $env:TEMP "engramark-install.ps1"
Invoke-WebRequest https://raw.githubusercontent.com/sunkanwei/engramark/main/install.ps1 -OutFile $script
& $script
```

安装包只包含一个内嵌 SQLite 的原生程序，不要求用户预装 Python、Homebrew、数据库或包管理器。重新安装和升级只替换程序，独立的记忆目录会保留。

当前公开包没有 Apple Developer ID 或 Windows 代码签名，因此 Windows 可能显示“未知发布者”或 SmartScreen 提示。请只从本仓库的 Releases 或上述官方脚本安装；安装器会校验发布页校验和与包内逐文件清单。路径、升级、卸载和信任边界详见[安装与升级](docs/安装指南.md)。

## 开始使用

重启已检测到的宿主后，直接用自然语言表达：

- “记住这个项目使用 API 24。”
- “以后这个仓库默认使用 pnpm。”
- “找一下发布检查清单相关的记忆。”
- “先存为候选，我之后再确认。”
- “归档记忆 18。”

保存与策展通过安装好的 MCP 工具完成。搜索只返回少量自然语言结果；助手确认相关后，才按编号读取正文。通过 MCP 或命令行完成的变更会立即进入新查询，无需重启宿主。

## 实测基线

以下是项目自己的回归结果，不是 LOCOMO 等公开记忆基准的成绩：

| 指标 | 结果 |
|---|---:|
| 合成长周期集合 | 2,000 张卡 |
| 全量索引重建 | 约 0.5 秒 |
| 热查询 p95 | 约 7 毫秒 |
| 金样 recall@5 | 1.0 |
| 无关查询拒答率 | 1.0 |
| 雷达误注入 | 0 |
| 项目隔离 | 通过 |
| 单次雷达输出 | 最多 3 条简短提示 |

可复现方法和发布前使用的 10,006 张卡验收见[测试与验收](docs/测试与验收.md)。

## 隐私与可靠性

- 程序和私人记忆分开存放；重新安装不会覆盖记忆，卸载也不会删除记忆。
- Unix 私有目录使用 `0700`，私有文件使用 `0600`；Windows 使用仅授权当前用户、SYSTEM 和 Administrators 的受保护 ACL。
- 显式搜索遇到缓存故障、锁超时或时间预算耗尽时会明确报错，不会伪装成空结果。
- 自动钩子采用失败开放策略：记忆不可用时，宿主请求仍会继续，只是不注入上下文。
- 一致备份复制文本真源和耐久编号状态，不直接复制正在使用的 SQLite；回滚前会先创建安全快照，编号高水位永不降低。
- 真实卡片、持久状态、缓存、日志和本机运行文件均被 Git 排除，并由仓库隐私测试守护。

## 配置

用户配置位于 `~/engramark/engramark.json`。常用选项：

| 配置 | 默认值 | 用途 |
|---|---:|---|
| `radar.budget` | `3` | 单次请求最多注入的简短提示数 |
| `radar.cooldown_ttl_seconds` | `86400` | 同会话、同记忆的冷却时间 |
| `opencode.request_radar_enabled` | `false` | 开启已验收版本的 OpenCode 请求雷达 |
| `search.query_timeout_ms` | `500` | 单次检索时间预算 |
| `search.high_threshold` / `medium_threshold` | `0.64` / `0.34` | 置信度与拒答阈值 |
| `search.preview_max_bytes` | `800` | 高置信第一名的正文预览上限 |

OpenCode 请求雷达默认关闭，因为短索引会写入该条用户消息的 `system` 字段，可能被主模型、标题生成或压缩流程看到。当前已验收的 OpenCode App 版本是 1.18.11；关闭雷达时，MCP 主动检索仍然可用。

## 文档

面向使用者：

- [使用指南](docs/使用指南.md)：日常记忆操作、作用域、备份、恢复、隐私和宿主行为。
- [安装与升级](docs/安装指南.md)：支持系统、可信安装、升级、目录和卸载。

面向维护者与贡献者：

- [架构设计](docs/架构设计.md)：核心不变量、存储、检索、并发、恢复、MCP 和宿主适配。
- [测试与验收](docs/测试与验收.md)：金样契约、黑盒测试、规模检查和原生 CI。
- [维护者发布指南](docs/发布指南.md)：构建、供应链检查、发布候选和 GitHub Release。
- [第三方组件说明](THIRD_PARTY_NOTICES.zh-CN.md)：依赖与 Unicode 数据的许可证信息。

## 当前边界

- 数据目录只支持本地文件系统；NFS、SMB、云盘同步目录和可移动介质不在一致性承诺内。
- OpenCode 请求雷达默认关闭并受版本门控；MCP 是稳定的主动接入路径。
- 当前使用确定性的词法检索，语义检索仍在路线图中。
- 发布包携带 SBOM、上游许可证、校验和、逐文件清单和 GitHub 构建来源证明，但当前公开包没有平台代码签名。

Engramark 采用 [MIT 许可证](LICENSE)。
