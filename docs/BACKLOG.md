# Next-batch backlog

Requested after the installer/dashboard release and initial tmux workspaces.
These are planning items only; do not implement them as part of the multiplexer batch.
The current product still supports fresh V2 launches and live-process reattach only.

## 1. Resume sessions from native Kiro stores

Goal: choose an earlier conversation and continue it from the dashboard, including
stored sessions with no live wrapper process. Support both Kiro V2 and V3 stores
through independently verified adapters.

### Discovery and compatibility milestone

- Probe the installed CLI's supported read-only listing and generation-specific resume
  commands using temporary HOME/Kiro/XDG roots and a scratch workspace. Authenticated
  acceptance requires an explicit user-controlled probe; never make paid calls in CI.
- Verify V2 and V3 storage paths, schemas, identifiers, lock ownership, titles, cwd,
  agent metadata and migration/version boundaries. Capture sanitized schema fixtures
  and record contradictions in FINDINGS. Do not equate engine generation with a file
  extension or assume SQLite versus JSONL alone establishes V2/V3 provenance.
- Implement separate versioned store adapters behind a shared read-only interface.
  Unknown schemas/generations get an explicit unsupported state, not guessed metadata.
- Build a compatibility matrix across supported Kiro versions and store generations;
  test adapters on both current and older fixture versions, corrupt/truncated files,
  missing stores, concurrent writers, permissions and explicit native path overrides.

### Dashboard and lifecycle milestone

- Distinguish live attachable sessions, inactive managed sessions and imported historical
  conversations. Historical means discoverable; it does not yet mean safely resumable.
- Selecting a live session reattaches to the existing owned child (or focuses its pane),
  preserving the one-attachment rule. Do not silently take over a foreign/live session.
- Selecting an inactive conversation shows its directory, agent, generation and reason
  it can/cannot resume, then starts the verified native resume command with its exact
  Kiro session ID and original cwd. Validate required profiles and available directories.
- Check native session locks before spawning. Never create two live children for one
  Kiro session ID, kill a foreign Kiro process or resume a conversation on another engine.
- Preserve Kiro's own errors, especially missing agent/auth/MCP/unsupported generation.
  No profile synthesis, automatic /upgrade-agent, tool approval or prompt/tool replay.
- User-selected native resume is distinct from automatic crash replay. An explicit user
  action may request resume; host recovery must never infer and execute that action.

Acceptance: fake native stores/CLI cover successful V2 and V3 resumes, same-generation
argv, cwd/profile handling, existing-live attachment, duplicate/racing locks, stale
locks, foreign processes, unavailable engine/profile, failed resume and crash recovery.
Real authenticated V2/V3 acceptance stays isolated and manually controlled.

## 2. Quick-open and full-text conversation search

Goal: type words or a fuzzy query into a dashboard quick-open/search surface, find
matching sessions across their actual conversation text, and choose one to attach or
resume. This means local search, not a dependency on an OpenSearch service.

### Search and indexing milestone

- Search titles, project paths, agent names and identifiers, plus actual user and
  assistant conversation text from every supported V2/V3 adapter. Define tool-output
  and archived/summary coverage explicitly rather than claiming an incomplete scan
  searches all text. Keep message-role/source provenance for snippets.
- Support word/full-text matches and fuzzy queries with predictable ranking: exact
  task/title matches, matching words and then approximate matches. Phrase/multi-word
  semantics, Unicode, case handling and ordering require behavioral fixtures.
- Prefer a bounded in-memory index initially. Any persistent index is a separate opt-in
  feature stored privately under wrapper state, with documented indexed content,
  retention, rebuild/delete controls and enabled/disabled tests. Never put transcripts
  into registry.json or diagnostics, upload conversation text, or modify Kiro stores.
- Read source text through schema-aware adapters only, never parse attached terminal
  traffic. Render bounded contextual snippets on demand; escape terminal controls and
  handle potentially sensitive conversation content deliberately.
- Build/search asynchronously with cancellation, incremental refresh and memory/I/O
  budgets. Show progress and partial coverage when limits or unreadable stores prevent
  a complete scan. Typing, editing and navigation must remain responsive.

### Open/resume milestone

- Result rows identify the matching session, project, generation, live/inactive state
  and a short source-context snippet; allow filtering without losing the current query.
- Enter on an owned live result attaches/focuses it; Enter on a resumable inactive result
  uses feature 1's explicit native resume path. Unsupported/locked results explain why.
- Apply the same selection in the regular dashboard and each tmux session pane. Closing
  the search/pane never deletes a conversation or stops its managed Kiro process.

Acceptance: cross-store text matches, fuzzy/word/phrase semantics, Unicode, ranking,
source snippets, terminal-control escaping, live versus inactive selection, generation
mismatch, index refresh/deletion, partial scans, cancellation, privacy toggles and memory
bounds. CI uses synthetic conversations and fake Kiro; no real auth/transcripts/model calls.

## Delivery order and gates

1. Native store probes + compatibility fixtures/matrix (both V2 and V3).
2. Read-only historical inventory and explicit generation-safe resume.
3. Search/index adapters and quick-open UI, reusing the same resume decision logic.
4. PTY/lifecycle regressions, dependency/security audit and a branch beta for user tests.
5. Merge/release only after compatibility and behavioral gates pass; unknown formats
   remain visibly unsupported. Never add history/resume claims to shipped documentation
   before both generation paths are actually implemented and verified.
