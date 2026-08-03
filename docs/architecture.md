# Architecture

**[English](architecture.md) | [简体中文](架构设计.md)**

This document records architecture constraints implemented by the current
Engramark codebase. It is not a wishlist. See
[Testing and Validation](testing.md) for gates and measured results.

## 1. Design boundary

Engramark is a local, on-demand long-term memory layer with no daemon. Multiple
coding assistants share one card source, one derived search cache, and one MCP
interface.

The implementation and release matrix includes macOS arm64 and x86_64, Linux
x86_64, and Windows x86_64. An artifact is supported only after the
corresponding native CI job completes the full suite, capability probe,
packaging, and installation lifecycle. Cross-compilation is not a substitute
for native validation. The data directory is supported only on a local
filesystem. NFS, SMB, synchronized cloud folders, and removable media are
outside the consistency guarantee.

## 2. Core invariants

1. `.mem` cards are the only source of memory content. SQLite is a derived
   local cache.
2. State that can change long-term behavior must be durable. Diagnostic
   counters and session cooldown may be disposable.
3. Allocated IDs are never reused. Deleted content becomes a tombstone.
4. After a write API returns success, a new query must observe the new value.
5. Explicit retrieval reports failures. Automatic hooks fail open.
6. A version change that affects recall, filtering, or ranking invalidates the
   old cache.
7. A cache whose embedded SQLite or Unicode capability fingerprint does not
   match the runtime is rejected rather than downgraded in place.
8. A memory is written only after the user clearly expresses long-term
   retention intent in any language. This is a semantic decision, not a fixed
   keyword check. Host hooks retrieve accepted memories only; they never
   record a session or propose a candidate.

## 3. Data and directories

```text
cards/                         memory source
state/id-sequence              ID high-water mark
state/transactions/*.txn       recovery evidence for incomplete writes
state/locks/                   cross-process coordination
cache/memory.mcache            derived SQLite cache (v7)
~/.local/share/engramark/
  bin/engramark                one native executable shared by every host
```

`state/` is durable and cannot be deleted like a cache. `last-used` influences
freshness and ranking, with the card as authority. The first detail read of an
accepted card on a given day writes back at most once. Search summaries,
previews, and radar injection remain read-only: text that a model may have
seen is not treated as an explicit detail read. The first release does not
count accesses, avoiding a cache write transaction on every read.

## 4. Engramark card format v1

`.mem v1` is an Engramark-specific readable source format, not a general
interchange standard.

- UTF-8 without BOM and LF endings; the reader accepts a complete CRLF file.
- Body blank lines, trailing spaces, and Unicode are preserved.
- Only defined directives are accepted before the title.
- Entities are deduplicated by normalized key; order has no meaning.
- Trust is stored as fixed-point integers from 0 through 6 and displayed as
  half-steps from T0 through T3.
- Semantic hashing uses a versioned canonical encoding with field numbers,
  types, and length boundaries.
- `F` is a derived display value and is not part of the semantic hash. The
  source file also has a byte-level SHA-256.
- A format upgrade must be an explicit migration that first saves the original
  files and a unified diff.

State machine:

```text
candidate ──accept──> published ──archive──> archived
    │                    │                      │
    └──reject────────────┴──confirmed delete───┴──> tombstone
```

Every state remains under `cards/`. Accept, reject, update, archive, and delete
atomically replace the same file. Feedback writes the card and
`state/feedback/<id>.mark` in one recoverable transaction so a crash cannot
allow the same card to be scored twice in one day. Exact duplicates reuse the
existing ID. A known replacement uses `# supersedes @old-id`; every reference
must exist and cycles are forbidden.

## 5. MCP semantics and project isolation

MCP never accepts a complete `.mem` document. Save and candidate tools use
`title`, `body`, `entities`, `type`, and `scope`; locking is available only for
an active save. The core validates the fields, constructs a card, and
serializes the source format. An active save defaults to `I3/T3` and a
candidate to `I2/T2`. Public types are `fact`, `decision`, and `skill`. An
assistant may create a candidate only after the user explicitly asks for one.

Updates use field-level PATCH semantics: an absent field is preserved, while
an empty body or empty entity array explicitly clears that field. Only title,
body, entities, and type may change. The core preserves ID, state, source,
lock, scope, importance, trust, access date, validity, and replacement
relationships. A no-op update does not write the source, commit a transaction,
or increment the cache generation.

MCP search accepts one non-empty natural-language `query`, searches active
memories only, and returns at most five items. Pagination, state ranges,
project parameters, and score explanation remain CLI-only. The first final
high-confidence result may include a body preview of at most 800 UTF-8 bytes;
other results use summaries capped at 160 Unicode code points. SessionStart
`top --human` always uses short summaries and never inherits search preview.
MCP and radar output use natural-language “memory ID + title” lines. Card
headers, `I/T/F`, raw JSON, and internal exception stacks do not enter the
conversation.

Scope is an isolation boundary. With a known project, only global memories and
memories from that project are visible. Without a known project, only global
memories are visible. Search, radar, reads by ID, feedback, updates, and every
lifecycle operation apply the same visibility rule. Other projects are
filtered before recall. After a project card is rejected or deleted, its
tombstone keeps a path-free scope identifier so the ID does not become visible
across projects.

Project context is resolved, in order, from the unique file root in MCP
`roots`, a process working directory with a project marker, or a managed
`mcp_servers.engramark.cwd` entry in trusted Codex project configuration.
`/`, the user home, common broad directories, the system temporary directory,
the program directory, and the memory data directory are not projects. If
context is unknown, a project-scoped write fails and never becomes global.
`engramark host-setup project-enable --project <directory>` can install an
explicit Codex project override; `project-disable` removes only that managed
block.

The server explicitly negotiates MCP `2025-06-18` and `2025-11-25`. For an
unknown version it returns the latest supported server version rather than
echoing the client blindly. Tools provide Chinese titles, strict JSON Schema,
and accurate side-effect annotations. Input and business errors are
self-correctable tool errors; unknown tools and protocol failures use JSON-RPC
errors. Transport logs record request metadata and byte counts, never the
query, title, body, entities, feedback reason, or full result.

## 6. Concurrency and durability

- A query holds a shared `cache.swap.lock` from database open through close.
- A mutation first acquires exclusive `mutation.lock`, then exclusive
  `cache.swap.lock`.
- Lock order is fixed and every lock has a timeout.
- Each MCP call opens SQLite briefly; no connection remains open across calls.
- A full rebuild creates the database in a private temporary directory on the
  same filesystem and acquires the exclusive swap lock only for final
  replacement.

Cross-process coordination uses Rust standard-library file locking: `flock` on
Unix and `LockFileEx` on Windows. Lock files reject symbolic links and Windows
reparse points. Timeout values are bounded.

Temporary files are created privately next to the target, written and synced,
then atomically replaced before the parent directory is synced. macOS adds
`F_FULLFSYNC`. Windows uses protected DACLs, `MoveFileExW`, and
`FlushFileBuffers`. SQLite is fixed to `journal_mode=DELETE`,
`synchronous=FULL`, `foreign_keys=ON`, `trusted_schema=OFF`, and `mmap_size=0`;
macOS also enables `fullfsync`. Fault injection demonstrates process-crash
recovery. It is not an end-to-end power-loss certification for every hardware
and filesystem combination.

## 7. Recoverable transactions

Every mutation:

1. Parses and validates the complete post-change card graph.
2. Acquires both exclusive locks in the fixed order.
3. Writes a self-checking transaction journal with UUID, before and after
   hashes, and a recovery payload.
4. Atomically replaces the source file and syncs the file and parent.
5. Updates the ordinary tables, both FTS indexes, radar, cache generation, and
   `applied_ops` in one SQLite transaction.
6. Uses the SQLite commit as the API write linearization point.
7. Deletes the journal and syncs its parent directory.

Recovery matrix:

| Source | Cache | Action |
|---|---|---|
| Old | Old | Operation never started; remove the journal |
| New | Old | Replay the cache from source |
| New | New | Operation completed; remove the journal |
| Old | New | Repair the cache to match the source and report it |
| Other | Any | Stop automatic recovery and preserve evidence |

A multi-file transaction recovers against the complete state vector. A partial
source state is completed from the hash-verified payload. UUIDs and
`applied_ops` make replay idempotent.

## 8. Single-executable runtime

Codex, OpenCode, hooks, MCP, and CLI all invoke `bin/engramark` from the current
release. Public archives contain a Rust executable built with the pinned
toolchain and embedded SQLite. They do not use the user's Python installation,
perform runtime downloads, rebind a runtime, or rely on a Python capability
fingerprint.

Public installation puts the program in `~/.local/share/engramark/` and private
data in `~/engramark/`. Reinstall first runs the binary self-check, atomically
switches the program directory while retaining one rollback version, migrates
cards, and rebuilds the cache. A failure restores the old program and wiring.
Host configuration is preflighted before any edit.

Cache v7 metadata records:

- card format and cache schema versions;
- schema, query-planner, normalization, tokenizer, and radar-compiler versions;
- exact SQLite version, compile-option summary, and probe results for STRICT,
  FTS5, trigram, contentless-delete, and contentless-unindexed;
- Unicode data version, complete source-set hash, build UUID, generation, and
  completion marker.

If a derived cache has an old capability fingerprint, Engramark rebuilds it
from the cards. A cache is not portable across machines. The portable boundary
is the text source or a controlled snapshot, from which the destination
installation rebuilds its own index.

## 9. Retrieval cache

Ordinary tables use STRICT mode, separate integer columns, and constraints.
Cache metadata independently records:

- card format and cache schema versions;
- schema, query-planner, normalization, tokenizer, and radar-compiler versions;
- exact SQLite version, compile-option summary, and capability results;
- complete source-set hash;
- cache generation, build UUID, completion marker, and effective date.

The Unicode FTS index stores complete titles and bodies. The trigram index
stores titles, entities, anchors, URLs, paths, and code identifiers without
copying the entire long body.

Default limits:

- 2 MiB per card;
- 4,096 characters per query;
- candidate pool of 80;
- 500 ms query budget;
- at most five detail cards per call and 2,000 bytes returned per card.

An explicit query fails on timeout or cache unavailability rather than
returning a fabricated empty result. Full diagnosis compares the effective
card set, ordinary table, both FTS indexes, semantic hashes, and FTS internal
integrity.

## 10. Radar and host adapters

The radar is a self-checking, sectioned bytecode BLOB in SQLite. It records a
magic value, format, compiler and normalization versions, state and edge
counts, section lengths, and SHA-256. An unknown required section invalidates
the radar; only unknown optional sections can be skipped.

- A Codex hook can scan the radar while the user submits input and inject short
  natural-language memory hints.
- OpenCode installs a minimal request-radar plugin that is disabled by default.
  When explicitly enabled, it reads active memories only and never records
  sessions, tools, commands, or candidates.
- OpenCode radar is validated only against App 1.18.11. It waits for the
  durable `message.updated` event before committing cooldown. If the message is
  not stored or another plugin rewrites the block, the short reservation is
  cancelled or expires.
- An OpenCode hit is stored in the user message's `system` field. The request
  itself does not enter Engramark. Radar state stores only a session hash and
  the IDs actually reserved or shown.
- Both hosts share the same executable, cards, and cache.

The core creates every radar line. A line is capped at 360 Unicode code points
and 900 UTF-8 bytes and can include a first-paragraph gist of at most 120 code
points. The complete injected host block is capped at 1,200 UTF-8 bytes. Host
adapters wrap, validate, and pass through this output without resummarizing or
truncating it. Cooldown applies per card only after actual display. Cards
outside the byte budget and other cards sharing an anchor are not cooled down.

Automatic hooks catch failures and continue the host flow. Explicit MCP calls
return visible errors. This difference is intentional failure semantics.

## 11. Security, backup, and Git

- Unix private directories use `0700` and card, state, and cache files use
  `0600`. Windows uses a protected DACL allowing the current user, SYSTEM, and
  Administrators.
- Release archives reject absolute and traversal paths, links, special files,
  case-folding collisions, and oversized contents. After extraction,
  `MANIFEST.tsv` verifies the allowlist, size, and SHA-256 of every file.
- `checksums.txt` detects transport corruption but is not, by itself, publisher
  identity. The tag workflow creates GitHub build provenance for all four
  native artifacts and an unpublished draft. Current public packages may be
  published manually after all native validation succeeds, but they do not
  have Apple Developer ID or Windows code signing, so operating systems may
  still display source or unknown-publisher warnings.
- The cache contains full memory content. At-rest protection depends on system
  disk encryption such as FileVault.
- A consistent backup first recovers pending transactions, holds the mutation
  lock, and copies only cards, ID state, and a manifest.
- A live SQLite file is never copied directly. Diagnosis, backup, and rollback
  use controlled interfaces.
- Rollback first saves the current state and never decreases the ID high-water
  mark.
- Git excludes real cards, state, caches, logs, locks, local runtimes, and
  installation records. Retired historical directories such as `raw/` remain
  ignored so old data cannot enter the repository.

Automated tests protect the repository privacy boundary, but `git add -f` must
never be used to bypass the ignore rules.
