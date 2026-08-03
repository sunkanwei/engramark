# Engramark

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.png" />
    <source media="(prefers-color-scheme: light)" srcset="assets/logo-light.png" />
    <img src="assets/logo-dark.png" width="140" alt="Engramark" />
  </picture>
</p>

<p align="center">
  <strong>让编码助手跨任务记住真正重要的事，只在需要时取回。</strong>
</p>

<p align="center">
  <a href="https://github.com/sunkanwei/engramark/releases/latest"><img src="https://img.shields.io/github/v/release/sunkanwei/engramark" alt="最新版本" /></a>
  <a href="https://github.com/sunkanwei/engramark/actions/workflows/ci.yml"><img src="https://github.com/sunkanwei/engramark/actions/workflows/ci.yml/badge.svg" alt="自动化检查" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT 许可证" /></a>
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

Engramark 为 Codex 和 OpenCode 提供一份共用的本地长期记忆。你决定哪些内容值得保留；以后再遇到相关问题时，助手可以找回它们，而不需要把全部历史对话塞进上下文。

记忆以可读文本保存在自己的电脑上。Engramark 不需要云端账户，没有常驻服务，不依赖大模型完成检索，也不包含遥测。

![Engramark 只在相关时取回简短记忆提示](assets/hero-context.svg)

## 它解决什么问题

- **不再反复解释项目约定。** 技术选型、目录别名、架构决策和工作流程可以跨任务保留。
- **不会把所有历史都塞给模型。** 只有相关的少量提示会进入当前请求，完整内容按需读取。
- **不同项目不会互相串记忆。** 项目记忆只在所属项目中可见，个人偏好可以单独保存为全局记忆。
- **不会暗中记录。** 只有你明确要求长期保存时才会写入；普通聊天、临时进度和工具输出不会被收集。
- **数据仍由你掌控。** 原始记忆是本机可读文本，搜索索引损坏或过期后可以重新生成。

## 它如何工作

1. **你决定什么值得记住。** 直接告诉助手记住一个事实、决定、偏好、路径或可复用流程。
2. **Engramark 在本机保存和查找。** Codex 与 OpenCode 可以使用同一批记忆，不需要把记忆上传到独立云服务。
3. **助手只在需要时取回。** 相关请求可以获得几条简短提示；需要确认细节时，助手再读取完整内容。

不需要学习固定口令，也不需要手工操作记忆文件。候选、归档、备份等整理功能见[完整使用指南](docs/使用指南.md)。

## 安装

当前发布目标包括 macOS（Apple Silicon 与 Intel）、Linux x86_64 和 Windows x86_64。安装包包含内嵌 SQLite 的原生程序，不要求预装 Python、Homebrew、数据库或包管理器。

macOS 与 Linux：

```sh
curl -fsSL https://raw.githubusercontent.com/sunkanwei/engramark/main/install.sh -o /tmp/engramark-install.sh
sh /tmp/engramark-install.sh
```

Windows PowerShell 5.1 或 PowerShell 7：

```powershell
$script = Join-Path $env:TEMP "engramark-install.ps1"
Invoke-WebRequest https://raw.githubusercontent.com/sunkanwei/engramark/main/install.ps1 -OutFile $script
& "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File $script
```

> [!NOTE]
> Windows 命令可以从 PowerShell 5.1 或 7 中粘贴执行，并会使用系统自带的 Windows PowerShell 5.1 运行安装脚本。`-ExecutionPolicy Bypass` 只影响这一次安装进程，不会永久修改执行策略，也不能绕过单位设置的组策略。

> [!WARNING]
> 当前公开包没有 Apple Developer ID 或 Windows 代码签名。请只从本仓库的 Releases 或上述官方脚本安装；无法确认下载来源时，不要绕过系统警告。安装器会核对发布校验和和包内文件清单。

更详细的平台说明、升级、路径和卸载方法见[安装与升级](docs/安装指南.md)。

## 三分钟完成第一次体验

安装完成后，彻底退出并重新打开 Codex 或 OpenCode，然后在一个项目中开始新任务。

1. 对助手说：“记住：这个项目的包管理器是 pnpm，请保存为项目记忆。”助手应当确认保存，并返回记忆编号。
2. 新建一个任务，再问：“你还记得这个项目使用什么包管理器吗？”助手应当回答 `pnpm`。
3. 继续说：“把刚才关于包管理器的记忆改为：这个项目使用 pnpm 10。”助手会更新原记忆，而不是创建一条冲突内容。
4. 如果这只是测试，可以说：“删除刚才那条测试记忆。”删除正式记忆需要再次明确确认。

平时也可以直接问“发布前需要检查什么？”“之前定下的数据库迁移流程是什么？”或“帮我看看有没有过期、冲突或待确认的长期记忆”。是否调用记忆能力由助手判断，用户不需要说出产品名或工具名。

## Codex 与 OpenCode

| 使用方式 | Codex | OpenCode |
|---|---|---|
| 自然语言保存和找回 | 支持 | 支持 |
| 相关请求自动获得简短提示 | 安装后可用 | 默认关闭 |
| 自动提示关闭时 | 仍可自然提问并主动找回 | 仍可自然提问并主动找回 |

OpenCode 自动提示默认关闭，是因为提示可能随对话消息保存。了解这一影响并愿意开启时，再按照[使用指南](docs/使用指南.md#自动想起与主动查找)中的说明配置。

## 隐私与可靠性

- 程序和私人记忆分开存放；重新安装不会覆盖记忆，卸载也不会删除记忆。
- 私有目录和文件使用仅当前用户可访问的系统权限；缓存中的完整记忆仍建议依靠 FileVault、BitLocker 等磁盘加密保护。
- “本地”指保存和检索发生在本机；被找回的记忆会进入当前编码助手的上下文，并由该助手按自身服务边界处理。
- 找不到可靠证据时会明确表示没有足够匹配，不会把弱关联内容说成已经记住的事实。
- 记忆功能临时失败时，Codex 或 OpenCode 仍可继续正常处理请求，只是不会获得自动提示。
- 一致备份不会直接复制正在使用的数据库；回滚前会先保存当前状态。

<details>
<summary><strong>查看项目回归基线</strong></summary>

以下数据来自项目自己的合成回归，不是 LOCOMO 等公开记忆基准，也不是所有设备上的性能保证。实际时延会随硬件、系统和记忆内容变化。

| 指标 | 当前参考结果 |
|---|---:|
| 合成长周期集合 | 2,000 张卡 |
| 全量索引重建 | 约 0.5 秒 |
| 热查询 p95 | 约 7 毫秒 |
| 金样 recall@5 | 1.0 |
| 无关查询拒答率 | 1.0 |
| 自动提示误命中 | 0 |
| 项目隔离 | 通过 |

可复现方法和发布前使用的 10,006 张卡验收见[测试与验收](docs/测试与验收.md)。

</details>

## 文档

面向使用者：

- [使用指南](docs/使用指南.md)：第一次使用、日常记忆、作用范围、整理、备份和常见问题。
- [安装与升级](docs/安装指南.md)：支持平台、可信安装、升级、配置、路径和卸载。

面向维护者与贡献者：

- [架构设计](docs/架构设计.md)：数据、检索、并发、恢复、安全和 Codex/OpenCode 接入。
- [测试与验收](docs/测试与验收.md)：不同改动应该运行哪些检查，以及发布验收边界。
- [维护者发布指南](docs/发布指南.md)：版本判断、构建、四平台验证和 GitHub Release。
- [第三方组件说明](THIRD_PARTY_NOTICES.zh-CN.md)：依赖与 Unicode 数据的许可证信息。

## 当前边界

- 数据目录只支持本地文件系统；NFS、SMB、云盘同步目录和可移动介质不在一致性承诺内。
- OpenCode 自动提示默认关闭，并且只对已验收版本开放；自然语言主动找回仍然可用。
- 当前检索主要依赖确定性的文字与标识符匹配，语义检索仍在路线图中。
- 发布包带有校验和、逐文件清单、依赖清单和 GitHub 构建来源证明，但当前没有平台代码签名。

Engramark 采用 [MIT 许可证](LICENSE)。
