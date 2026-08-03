# User Guide

**[English](user-guide.md) | [简体中文](使用指南.md)**

This guide is for people using Engramark for the first time. See [Install and Upgrade](installation.md) for setup, upgrades, and uninstall behavior, or [Architecture](architecture.md) for storage and retrieval internals.

## How you use Engramark

Engramark is not a separate note-taking app. After installation and a restart of Codex or OpenCode, keep talking to your coding assistant as usual. The assistant saves something only when you clearly ask it to remember long term. Later, it can receive a short hint automatically when the memory is relevant, or search explicitly when you ask.

Three points cover most daily use:

- **It does not record every conversation.** Ordinary questions, temporary progress, tool output, and command history are not saved.
- **Use natural language.** There are no required wake words, and you do not need to call MCP tools yourself.
- **Memories stay local.** Codex and OpenCode can share the same collection, while project memories remain isolated from other projects.

## Complete your first workflow in five minutes

Finish the [installation](installation.md#install), fully quit and reopen Codex or OpenCode, then start a task inside a real project directory.

### 1. Save one project memory

Tell the assistant:

> Remember that this project uses pnpm by default. Save this as a project memory.

The assistant should confirm the save and report the memory ID. This request explicitly selected project scope, so the memory is visible only inside the current project.

### 2. Retrieve it in a new task

Start a new task and say:

> Search Engramark: which package manager does this project use?

The assistant should search first, retrieve the relevant memory, and answer `pnpm`. This checks the host integration, save path, project detection, and retrieval in one short workflow.

### 3. Update it

Continue with:

> Update that memory to say this project uses pnpm 10 by default.

The assistant should update the existing memory instead of creating a conflicting copy. If the target is ambiguous, include the memory ID returned by the save.

### 4. Remove the test memory

If this was only a test, say:

> Delete that test memory.

Deleting an active memory requires another explicit confirmation. Its content is cleared, but the ID is never reused. If the content may be useful later, archive it instead.

## Daily operations

The phrases below are examples, not commands that must be copied exactly. Engramark interprets the intent of the full request.

| Goal | Example request | What happens |
|---|---|---|
| Save a project convention | “Remember that this repository uses pnpm. Save it as a project memory.” | Saves an active memory visible only in the current project |
| Save a cross-project preference | “From now on, answer me in Chinese by default. Save this as a global preference.” | Saves an active memory available across projects |
| Retrieve something explicitly | “Search Engramark for the release checklist.” | Searches a small result set, then reads only relevant details |
| Correct old information | “Update memory 18 to say…” | Changes the existing memory instead of creating a conflict |
| Review the collection | “Audit my memories for candidates, stale items, or possible conflicts.” | Returns a readable report without modifying anything |
| Remove something from daily use | “Archive memory 18.” | Keeps the content but removes it from default search and automatic hints |
| Clear content permanently | “Delete memory 18.” | Requests confirmation, then clears the content while retaining the used ID |

You do not need to restart after a save or edit. Changes made through the assistant or CLI are immediately available to later searches.

## What belongs in long-term memory

Save information that will still affect future decisions and would otherwise need to be explained repeatedly:

| Content | Example | Suggested scope |
|---|---|---|
| Project conventions | “This repository uses pnpm and does not commit an npm lockfile.” | Project |
| Architecture decisions | “Authentication belongs in the gateway; services do not parse tokens directly.” | Project |
| Important paths or aliases | “When the team says core, it means `packages/core/`.” | Project |
| Reusable workflows | “After a schema change, generate a migration and run compatibility checks.” | Project |
| Cross-project preferences | “Give me a proposal and wait for confirmation before high-risk edits.” | Global |
| Stable identity or environment facts | “My primary development machine uses Apple Silicon.” | Global |

Avoid saving:

- temporary progress, one-task todos, and intermediate conclusions;
- ordinary facts that are always available from source code or authoritative documentation;
- large logs, complete conversations, build output, or unedited source material;
- unverified guesses, unless you explicitly want a candidate for later review;
- passwords, access tokens, private keys, or other credentials. Memories are local, but retrieved content still enters the coding assistant's context.

A good memory is self-contained and remains understandable later. Prefer “After a schema change, generate a migration and run compatibility checks” over “Do it this way next time.”

## Project or global scope

Ask one question: **should this still apply after switching to another project?**

- If not, use **project scope**. Repository conventions, directory aliases, architecture decisions, and project workflows normally belong here.
- If yes, use **global scope**. Personal communication preferences, cross-project habits, and stable environment facts normally belong here.

Every save must have an explicit scope. Project memories cannot be searched, read, changed, or deleted from other projects. If the host cannot identify a reliable project root, a project save fails instead of silently becoming global.

## Automatic recall and explicit search

### Explicit search is the reliable path

When retrieval matters, tell the assistant to search Engramark. Include a project name, component, path, technology, or decision topic when possible:

> Search Engramark for the database migration checks.

A vague prompt such as “What did we do last time?” may not provide enough evidence. Engramark reports insufficient evidence instead of presenting a weak association as a remembered fact.

### Automatic hints depend on the host

- **Codex:** installed wiring can scan the local index when an ordinary request is submitted. A match provides at most three short hints; the assistant still fetches details only when needed. A miss adds no context.
- **OpenCode:** explicit MCP search is always available. The automatic request radar is disabled by default because its hints may be stored with conversation messages; enable it only after accepting that behavior.

Automatic recall is a convenience, not a guarantee that every related prompt will match. Ask for an explicit search when the answer matters.

## Candidate memories are optional

A candidate is not a required step before every save. Use it only when you are unsure whether information deserves long-term retention:

> Save this as a candidate: releases may need a full performance test. I will confirm later.

Candidates stay out of default search and automatic hints. Later you can:

- say “Audit my memories” to find pending candidates and other items that may need attention;
- say “Make memory 18 an active memory” to move it into daily use;
- say “Reject candidate memory 18” to clear its content.

Engramark does not create candidates merely because something looks important, and it does not repeatedly ask whether to save ordinary conversation.

## Correct, archive, or delete

- **The information remains valid but its wording or details changed:** update the existing memory.
- **It is not useful now but may matter later:** archive it. The content remains, but default search and automatic hints ignore it.
- **It should no longer be retained:** delete it. Deletion requires explicit confirmation and leaves only the non-reusable ID.
- **A real outcome proves a memory correct or incorrect:** tell the assistant about the evidence. Trust feedback is recorded only when concrete evidence exists.

Important paths, identity details, and long-term preferences can be locked when saved. A locked memory is protected from automatic trust reduction, but you can still explicitly update, archive, or delete it.

## Back up and restore

Do not copy a live SQLite cache. Use Engramark's consistent snapshot command.

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark backup /path/to/new-backup
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" backup "D:\Backups\engramark"
```

A snapshot contains the text source, durable ID state, and an integrity manifest, not the live cache. Before rollback, Engramark validates the snapshot and creates a safety snapshot of the current state. IDs created after the snapshot become tombstones, and the high-water mark never decreases.

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark rollback /path/to/snapshot --confirm
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" rollback "D:\Backups\engramark" --confirm
```

## If the assistant does not remember

Check these items in order:

1. **Restart the host.** After installation or an upgrade, an old task may still use the previous wiring.
2. **Ask for an explicit search.** OpenCode's automatic radar is off by default, and Codex automatic hints may miss a prompt with too few clues.
3. **Check the scope.** A project memory is available only from its original project.
4. **Use a more specific query.** Include the project, component, path, or decision topic instead of asking about “that thing from last time.”
5. **Run the full diagnosis.** It checks cards, durable ID state, caches, and retrieval capabilities.

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark diagnose --full
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" diagnose --full
```

If diagnosis reports a pending transaction, run recovery. It inspects the transaction and idempotently replays any steps that still need to complete.

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark recover
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" recover
```

If Codex does not provide a reliable project root, enable a project override from a trusted project directory, then start a new Codex task.

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark host-setup \
  project-enable --project "$PWD"
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" host-setup `
  project-enable --project (Get-Location).Path
```

Replace `project-enable` with `project-disable` to remove the override. The command manages only Engramark's block in the project's `.codex/config.toml`. It stops instead of overwriting an existing entry with the same name.

## OpenCode automatic hints

Explicit MCP search in OpenCode requires no extra setting. Enable `opencode.request_radar_enabled` only if you accept that short memory hints may be stored with OpenCode conversation messages, then restart OpenCode. The configuration file is `~/engramark/engramark.json` on macOS and Linux, or `%USERPROFILE%\engramark\engramark.json` on Windows.

The automatic radar handles only ordinary text submitted directly by the App editor. It skips slash commands, expanded attachments, synthetic text, and ignored text. It does not record chat or tool history and never creates memories. Unverified versions remain disabled by default; `allow_unverified_version` is only for temporary compatibility testing and should return to `false` afterwards.

## Data location and advanced maintenance

The source of truth lives in `~/engramark/cards/` on macOS and Linux, or `%USERPROFILE%\engramark\cards\` on Windows, with one `.mem` file per memory. `.mem` is Engramark's own readable text format, not an industry standard. Ordinary users do not need to understand or edit it.

The internal states mean:

| Internal state | User-facing meaning | Default search | Automatic hints |
|---|---|---:|---:|
| `candidate` | Waiting for review | No | No |
| `published` | Active memory used in daily work | Yes | Yes |
| `archived` | Retained but removed from daily use | No | No |
| `tombstone` | Content cleared; used ID retained | No | No |

Edit `cards/*.mem` directly only for manual repair or bulk maintenance. Files must use UTF-8 without BOM and LF line endings. Header fields, directives, and value ranges must remain valid; body order and Unicode content are preserved. After a manual edit, rebuild the derived cache:

```sh
~/.local/share/engramark/bin/engramark rebuild
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" rebuild
```

See [Architecture](architecture.md) for the complete format, invariants, and recovery model.
