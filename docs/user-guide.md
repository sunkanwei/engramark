# User Guide

**[English](user-guide.md) | [简体中文](使用指南.md)**

This guide is for people using Engramark for the first time. See [Install and Upgrade](installation.md) for setup, upgrades, configuration, and uninstall behavior, or [Architecture](architecture.md) for storage and retrieval internals.

## One thing to understand first

Engramark is not a separate note-taking app. After installation, keep talking to Codex or OpenCode as usual. The assistant saves something only when you clearly ask it to remember long term, then retrieves it when a later question makes it relevant.

Three points cover most daily use:

- **It does not record every conversation.** Ordinary questions, temporary progress, tool output, and command history are not saved.
- **State the goal naturally.** There are no required wake words, and you do not need to name Engramark, MCP, or any tool.
- **Memories stay local.** Codex and OpenCode can share the same collection, while project memories remain isolated from other projects.

## Complete your first workflow in five minutes

Finish the [installation](installation.md#install), fully quit and reopen Codex or OpenCode, then start a task inside a real project directory.

### 1. Save one project memory

Tell the assistant:

> Remember that pnpm is this project's package manager. Save it as a project memory.

The assistant should confirm the save and report the memory ID. This request explicitly selected project scope, so the memory is visible only inside the current project.

### 2. Ask naturally in a new task

Start a new task and ask:

> Do you remember which package manager this project uses?

The assistant should retrieve the saved content and answer `pnpm`. This checks installation wiring, saving, project detection, and retrieval in one short workflow.

### 3. Update the existing memory

Continue with:

> Update the package-manager memory to say this project uses pnpm 10.

The assistant should update the existing memory instead of creating a conflicting copy. Include the memory ID only if the target is ambiguous.

### 4. Remove the test content

If this was only a test, say:

> Delete that test memory.

Deleting an active memory requires another explicit confirmation. Its content is cleared, but the ID is never reused. Archive it instead if the content may be useful later.

## What to say in daily work

These are natural-language examples, not commands that must be copied exactly.

| Goal | Example request | What happens |
|---|---|---|
| Save a project convention | “Remember that this repository uses pnpm. Save it as a project memory.” | Keeps it only in the current project |
| Save a personal preference | “From now on, answer me in Chinese by default. Save it as a global preference.” | Keeps it across projects |
| Recall an earlier decision | “What checks do we run before a release?” | Finds relevant memories and answers when evidence is sufficient |
| Change old information | “Update the package-manager memory to use pnpm 10.” | Updates the original instead of creating a conflict |
| Review the collection | “Review my long-term memories for anything stale, conflicting, or waiting for confirmation.” | Returns a report without changing anything automatically |
| Stop using something for now | “Archive the old release-process memory.” | Keeps the content but removes it from daily retrieval and automatic hints |
| Clear content permanently | “Delete the old test-server memory.” | Identifies the target, then asks for deletion confirmation |

A save or edit becomes visible to later requests immediately. No restart is needed.

## What belongs in long-term memory

Save information that will still affect future decisions and would otherwise need to be explained repeatedly:

| Content | Example | Suggested scope |
|---|---|---|
| Project conventions | “This repository uses pnpm and does not commit an npm lockfile.” | Project |
| Architecture decisions | “Authentication belongs in the gateway; services do not parse tokens directly.” | Project |
| Important paths or aliases | “When the team says core, it means `packages/core/`.” | Project |
| Reusable workflows | “After a schema change, generate a migration and run compatibility checks.” | Project |
| Cross-project preferences | “Give me a proposal and wait for confirmation before high-risk edits.” | Global |
| Stable environment facts | “My primary development machine uses Apple Silicon.” | Global |

Avoid saving:

- temporary progress, one-task todos, and intermediate conclusions;
- ordinary facts that are always available from source code or authoritative documentation;
- large logs, complete conversations, build output, or unedited source material;
- unverified guesses, unless you explicitly want a candidate for later review;
- passwords, access tokens, private keys, or other credentials.

> [!WARNING]
> Memories are local, but retrieved content still enters the coding assistant's context. Local storage does not make passwords or keys appropriate memory content.

A good memory is self-contained and remains understandable later. Prefer “After a schema change, generate a migration and run compatibility checks” over “Do it this way next time.”

## Project or global scope

Ask one question: **should this still apply after switching to another project?**

- If not, use **project scope**. Repository conventions, directory aliases, architecture decisions, and project workflows normally belong here.
- If yes, use **global scope**. Personal communication preferences, cross-project habits, and stable environment facts normally belong here.

Every save must have an explicit scope. Project memories cannot be searched, read, changed, or deleted from other projects. If the current project cannot be identified reliably, a project save fails instead of silently becoming global.

## Automatic recall and explicit lookup

### Ask the real question first

You do not need to issue a separate “search memory” command. Ask the question you actually care about:

> What database migration process did we decide on?

Include a project name, component, path, technology, or decision topic when possible. A vague prompt such as “What did we do last time?” may not provide enough evidence. With no reliable match, the assistant should say that it is uncertain instead of presenting a weak association as a remembered fact.

If the assistant does not realize that you are asking about saved knowledge, add:

> Look in the long-term memories we saved earlier.

This is still ordinary language; no product or tool name is required.

### Automatic hints depend on the coding assistant

| Coding assistant | Default behavior |
|---|---|
| Codex | Related requests can receive a few short hints; full content is still read only when needed |
| OpenCode | Natural-language retrieval is available; automatic request hints are disabled by default |

Automatic hints are a convenience, not a guarantee that every related prompt will match. When an answer matters, explicitly ask about the earlier decision or preference. See [OpenCode automatic hints](installation.md#opencode-automatic-hints) for the privacy boundary and opt-in setting.

## Candidate memories are optional

A candidate is not a required step before every save. Use it only when you are unsure whether information deserves long-term retention:

> Save this as a candidate: releases may need a full performance test. I will confirm later.

Candidates stay out of daily retrieval and automatic hints. Later you can:

- say “Show me the long-term memories waiting for confirmation” to review candidates;
- say “Make the performance-test candidate an active memory” to move it into daily use;
- say “Discard the performance-test candidate” to clear it.

Engramark does not create candidates merely because something looks important, and it does not repeatedly ask whether to save ordinary conversation.

## Update, archive, or delete

- **The information remains valid but its details changed:** update the existing memory.
- **It is not useful now but may matter later:** archive it. The content remains, but daily retrieval and automatic hints ignore it.
- **It should no longer be retained:** delete it. Deletion requires explicit confirmation and leaves only the non-reusable ID.
- **A real outcome proves it correct or incorrect:** tell the assistant about the evidence. Trust changes only when concrete evidence exists.

Important paths, identity details, and long-term preferences can be locked when saved. A locked memory is protected from automatic trust reduction, but you can still explicitly update, archive, or delete it.

## Back up and restore

Do not copy a live database file. Use Engramark's consistent snapshot command.

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark backup /path/to/new-backup
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" backup "D:\Backups\engramark"
```

Before rollback, Engramark validates the snapshot and preserves the current state. IDs created after the snapshot are not reassigned.

macOS and Linux:

```sh
~/.local/share/engramark/bin/engramark rollback /path/to/snapshot --confirm
```

Windows PowerShell:

```powershell
& "$env:LOCALAPPDATA\Engramark\bin\engramark.exe" rollback "D:\Backups\engramark" --confirm
```

## If earlier content cannot be found

Check these items in order:

1. **Did you restart Codex or OpenCode after installation or upgrade?** An old task may still use previous wiring.
2. **Are you in the correct project?** A project memory is available only from its original project.
3. **Is the question specific enough?** Include the project, component, path, or decision topic instead of asking about “that thing from last time.”
4. **Is OpenCode merely running with automatic hints off?** That does not disable natural-language retrieval.
5. **Ask the assistant to look in saved long-term memories.** This rules out a missed recall intent.

If the problem remains, follow [Installation troubleshooting](installation.md#troubleshooting) to run diagnosis and check project wiring.

## Where the data lives

| System | Private data directory |
|---|---|
| macOS and Linux | `~/engramark/` |
| Windows | `%USERPROFILE%\engramark\` |

Original memories live under `cards/`, with one `.mem` text file per memory. `.mem` is Engramark's own readable format, not an industry standard. Ordinary users do not need to understand or edit it.

Program files live separately, so reinstalling or uninstalling the program does not automatically delete memories. See [Architecture](architecture.md#4-memory-files-and-states) for the format, lifecycle states, and manual repair boundary.
