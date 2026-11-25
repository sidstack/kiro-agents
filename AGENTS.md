# AGENTS.md — kiro-agents

Published Rust 2021 CLI (MIT, <https://github.com/sidstack/kiro-agents>) that keeps one
`kiro-cli chat --v2 --tui` child per managed session on its own PTY under a long-lived per-user
host, driven from a ratatui board. It supervises processes the user cares about, sits on their real
Kiro state, and pipes raw bytes into a live agent: a wrong change can orphan Kiro processes, leak
transcript material, or steal keystrokes. Shipped (Linux x86_64 binaries): fresh V2 launch,
live-process reattach, read-only history quick-open, `v1`-store resume. Not shipped: V3 launching,
cross-generation migration, hooks, per-workspace-store resume; `MVP vN` in `docs/journal/` is the
product iteration, never a Kiro engine version.

## Start here

1. `.agents/PLAN.md` then the newest `.agents/JOURNAL.md` entry — the only current status records,
   both **git-ignored**; a fresh clone has neither (see Rules). `docs/PLAN.md` is a dated
   2026-10-16 roadmap snapshot, not status; tracked evidence sits in the newest
   `docs/journal/<workstream>.md`.
2. Lifecycle, attach, IPC or terminal ownership → `docs/ARCHITECTURE.md`; session unit, stores and
   resume → `docs/SESSIONS.md`; Kiro argv, profiles, V2/V3 → `docs/GENERATION-SUPPORT.md` and
   `docs/KIRO-INTEGRATION.md`; tmux → `docs/WORKSPACES.md`.
3. A Pi-inspired interaction → `docs/PI-PARITY.md` first: `../pi-codex-agents` is the reference,
   native Kiro ownership is the boundary.

## Commands

Development needs Linux, Rust stable (`rust-toolchain.toml` adds rustfmt/clippy), Python 3.10+ and a
PTY; workspace tests need tmux 3.2+. End users need neither, and install/authenticate Kiro separately.

- `make check` — **gate**: fmt, `clippy -D warnings`, tests, `cargo build --locked`, 18 fake-Kiro
  PTY/installer/packaging scripts, `sh -n install.sh`. Done when it exits 0.
- `cargo test --test lifecycle` — one Rust behavioral test target while iterating.
- `python3 tests/board_smoke.py` — one board PTY scenario against fake Kiro.
- `cargo build --locked && ./target/debug/kiro-agents --help` — CLI surface smoke. Source install:
  `cargo install --path . --locked`; run `kiro-agents doctor` after any install or upgrade (release
  installs go to `~/.local/bin`; policy: `docs/RELEASING.md`).
- `make audit` — RustSec over the whole lockfile, on any `Cargo.lock` change and before a release
  claim. Needs `cargo install cargo-audit --version 0.22.2 --locked` (the CI pin).
- `make probe` — **opt-in, real Kiro CLI** (`cargo test --features probe -- --ignored`);
  excluded from the gate. Use a temporary `KIRO_HOME`/`KIRO_SESSIONS_DIR` and a scratch dir.

## Rules

- Never write inside `~/.kiro/` or a project's `.kiro/`; instead read Kiro's files and let Kiro own
  agents, sessions, steering, hooks, MCP, skills, permissions and settings. The single opt-in
  telemetry Hook file is the only exception and must stay removable.
- One live child per Kiro session id: check the native `<id>.lock` (PID + start time) before
  spawning; never signal or kill a foreign Kiro process — signal only an owned, unreaped child.
- Never emulate a terminal, parse Kiro output, or write an ACP client; while attached forward
  bytes and resize (`TIOCSWINSZ` + SIGWINCH) only — the frontend leader-key filter is the sole
  exception, and Left Arrow always passes through.
- Never synthesize or rewrite a Kiro agent profile; instead run `kiro-cli agent validate
  --path <file>` and surface Kiro's own error. `/upgrade-agent` is a user action, not ours.
- Never resume across generations: V2 sessions on `--v2`, V3 on `--v3`; this build launches
  `chat --v2 --tui` only and its V3 probes are open questions (`docs/GENERATION-SUPPORT.md`).
- Never auto-approve tools: `--trust-all-tools` only on an explicit unattended session, always
  badged on the board row. Never replay a prompt or re-run a tool after a host crash or restart;
  restore the row as failed and tell the user to review the transcript first.
- Never log or commit child environment, tokens, OAuth URLs, transcript text, `host.log` or
  scanner reports; the registry holds metadata only and drafts stay in their own files.
- Never `println!` from the host, never `unwrap()` in `src/` outside tests: `tracing`, `anyhow` at
  entry points, `thiserror` in library modules. Rust 2021, `cargo fmt` defaults,
  `clippy -D warnings`; `snake_case` functions, `PascalCase` types, `kebab-case` files and crates.
- Keep the seams honest: raw mode survives board → native Kiro → board and the board alternate
  screen is released **only after the host attach ACK** (bounded wait, recovery on failure), so
  native primary-screen scrollback works and wheel/PageUp bytes forward unchanged; discovery stays
  bounded, cancellable, reaped as an owned process group, with default and manual fallback; board
  rendering stays event-driven with status refresh off the input path.
- Optional features need a config toggle, cleanup on host close, enabled **and** disabled tests and
  docs; core imports the feature trait, never a concrete feature module (workspaces assemble
  through `WorkspaceFeature`, host ownership independent). Layout: `src/host/` lifecycle, registry,
  locks, IPC, PTY · `src/attach/` · `src/board/` · `src/kiro/` (only crate-internal Kiro
  argv/format knowledge) · `src/workspace/` · `tests/` (+ `tests/fake-kiro`) · `probes/` never packaged.
- Findings beat documentation: when a probe or real run contradicts a doc claim, append the dated
  observation to `docs/FINDINGS.md` in the same change instead of arguing from the doc.
- Never hand-bump a version, push a `vVERSION` tag, add a release-override footer or upload a
  workstation build; Release Please derives the version from the squash-merge title
  (`docs/RELEASING.md`). Never game the process — no ignored advisory, no weakened or skipped check
  to ship a binary, no probe against the user's real `~/.kiro`, no paid model call in CI; instead
  fix or pin, use fake Kiro with temporary state, keep real-CLI tests `#[ignore]`d and probe-gated
  (`docs/SECURITY-REVIEW.md`).
- Never use the user's default tmux server or edit `~/.tmux.conf`: only the private socket under
  `KIRO_AGENTS_HOME/workspaces`, and closing a pane or workspace never stops a managed Kiro session.
- Never assume `.agents/{SPEC,PLAN,JOURNAL}.md` exist: they are git-ignored, so a fresh clone or
  another worktree has no plan. Where they are absent, say so in the handoff, treat `docs/PLAN.md`
  as a dated snapshot, bootstrap from tracked docs and verified repository state without claiming
  to have recovered earlier notes, and never write live records into a tracked file to fix that.
  In linked worktrees the **primary checkout's** `.agents/` is canonical (`git worktree list`); the
  coordinator owns shared plan/journal edits, workers report results.
- Keep policies and changing evidence in their linked documents: never copy scan counts or release
  mechanics back into this file; point at `docs/RELEASING.md`, `docs/SECURITY-REVIEW.md`,
  `docs/SESSION-NAMES.md`, `docs/WORKSPACES.md`.
- Keep GitHub's guards on: secret scanning, push protection, private vulnerability reporting,
  dependency alerts, read-only PR tokens (`docs/SECURITY-REVIEW.md`).

## Protocol invariants

`src/protocol.rs` is a versioned JSONL contract over a private Unix socket; the handshake enforces
`PROTOCOL_VERSION`, never build identity (frames and limits: `docs/ARCHITECTURE.md`).
- A new request variant added **without** bumping `VERSION` is invisible to a running host: it
  cannot deserialize the frame and closes without replying, so the client sees EOF, not
  "unsupported" (`docs/FINDINGS.md` 2026-11-02, `tests/history_legacy.py`).
- Frontend-level capability (names, Ctrl+Q filtering, cooperative attachment moves) lives in shared
  state-store metadata or the frontend — it never needs a new host RPC or a host restart.
- The board never formats argv: it sends `Action::Resume { cwd, native_id }` and the host
  revalidates the id shape, lock and directory before spawning. Only a Kiro-shaped UUID reaches a
  command line. Locks are read, never written or removed; a limit-induced socket/IPC loss is an
  error, never a success. Keep the pre-naming compatibility fixture (synthetic old peer in CI) and
  version-independent rendering snapshots.

## Decision table

| If | Use | Not |
|---|---|---|
| Add or change a Kiro flag | `src/kiro/` argv builder, asserted in `tests/history_resume.rs` against `tests/fake-kiro` | hand-built argv in `src/board/` or `src/main.rs` |
| Return to the dashboard while attached | Ctrl+Q, filtered in the invoking frontend | Ctrl+G (Kiro's crew panel) or tmux's Ctrl+B prefix |
| Scroll a primary-screen workspace pane | tmux copy mode via wheel/PageUp | injecting PageUp/Down or arrow keys into Kiro |
| Publish a version | merge the generated Release Please PR; the squash title is the version input | `git tag`, edited `Cargo.toml`, local tarballs |
| Upgrade a running system | install the binary and relaunch the frontend | restarting a live host or its managed sessions |
| Name a session | F2 or `kiro-agents rename` into `~/.kiro-agents/names.json` | guessing from cwd, newest session, or terminal output |
| Scope the next batch | a `docs/BACKLOG.md` batch of its own | folding backlog work into the current batch |

## Working loop

1. Read the current records (`.agents/…`, else say they are missing) and the one contract doc
   for the area. **Done when** you can name the module you will edit and the invariant you must
   not break.
2. Reproduce against `tests/fake-kiro` with temporary `KIRO_AGENTS_HOME`/`KIRO_HOME`.
   **Done when** a failing `tests/<behavior>.rs` or PTY script shows the defect.
3. Implement in the owning module only. **Done when**
   `cargo clippy --all-targets --all-features -- -D warnings` is clean.
4. Add the regression: lifecycle/host → Rust test; board rendering → `insta` snapshot;
   terminal/PTY/keys → a fake-Kiro PTY script registered in the Makefile `check` target.
   **Done when** it fails on the pre-change code and passes now.
5. Run the gate. **Done when** `make check` exits 0; add `make audit` when dependencies moved.
6. Any claim about real Kiro behavior needs a probe: run `make probe` with temporary Kiro
   roots, then append the dated result to `docs/FINDINGS.md`. **Done when** the contradicting
   doc line is fixed in the same commit.
7. Commit (Conventional Commit subject), PR into `main`, naming which checks ran and which claims
   stay manual — authenticated agent-list schema, real resume, Guacamole/MobaXterm scrolling and
   clipboard are never inferred from fake-CLI green. **Done when** CI `check`, `rustsec` and
   `beta-integration` (required on protected `main`, read 2026-11-25) are green.

## Traps

- Frontend burns 100% CPU after the PTY closes — crossterm's mio backend retries terminal EOF/EIO
  inside poll — keep the `use-dev-tty` backend plus explicit terminal-hangup checks, and make
  `tests/frontend_lifecycle.py` prove the frontend exits, not that tmux removed a pane
  (`docs/INCIDENT-2026-10-26-CPU.md`).
- Kiro processes reparent to PPID 1 and spin — ownership checks looked only at main-thread children
  and missed Kiro's runtime-thread launcher chain — walk a bounded, start-time-checked descendant
  parent chain; never guess by cwd.
- Resume silently does nothing against an already-running older host — `VERSION` unchanged, so it
  cannot deserialize the frame and closes without replying — treat EOF as an unknown outcome, ask
  the user to check the session list, and never restart the host by itself.
- Wheel or PageUp cycles Kiro's prompt history in a workspace pane — a primary-screen pane with no
  native mouse request gives scrollback to tmux — route those keys to tmux copy mode and test real
  viewport movement, not echoed bytes. Likewise a prefix-key test that passes while real users cannot
  detach: `tmux send-keys` bypasses the client's prefix parser — drive prefix chords through a real
  attached PTY client.
- Installer fails with a missing asset just after a release — GitHub marks the release `latest`
  before the binary job finishes uploading — keep the bounded asset retry; never weaken checksum,
  archive or version verification (`docs/RELEASING.md`). A matching `--version` string does not
  prove which binary is installed — compare the release checksum.

- The picker misses deeply nested projects — only ~200 immediate children/40 rows used to be ranked;
  hidden, dependency, build and symlinked trees stay excluded on purpose — the recursive worker keeps
  every row, reuses a 24 h path cache, and `Ctrl+R` forces a rescan.
- "Already attached" when moving a session — attachment is exclusive terminal ownership of one shared
  registry, not two session stores — use `m` + confirm or `attach <ID> --move`: token-checked frontend
  lease release with a bounded wait, then ordinary Attach; never signal or kill the source, restart
  the host, or change the protocol for this. Keep old-peer refusal, the native child PID, and
  token-checked cleanup of successor registrations.
- `MVP v3` read as Kiro V3 support — `MVP vN` is the product iteration — never imply V3 launching,
  resume for every store, or hooks are shipped.

## Docs

| file | what it is | change it when |
|---|---|---|
| `.agents/SPEC.md`, `.agents/PLAN.md`, `.agents/JOURNAL.md` | **git-ignored** live records; never committed, published or packaged | every session, in the primary checkout |
| `docs/FINDINGS.md` | append-only dated probe evidence; findings beat documentation | every probe that contradicts a claim |
| `docs/PLAN.md`, `docs/BACKLOG.md` | tracked roadmap **snapshot dated 2026-10-16** (history, not status); tracked next-batch scope, as its own batch | re-baselining; scope change |
| `docs/journal/*.md` | per-workstream milestone logs, not one log: newest `LIVE-SESSION-NAVIGATION.md` (2026-10-30), then `MULTIPLEXER.md`, `MVP-V3.md` (2026-10-28) | each milestone |
| `docs/ARCHITECTURE.md` | architecture, IPC frames/limits, feature toggles, upgrade boundary | lifecycle or IPC |
| `docs/SESSIONS.md` | session unit, native stores, quick-open, resume rules | history/resume |
| `docs/GENERATION-SUPPORT.md`, `docs/KIRO-INTEGRATION.md` | V2 vs V3 table, unprobed questions; Kiro CLI surface, storage, hooks, env, keys | launch/generation; Kiro integration |
| `docs/SESSION-NAMES.md`, `docs/WORKSPACES.md`, `docs/RELEASING.md`, `docs/SECURITY-REVIEW.md` | naming precedence and scan bounds; tmux sockets and scroll ownership; release/beta/artifact/installer policy; dated scan evidence and advisory rules | naming; workspaces; release; dependency/security |
| `docs/RISKS.md`, `docs/DECISIONS.md`, `docs/PI-PARITY.md`, `docs/INCIDENT-2026-10-26-CPU.md`, `docs/MVP.md` | risk register, settled choices, Pi parity map, dated incident, dated MVP review | their own subject; `MVP.md` not edited |

House doc-contract exemption, recorded 2026-11-25: no `docs/SPEC.md` or `docs/JOURNAL.md` exists
and the live records are git-ignored under `.agents/`, so the canonical three-document contract does
not apply here; `docs/MVP.md` and `docs/journal/*` were **not** renamed pending an owner decision.

## Handoff

Update `.agents/PLAN.md` and append a dated `.agents/JOURNAL.md` entry: the commit, the evidence
paths, and what was **not** verified. If those files are missing, put the same content in the PR
description and say so. Contradicting findings go to `docs/FINDINGS.md` in the same commit. Never
end a task with a change the repo cannot reproduce, and never stage `.agents/`, transcripts,
`host.log`, Kiro auth state, child environments or scanner output.
