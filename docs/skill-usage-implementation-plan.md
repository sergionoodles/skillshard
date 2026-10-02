# Local skill usage tracking implementation plan

## Objective

Track observed skill activations by coding agent, including activations made by
subagents, and maintain an efficient local data source for counters, rankings,
last-used information, and trend charts.

Import existing agent histories rather than requiring hooks or modifying skills.
Keep collection independent of the GPUI views. The app imports history on startup
and while open; activity recorded while it is closed is imported next time it runs.
Scanning runs only during the app process lifetime, while tracking is enabled.

## Reference and existing application

SkillScout reads local transcripts and OpenCode's database, recognizes skill tools,
file reads, shell reads, and explicit attachments, and counts each skill once per
agent/chat. Its parsed-file JSON cache avoids reparsing unchanged files.

Reference implementation reviewed at commit
[`6b60d21`](https://github.com/flaviocopes/skillscout/tree/6b60d21e38a6d8bf0495f76da44ba8b004a4794e):

- `Skillscout/PromptLibrary.swift`: discovery, caching, Cursor/Claude/Codex parsers.
- `Skillscout/PromptLibrary+Tools.swift`: additional parsers and OpenCode queries.
- `Skillscout/Models.swift`: usage records and chat-level aggregation.

Do not reproduce its Codex child-session exclusion, name-only skill identity, or
Cursor use of transcript modification time as the activation timestamp.

Skillshard currently has filesystem discovery in `src/scan.rs`, agent directories
in `src/agents.rs`, skill/scope models in `src/model.rs`, and path overrides in
`src/paths.rs`. Usage is new application data, separate from CLI lockfiles and the
per-scope disabled-skill state in `src/state.rs`.

## Measurement contract

Expose separate metrics rather than using an ambiguous single "uses" count:

| Metric | Definition |
| --- | --- |
| Activations | Distinct observed activation events, including repeated loads in one session |
| Sessions using skill | Distinct agent/session pairs with at least one activation |
| Conversations using skill | Distinct agent/root-session pairs, folding linked descendants into the root |
| Subagent activations | Activations attributed to a confirmed child session or worker |
| Last used | Latest known activation timestamp, with timestamp quality available |

Use activations as the primary counter. Include subagents in totals by default,
with filters for all workers, main agents, and subagents. An agent application
(Codex, Claude Code, etc.) and a worker/session instance are different dimensions.
Capture worker name/role/model only when explicitly present in source metadata.

Evidence can be a dedicated skill tool, explicit skill attachment, a file-read
tool opening `SKILL.md`, or a recognized shell command reading it. Exclude mere
mentions, skill catalogs, file listings, comparisons, and edits. A file read is
evidence of loading instructions, not proof that the agent followed them.

Record outcome as `succeeded`, `failed`, or `unknown`. Exclude known failures from
default usage; show unconfirmed detections separately. When logs contain requests
and results, join them using the tool-call ID. A missing result stays unknown.
Explicit attachments count when the transcript records delivery into context.

Do not count every inherited skill as a new activation when a subagent starts.
Only count explicit loading/attachment evidence attributable to that worker.

## Local data source

Use one SQLite database owned by Skillshard, outside repositories and skill trees.
Proposed locations:

- Linux: `$XDG_DATA_HOME/skillshard/usage.sqlite3`, falling back to
  `~/.local/share/skillshard/usage.sqlite3`.
- macOS: `~/Library/Application Support/skillshard/usage.sqlite3`.
- Windows: `%LOCALAPPDATA%/skillshard/usage.sqlite3`.

Add a testable path resolver alongside existing helpers. Fail visibly when the
data directory cannot be resolved. Respect existing agent home overrides during
history discovery, including `CODEX_HOME` and `CLAUDE_CONFIG_DIR`.

Prefer `rusqlite` with bundled SQLite for predictable cross-platform behavior.
Prefer bounded periodic discovery initially; add a native watcher only after
measurement justifies it. These are dependency proposals: obtain approval before
adding dependencies, choose versions then, and install through `sfw` if available.

Configure WAL mode, foreign keys, a bounded busy timeout, and one serialized writer.
Use short transactions and prepared statements. GPUI reads snapshots from background
queries; parsing, SQL, and filesystem scans never run in rendering code.

### Logical schema

Use integer internal keys, UTC integer millisecond timestamps, and versioned
migrations. Keep indexed relational dimensions out of JSON blobs.

| Table | Important fields and purpose |
| --- | --- |
| `sources` | Adapter, normalized path, file generation, size/mtime, consumed byte offset, checkpoint fingerprint, parser version, scan/error status |
| `sessions` | Agent key, native session ID, optional parent identity, resolved parent/root IDs, worker role/name/model, project ID; unique agent/native session ID |
| `projects` | Normalized full project path and display label; do not identify by basename |
| `skills` | Stable internal ID, name, source metadata, scope/install identity, installed/historical status |
| `skill_aliases` | Skill ID, lexical/resolved path or namespaced name, scope/project/agent context, validity interval |
| `activation_events` | Session, optional worker identity within a session, tool-call/message ID, timestamp and quality, evidence kind, raw skill reference, resolved skill ID, outcome, deduplication key |
| `event_observations` | Event, source/generation, native record locator; preserves provenance across duplicate transcript exports |
| `daily_usage` | Optional later rollup of activations by UTC day, skill, agent, project, worker role, and outcome |

Index events for `(skill_id, occurred_at)`, `(occurred_at, skill_id)`, and
`(session_id, skill_id)`. Index session parent/root references and alias lookup
fields. Enforce uniqueness of deduplication keys and observation locators.
Keep unresolved references and sessions: attribution can improve after import.

Start with indexed event queries. Do not store permanent total counters that can
drift. Introduce daily rollups only if benchmarks demonstrate a need. Session and
conversation counts require distinct identities across the whole requested range;
summing daily distinct counts is incorrect. For local-calendar charts, bucket raw
timestamps in the selected timezone rather than summing UTC days incorrectly.

### Skill identity and resolution

Reuse scanner and lock metadata to register installed skill copies and aliases.
Resolve an absolute path or a path relative to the event's working directory first,
then its symlink target. Preserve lexical paths even when the file no longer exists.
Use scope, project, source repository, and source skill identifier where available.
Do not collapse different skills because their display names match.

Resolve bare names only within the agent/session's known available skills and
namespace. Ambiguous matches remain unresolved and are excluded from per-skill
rankings until resolved. Persist namespace prefixes instead of stripping them.
Record aliases before and after Skillshard move/disable operations so history
survives path changes. Reconcile externally changed installs without inventing
historical mappings that are not supported by evidence.

## Subagent attribution

Import child transcripts instead of skipping them. Preserve native session IDs,
parent IDs, spawn/tool-call IDs, and worker IDs wherever the format supplies them.
Some formats log multiple workers inside one session; preserve that worker identity
on the event rather than inventing a separate conversation.

Read spawn records to link returned child/worker IDs to parents. Resolve relationships
after both imports, so a child imported before its parent still works. Follow
parent chains with cycle detection. Do not infer parentage from timestamps or
matching project names alone. Unknown roles remain unknown, not "main agent".

Keep each activation attributed to its emitting worker. Parent and child sessions
using the same skill produce two activations but one conversation once linked.
For an unresolved parent chain, mark conversation counts provisional rather than
claiming a definitive root. Root reconciliation must refresh affected queries.

Deduplicate duplicated exports or parent mirrors by native event/tool-call identity
and emitting worker. Never deduplicate distinct child activations simply because
their skill and timestamp match. If a mirror lacks enough identity to prove it is
the same event, flag attribution uncertainty instead of silently merging it.

### Adapter rollout and format validation

First support Codex and Claude Code, including their child sessions. Then Cursor,
followed by OpenCode, Pi, Gemini CLI, Droid, and Amp as separate verifiable adapters.
Skillshard's installed-agent registry does not imply usage-tracking support.

Before implementing each adapter, inspect the installed agent version and obtain
sanitized sample transcripts covering a main session, a child session, spawning,
activation, and result records. Verify against official source/docs where available.
Do not read or copy private chats into the repository without authorization.

- Codex: active and archived JSONL sessions; validate current parent metadata,
  child source variants, tool-call IDs, explicit skill blocks, and shell reads.
  Do not retain SkillScout's early return for child sessions.
- Claude Code: main and nested subagent JSONL transcripts; validate session/worker
  IDs, spawn relationships, `Skill` calls, reads, shell commands, and slash expansion.
- Cursor: main and subagent transcript layouts, reads and manual attachments;
  use event timestamps when provided. If absent, retain unknown event time and an
  import timestamp separately. Do not assign file mtime as exact usage time.
- Other adapters: validate their current schema and relationship evidence first.
  OpenCode uses a read-only connection to its source database; never migrate it.

Tracking status should distinguish supported, no history found, permission denied,
unsupported format, and partial attribution. Unsupported coverage is not zero usage.

## Efficient and crash-safe ingestion

1. Discover history sources on startup and through bounded background polling.
   Track newly created directories and archived/moved files. A future watcher should
   only enqueue changed sources and still use periodic discovery for missed events.
2. Skip unchanged sources using size/mtime and stored generation/checkpoint metadata.
   For growing JSONL files, seek to the last committed complete-record boundary.
3. Parse bounded batches. Keep incomplete trailing lines uncommitted for the next
   pass. Malformed complete records advance with a diagnostic; excessive corruption
   pauses the source and surfaces an error. Bound record size and buffered bytes.
4. Commit events, result updates, observations, and byte checkpoints together.
   A crash before commit replays the batch safely through unique constraints.
5. Detect truncation/replacement using file identity where available and checkpoint
   fingerprints. Advance source generation and reconcile changed observations;
   do not blindly append duplicate events after a rewrite.
6. For whole-document JSON, reparse changed files and replace their observation set
   atomically. Retain an event if another source still observes it. Parser upgrades
   replay affected sources and replace obsolete detections through the same mechanism.
7. Query source databases with stable cursors and overlap reconciliation where their
   schema permits; use bounded rescans when updates cannot be detected reliably.
   Handle WAL changes and never write to agent-owned databases.
8. Notify GPUI once per committed batch, not once per event. Support cancellation,
   bounded queues, and progress so large historical imports do not freeze the app.

Store normalized usage metadata, not prompts, responses, or complete tool arguments.
History disappearing from disk does not erase already imported usage automatically.
Retain dated events only within the configured tracking window, pruning events and
their observations transactionally. Preserve the minimal session ancestry, source
checkpoints, and skill aliases needed to interpret retained events. No remote upload
is required.

## App lifetime, stopping, and crash recovery

Use a single app-owned ingestion service with explicit `stopped`, `starting`,
`running`, `stopping`, `rebuilding`, and `failed` states. Closing the app or disabling
tracking stops collection. Hiding or minimizing the window does not stop it while
the app remains open. Start, stop, and restart requests must be idempotent.

- Start: acquire an exclusive local writer lock held for the process/service
  lifetime, open the active database, recover its SQLite journal, validate schema
  and integrity, then discover sources and resume committed checkpoints. A second
  app instance must not start another collector or rebuild; expose a clear status.
- Stop: cancel polling/watchers, reject new queued work, and invalidate the service
  generation so stale tasks cannot publish results. Check cancellation between
  records and bounded batches. Finish an already executing short transaction or
  roll it back; discard uncommitted parsed work without advancing checkpoints.
- Join all service tasks and release reader/writer connections before releasing
  the writer lock. Do not detach importer tasks. Bound shutdown work through batch
  limits and I/O timeouts rather than trying to finish an entire source file.
- If forced termination occurs, rely on SQLite transaction recovery and the last
  committed checkpoints. Do not require a final save or WAL checkpoint for
  correctness. A crash after commit but before notification must still preserve
  the imported data and avoid duplicate activations on restart.
- Restart: create fresh cancellation state and a new service generation only after
  the old service has stopped. Reconcile current source identities and fingerprints
  before seeking to offsets; handle files changed, truncated, or archived while
  the app was closed. Use the current tracking window, not the old import cutoff.
- Corruption: stop ingestion, show a recoverable error and the Settings rebuild
  action. Do not silently delete the database or mark the scan successful.

## Settings and history window

Add `tracking_days` to existing preferences as a validated enum with exactly
**7, 15, 30, and 60 days**, defaulting to **30 days**. Label it "Tracking history".
This controls history import, local dated-event retention, and the maximum reporting
range, rather than being only a chart filter. Persist settings outside the usage
database so corruption or rebuilding cannot reset the user's choices.

Define the window as a rolling duration: `cutoff = now_utc - tracking_days * 24h`.
Capture one cutoff per scan/query operation. Charts may show shorter ranges but
cannot request history beyond the configured window. Refresh expiry while the app
is open and on startup; no expiry job runs while it is closed.

- Decreasing the window: serialize the change with ingestion, invalidate stale
  queries, prune expired events/observations, and refresh all metrics.
- Increasing it: schedule historical backfill from the new cutoff. Existing byte
  checkpoints cannot skip earlier records. Track imported coverage per source and
  parser version, replay relevant sources, and deduplicate against retained events.
  Explain missing coverage when original histories are unavailable.
- Source discovery may use mtime to prioritize reads but must not treat it as
  activation time. Read session headers and parent/spawn metadata outside the event
  window when needed to attribute a retained child activation; exclude older
  activations from counts and avoid storing old conversation content.
- Events without timestamps cannot be proven to fall within the window. Exclude
  them from dated history and counters, report the coverage limitation, and retain
  only bounded diagnostics needed to explain it.

## Settings: drop and rebuild local history

Provide a "Rebuild local history" action with confirmation explaining that it
replaces Skillshard's local usage data and rescans available agent histories for the
selected number of days. It does not alter agent transcripts, installed skills,
CLI lockfiles, or preferences. Show progress, disable repeated rebuild requests,
and surface errors with retry available. Allow rebuilding even if the active
database cannot be opened. When tracking is disabled, rebuilding is an explicit
one-shot scan while the app is open; it does not enable ongoing polling.

Use a fresh database generation rather than deleting rows from a potentially
corrupt database. Store the active generation in a small atomically replaced local
manifest; keep a durable rebuild marker outside the database:

1. Acquire the same exclusive writer lock, stop/join ingestion, cancel old queries,
   close connections, and clear UI usage snapshots. Persist the rebuild intent,
   selected window, and fresh generation ID before changing local history.
2. Create the fresh generation in the same application data directory. Initialize
   its schema and import from available histories with fresh checkpoints. Normal
   scans cannot race it. Do not reuse cached events or offsets from the old database.
3. Commit import checkpoints as usual. Closing the app cancels the rebuild safely;
   reopening resumes the recorded rebuild, provided tracking is enabled. If disabled,
   show "Rebuild paused" and require explicit resume for the one-shot scan.
4. Validate the fresh database, close its connections, and checkpoint its WAL before
   publishing. Atomically replace the active-generation manifest only after the
   fresh generation is ready. Persist metadata/manifests with the platform's durable
   file-write and atomic-replacement primitives, including directory synchronization
   where supported; test behavior on supported operating systems.
5. Open the new generation, refresh metrics, and resume polling only if enabled.
   Remove the replaced database and its WAL/SHM files once all old connections are
   closed, then clear the rebuild marker. Cleanup is limited to owned generation
   files identified by the manifest/marker, never arbitrary paths.
6. On restart, reconcile the marker and manifest before importing: resume an
   unfinished fresh generation, or finish cleanup if publication already succeeded.
   Never open mixed database/WAL generations or silently revert to stale counts.
   If building fails, retain the pending state and error for retry; do not present
   the old history as successfully rebuilt.

If the tracking window changes during a rebuild, stop it and reconcile the new
window before resuming. Fresh generations and checkpoints must remain restartable;
do not leave an unbounded collection of abandoned rebuild files.

## Proposed Rust module boundaries

- `src/usage/mod.rs`: public service/query API and domain types.
- `src/usage/storage.rs`: schema, migrations, transactional persistence, queries.
- `src/usage/ingest.rs`: discovery scheduling, checkpoints, bounded batch import.
- `src/usage/resolve.rs`: skill reference and session ancestry reconciliation.
- `src/usage/adapters/`: one module per supported agent; pure record parsers separated
  from filesystem/database readers.
- `src/usage/queries.rs`: counters, rankings, timeseries, and coverage snapshots if
  storage query logic grows enough to warrant a separate module.

Integrate through `src/lib.rs`, existing background executor patterns in
`src/ui/app.rs`, settings in `src/preferences.rs`, and small dedicated usage views.
Keep usage outside `Skill` initially: return usage snapshots keyed by stable skill
identity rather than making filesystem scanning depend on the database.

## Implementation phases

### 1. Validate formats and finalize contracts

Confirm Codex/Claude main and child layouts with sanitized fixtures. Finalize
activation outcome semantics, native identity rules, database location, and the
proposed dependencies. Review any public model changes before implementing them.

Acceptance: fixtures demonstrate main/child attribution and duplicate detection;
uncertain format assumptions are documented in adapter capabilities.

### 2. Build storage and first importers

Implement schema/migrations, repository-independent storage, pure Codex/Claude
parsers, child relationships, incremental import, skill resolution, and indexed
queries. Start import when tracking is enabled. No UI charts in this phase.
Implement app-lifetime ownership, exclusive writer locking, cancellation, and
transactional restart before connecting collection to the UI.

Acceptance: importing fixtures twice or restarting halfway yields identical counts;
child events stay attributed to children and participate in overall totals.

### 3. Add counters and coverage UI

Add activation count, last used, main/subagent breakdown, and tracking/import status.
Add agent/project/time filters, with usage distinct from public install counts.
Provide an opt-in local tracking setting and coverage information per adapter.
Include the 7/15/30/60-day selector and the crash-safe drop/rebuild action in Settings
in this phase, including progress, paused recovery, error, and retry states.

Acceptance: changing filters reads local queries without reparsing histories; main
and child usage can be inspected separately; unavailable tracking is explicit.
Changing the tracking window backfills or prunes correctly; rebuilding works with
a corrupt database and never changes original agent history.

### 4. Add rankings and trends

Add most-used skills for a selected range, activation/session/conversation metric
selection, and daily/weekly charts. Display timestamp/attribution limitations and
omit unknown-time events from dated buckets while reporting their count separately.
Benchmark before adding rollups.

Acceptance: chart totals match the selected metric and filter semantics, including
timezone boundaries, repeated usage, and linked subagents.

### 5. Expand adapters and operational controls

Add other agents individually and expand parser diagnostics. Keep the same lifecycle,
tracking-window, and rebuild contracts across adapters. Missing original history
must appear as incomplete coverage, not a successful complete reconstruction.

## Verification

Use the repository's Rust test layout and temporary files/databases. No production
build is needed. Run `cargo fmt --check`, `cargo check`, `cargo clippy --all-targets`,
and `cargo test`, checking repository conventions again at implementation time.
Run existing headless GPUI tests for affected interactions. Treat pre-existing
failures separately and do not report completion while relevant checks fail.

Required behavior tests:

- Dedicated activation, read, shell read, explicit attachment, failed call, pending
  result, mention-only text, and unrecognized record shapes.
- Main and nested child activation; child-before-parent import; inline worker IDs;
  missing parents; ancestry cycles; mirrored records; independent sibling uses.
- Append, incomplete final record, malformed record, truncation, replacement,
  archive relocation, parser replay, crash/retry, and transactional checkpoints.
- Same-name skills, symlinks, relative paths, namespaces, disabled/moved skills,
  missing files, and ambiguous unresolved references.
- Agent/project/role/range filters; distinct sessions and root conversations;
  unknown timestamps; timezone/day boundaries; corrected ancestry and outcomes.
- Source permission/schema errors, database contention, and migration rollback.
- Repeated start/stop, closing during parsing or commit, restart with pending tool
  results, forced termination before/after commit, and no work after shutdown.
- Tracking windows of 7/15/30/60 days, preference validation/defaults, exact cutoff
  boundaries, expiry, decreasing retention, and increasing-window backfill despite
  existing checkpoints. Older parent metadata must still attribute retained children.
- Rebuild from a corrupt database; cancellation; disabled-tracking one-shot rebuild;
  injected crashes at every marker/manifest/publication/cleanup boundary; retry;
  concurrent app instances; stale UI tasks; and untouched agent source files.

Benchmark a deterministic synthetic dataset of at least one million events.
Initial targets: no full reparse on an unchanged scan, bounded import memory,
and indexed counter/ranking queries under 100 ms on a documented development
machine. Record actual results; refine targets based on measurements.

For substantial UI phases, finish automated checks before final manual verification
of the native GPUI app. Check one main/child fixture flow, filter behavior, rankings,
and chart totals. Browser automation does not verify this native application.

## Decisions to confirm before implementation

- Approval to add the SQLite dependency and its selected version.
- Whether unknown-outcome observations should join the main counter. Proposed
  starting point: confirmed activations as the primary count, unconfirmed evidence
  visible separately, avoiding a claim of successful loading without evidence.

This document proposes implementation only. It adds no dependencies, runs no
migrations, imports no private histories, and changes no application behavior.
