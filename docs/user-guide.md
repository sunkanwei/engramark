# User Guide

**[English](user-guide.md) | [简体中文](使用指南.md)**

This guide covers daily Engramark use and maintenance. For setup, upgrades,
and uninstall behavior, see [Install and Upgrade](installation.md). For the
underlying design, see [Architecture](architecture.md).

## Where memories live

The memory source is `~/engramark/cards/`, with one `.mem` file per card. This
is Engramark's own readable card format, not an industry standard. Ordinary
users do not need to understand or edit it. Candidates, active memories,
archived memories, and tombstones share the same directory; the first line
records the state:

```text
@8 fact candidate I2 T2 F1.0 2026-01-01
= Example entity
~ self:agent
# format 1
A self-contained memory title.
Optional body text.
```

| State | Meaning | Default search | Radar |
|---|---|---:|---:|
| `candidate` | A memory the user explicitly asked to hold for review | No | No |
| `published` | An active, accepted memory | Yes | Yes |
| `archived` | Content retained but removed from daily use | No | No |
| `tombstone` | Used ID retained without the deleted content | No | No |

IDs are never reused. Gaps and tombstones are therefore expected.

## Daily operations

Speak directly to an assistant connected to Engramark. A write occurs only
when you clearly express long-term retention intent. Engramark does not use a
fixed keyword list or restrict the language of the request; it interprets the
meaning of the whole sentence. Ordinary chat, temporary progress, and tool
output are not recorded, and the assistant should not repeatedly ask whether
to save them.

- “Remember…”, “save this for later”, “make a note”, “from now on…”, and
  “don't forget…” save an active memory and receive a short confirmation.
  Important paths, identity details, and long-term preferences can be locked.
- “Find the memory about…” returns short natural-language results before
  details are retrieved by ID.
- “Save this as a candidate” is the only workflow that creates a candidate.
- “Show the candidates” lists memories waiting for review.
- “Accept memory 8” turns a candidate into an active memory.
- “Reject memory 8” clears a candidate's content and leaves a tombstone.
- “Archive memory 8” retains the content but removes it from default search and
  radar.
- “Delete memory 8” removes active content after a second explicit
  confirmation; the ID remains as a tombstone.

MCP and CLI changes synchronize the cache immediately. Codex and OpenCode do
not need to restart before a new search can see the change.

Every save or proposal has an explicit scope:

- `global`: identity, preferences, paths, or workflows that apply across
  projects.
- `project`: facts, decisions, or workflows belonging only to the current
  project.

A project memory cannot be searched, read, or changed from another project.
If the host cannot provide a reliable project directory, a project-scoped
write fails instead of silently becoming global.

If Codex does not provide a project root to MCP, run this once in a trusted
project directory:

```sh
~/.local/share/engramark/bin/engramark host-setup \
  project-enable --project "$PWD"
```

Remove that project override with:

```sh
~/.local/share/engramark/bin/engramark host-setup \
  project-disable --project "$PWD"
```

The command manages only the `mcp_servers.engramark.cwd` block in the
project's `.codex/config.toml`. It stops rather than overwriting an existing
entry with the same name. Open a new Codex task for the change to take effect.

## Advanced maintenance: edit card files

Direct editing of `cards/*.mem` is recommended only for manual repair or bulk
maintenance. Follow these rules:

- Use UTF-8 without BOM and LF endings; the reader also accepts complete CRLF
  files.
- The first line must contain the ID, type, state, importance, trust, and date.
- Trust accepts only `0`, `0.5`, `1`, `1.5`, `2`, `2.5`, and `3`.
- `=`, `~`, and `#` before the title are structural directives; unknown
  directives are rejected.
- Body text after the title is preserved byte-for-byte with respect to blank
  lines, trailing spaces, and Unicode.
- Unicode normalization is used for retrieval and does not rewrite body text.
- Entities are deduplicated by normalized value. Entity order has no meaning;
  body order does.

Rebuild the cache after a manual edit:

```sh
~/.local/share/engramark/bin/engramark rebuild
```

This is the consistency boundary between manual source edits and the system's
derived index.

## Diagnose and recover

```sh
# Quick diagnosis
~/.local/share/engramark/bin/engramark diagnose

# Full cache and business-invariant checks
~/.local/share/engramark/bin/engramark diagnose --full

# Regenerate the cache from the card source
~/.local/share/engramark/bin/engramark rebuild

# Inspect and recover pending transactions
~/.local/share/engramark/bin/engramark recover
```

Explicit retrieval reports corrupt databases, lock timeouts, and manual
conflicts instead of presenting them as “no match.” Automatic hooks fail open:
they skip injection and write diagnostics without blocking the host.

## Back up and roll back

Do not copy an active SQLite cache directly. Create a consistent snapshot:

```sh
~/.local/share/engramark/bin/engramark backup /path/to/new-backup
```

A snapshot contains only:

- `cards/`;
- `id-sequence`;
- `manifest.json`.

Roll back with:

```sh
~/.local/share/engramark/bin/engramark rollback /path/to/snapshot --confirm
```

Before replacement, rollback validates the manifest, card count, ID high-water
mark, and complete source-set hash, then creates a safety snapshot of the
current state. IDs created after the snapshot become tombstones, and the
high-water mark never decreases.

## Codex and OpenCode

- **Codex:** a hook can scan the precompiled radar during user input and inject
  a small number of relevant, natural-language memory hints.
- **OpenCode:** installation adds a minimal request-radar plugin, but it is
  disabled by default. MCP search remains available. If you accept that short
  indexes will be stored with OpenCode conversation messages, set
  `opencode.request_radar_enabled` to `true` in
  `~/engramark/engramark.json` and restart OpenCode. Disabling it also requires
  a restart. Unverified OpenCode versions remain disabled by default;
  `allow_unverified_version` is only for temporary compatibility testing and
  should return to `false` afterwards.
- Both hosts use the same cards, cache, and native executable.

Radar hints include a bounded first-paragraph gist after the title and match
reason. The complete injected block has a strict byte limit. During explicit
`memory_search`, only the top final high-confidence result can receive a
longer preview; other results and SessionStart still use short summaries.
Hints and previews do not update `last-used`. Only an explicit detail read does.

OpenCode automatic radar handles only ordinary text submitted directly by the
App editor. It skips slash commands, expanded attachments, synthetic text,
and ignored text. It never records chat or tool history and never creates an
active or candidate memory. A failure merely skips injection and does not
affect the original message or explicit MCP.

## Git and privacy

Real cards, state, caches, logs, and local runtime files are excluded by
`.gitignore`. The retired `raw/` directory remains ignored to keep historical
session data out of the repository. Never use `git add -f` to bypass these
rules.

Before making a repository public:

```sh
cd /path/to/engramark-source
git status --short --ignored
git check-ignore cards/0001.mem state/id-sequence cache/memory.mcache
python3 tests/test_repository_privacy.py
```

## Uninstall

Uninstall removes the program and host wiring while preserving all memories:

```sh
~/.local/share/engramark/bin/uninstall
```

Engramark intentionally has no automatic delete-memories option. If the
private data is no longer needed, remove `~/engramark/` manually after
confirming a backup. Reinstallation reconnects the preserved data.
