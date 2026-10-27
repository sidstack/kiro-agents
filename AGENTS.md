# Repository Guidelines

Published Rust 2021 application: https://github.com/sidstack/kiro-agents. Read `docs/ARCHITECTURE.md`, `docs/SESSIONS.md`, and
`docs/GENERATION-SUPPORT.md` before changing lifecycle or Kiro-launch behavior. Update
`docs/FINDINGS.md` whenever a probe contradicts a doc claim; findings beat documentation.

## Current scope and structure

MVP v3 is the product iteration, not Kiro V3 engine support. The application creates
fresh Kiro V2 sessions and reattaches to live owned processes. Do not imply that
history resume, cross-generation migration, hooks or Kiro V3 launching are shipped.

- `src/host/` — lifecycle, registry persistence, locks, IPC and owned PTY supervision.
- `src/attach/` — native byte passthrough, resize and leader-key detach.
- `src/board/` — ratatui dashboard, directory/agent selection, rendering and terminal ownership.
- `src/kiro/` — Kiro argv, capability checks and bounded cancellable agent discovery.
- `src/main.rs`, `src/lib.rs`, `src/protocol.rs` — CLI, library and IPC definitions.
- `tests/` — Rust behavioral tests, Python PTY/packaging tests and `tests/fake-kiro`.
- `docs/` — architecture, findings, parity, release/security policies and MVP journals.
  `docs/journal/MVP-V3.md` records completed tasks and milestones.
- `.github/` — checks, security/release workflows, Dependabot and contribution templates.
- `scripts/package_release.py` — tagged binary packaging and checksums.
- `install.sh` — verified release installation, explicit versions/betas and uninstall.
- `probes/` — opt-in probe scripts and output; never shipped in the package.

Use `../pi-codex-agents` as a dashboard interaction/parity reference when available.
Preserve native Kiro ownership and the boundaries below when adapting an interaction.

## Hard rules

1. **Never write inside `~/.kiro/` or a project's `.kiro/`** except the single opt-in
   telemetry Hook file. Kiro owns agents, sessions, steering, hooks, MCP, skills,
   permissions, and settings.
2. **One live child per Kiro session id.** Check the session lock before spawning; never kill
   a foreign Kiro process.
3. **No terminal emulation, no output parsing, no ACP client.** While attached, forward bytes
   and resize only. The leader-key filter is the only exception.
4. **Never synthesize or rewrite a Kiro agent profile.** Validate with
   `kiro-cli agent validate` and surface Kiro's own errors.
5. **Never resume across generations.** V2 sessions on `--v2`, V3 sessions on `--v3`.
   `/upgrade-agent` is a user action, not ours.
6. **Never auto-approve tools.** `--trust-all-tools` only on explicit unattended sessions,
   always badged in the board.
7. **Never replay a prompt or re-run a tool** after a host crash or restart.
8. **Never log child environment, tokens, OAuth URLs, or transcript text.** Registry holds
   metadata only; drafts stay in their own files.
9. Optional features need a config toggle, cleanup on host close, enabled/disabled test
   coverage, and docs. Core imports the feature trait, never a concrete feature module.

## Build, test, and probe commands

Development requires Linux, Rust stable and Python 3.10+ with a PTY-capable environment.
`rust-toolchain.toml` selects stable with rustfmt and Clippy. End users installing the
native binary need neither Rust nor Python; Kiro CLI is installed/authenticated separately.

```sh
make check                                # fmt, strict Clippy, Rust tests/build, PTY and packaging tests
cargo install cargo-audit --version 0.22.2 --locked
make audit                                # RustSec audit; warnings fail
cargo run --bin kiro-agents -- --help
cargo install --path . --locked            # install from source
make probe                                # opt-in ignored real-CLI tests
```

Real-CLI probes must use a temporary `KIRO_HOME`/`KIRO_SESSIONS_DIR` and a scratch directory.
Never run them against the user's real `~/.kiro`, and never make paid model calls in CI.
Normal checks use fake Kiro and temporary state. Do not restart a live host merely to
install an updated frontend; host restarts remain explicit user actions.

## Style

Rust 2021, `cargo fmt` defaults, `clippy -D warnings`. `snake_case` modules and functions,
`PascalCase` types, `kebab-case` file and crate names. Strict typing: no `unwrap()` in
`src/` outside tests; use `anyhow` at entry points and `thiserror` in library modules.
`tracing` for diagnostics, never `println!` from the host. ESM-style small modules with
explicit boundaries: `kiro/` is the only crate-internal knowledge of Kiro argv and file
formats.

## Tests

Name tests `tests/<behavior>.rs` and unit tests in-module. Every lifecycle change needs a
behavioral regression test. Use `insta` snapshots for board rendering and `tests/fake-kiro`
for PTY integration. Preserve raw mode through board → native Kiro → board; release the board alternate
screen only after the host attach ACK so native primary-screen scrollback works, with
a bounded wait and recovery on failure. Forward wheel/PageUp bytes unchanged.
Keep directory/agent creation usable with selection alone. Agent discovery must stay
bounded and cancellable, reap its owned process group, and retain default/manual fallback.
Keep rendering event-driven and status refresh off the input path. Actual authenticated
agent-list schema and Guacamole visual acceptance remain manual validation areas; fake
CLI tests do not establish those claims. Keep real-CLI tests `#[ignore]`d and probe-gated.

## Commits and PRs

Branch from `main`, PR into `main`; use Conventional Commit subjects and PR titles
(`feat: add directory selection`, `fix: restore terminal modes`, `docs: clarify installation`).
Use `!` or a `BREAKING CHANGE:` footer for breaking changes. Squash merges preserve the
PR title for version calculation. Protected `main` requires `check` and `rustsec` checks
and resolved conversations. PRs state what changed, why, and which checks ran; include
`docs/FINDINGS.md` deltas when a probe changed an assumption.

Never commit real transcripts, `host.log`, Kiro auth state, child environments, generated
build output or scanner reports containing sensitive data. Follow `CONTRIBUTING.md`,
`CODE_OF_CONDUCT.md` and `SECURITY.md`; report vulnerabilities privately through GitHub.
The project is MIT licensed and independent of Kiro, Amazon, OpenAI and Pi.

## Versioning, releases and installation

Release Please owns semantic versions, Cargo.toml/Cargo.lock, the version manifest and
CHANGELOG.md. `feat:` bumps minor; `fix:` bumps patch. Breaking changes bump minor before
1.0 and major afterward. Docs/chore changes normally do not release the application.
Do not hand-bump versions, push release tags, add release override footers or upload local
builds. Merge the generated release PR to publish a tagged GitHub release and binary.

The release workflow runs checks and a strict RustSec audit, dispatches checks for its
bot-created PRs, and calls the binary workflow directly on release creation. It uses the
built-in GITHUB_TOKEN; no personal token or package-registry credentials are required.
Actions must retain permission to create PRs. Actions are pinned to immutable commits;
keep PR workflow tokens read-only and grant write permissions only to release jobs.

The current distribution target is Linux x86_64 GNU, built on Ubuntu 22.04 with a documented
glibc 2.35 baseline. Each release attaches `kiro-agents-vVERSION-x86_64-unknown-linux-gnu.tar.gz`
and `SHA256SUMS`. Archives include only the executable, LICENSE, README and CHANGELOG.
Packaging verifies tag/HEAD/Cargo/binary identity. Do not claim cross-toolchain reproducible
compiler output. No crates.io/npm package, macOS/Windows/ARM/musl binary is published.

Direct users to GitHub Releases and `README.md` installation instructions: verify the
checksum, extract, install to `~/.local/bin`, then run `kiro-agents doctor`. Source installation
uses `cargo install --path . --locked`. For workflow retry and detailed release policy, read
`docs/RELEASING.md`; use its existing-tag binary workflow instead of inventing a new release.

Branch beta workflows run full checks and audit before packaging, then rerun all
fake-Kiro PTY scripts against the extracted optimized binary. Betas use the next Cargo
patch plus `-beta.RUN.ATTEMPT`, recording the exact source SHA without committing version
changes. A separate publisher executes only trusted default-branch code and validates
artifacts; never execute branch code with its write token. Fork builds provide artifacts
only. Betas are prereleases and never replace latest stable. See `docs/RELEASING.md`.

## Security and OSS maintenance

`make audit` checks the complete lockfile with RustSec; CI runs it on changes, releases
and weekly. Dependabot groups Cargo and Actions version updates into one weekly PR;
review breaking changes and keep merging explicit. See `CONTRIBUTING.md` for dependency
squash titles and the separate release PR. Preserve secret scanning,
push protection, private vulnerability reporting and dependency alerts on GitHub.
Do not suppress advisories or weaken release checks to ship a binary.

`docs/SECURITY-REVIEW.md` records dated RustSec and Trivy dependency/secret scan evidence
and reproduction commands. Results are time-bound: rerun for new claims, record scope
and tool/database versions, and never describe a clean dependency scan as proof that the
application, operating system or separately installed Kiro CLI has no vulnerabilities.
Keep detailed policies and changing evidence in their linked documents rather than
copying scan counts or release versions into these repository instructions.

Session names prefer explicit wrapper overrides, then PID/cwd/fresh-lock-matched native
Kiro titles, then a bounded first-Prompt extractive title, then project/agent. Never guess
by newest session in a directory, write native titles, or parse terminal traffic for names.
`KIRO_AGENTS_AUTO_NAME=auto|kiro|off` controls read-only enrichment at host startup;
manual names remain available. See `docs/SESSION-NAMES.md` for bounds and compatibility.

Installer changes must retain checksum/archive/version verification and atomic replacement,
never require sudo or restart hosts. Tests use local mocked release downloads and a temporary
installation directory; keep the default latest channel stable, with explicit beta selection.

Optional workspace work lives on feat/tmux-workspaces: real external tmux owns panes,
windows, scrollback and shells. Assemble the backend through WorkspaceFeature; keep
session-host ownership independent. Never use the default/user tmux server or modify
~/.tmux.conf; use only the private wrapper-state socket. Closing panes/workspaces must
not stop managed Kiro sessions. See docs/WORKSPACES.md and docs/journal/MULTIPLEXER.md.
Workspace tests require tmux 3.2+ and use only temporary private sockets. Exercise
prefix keys through a real attached PTY client: tmux send-keys bypasses the client's
prefix parser. Keep the test in make check and the extracted-beta integration suite.

Next-batch plans live in docs/BACKLOG.md: native historical-session resume and local
fuzzy/full-text session quick-open, with separately verified V2/V3 store adapters.
These are backlog items only; do not include them in the current multiplexer batch.
Naming upgrade compatibility: custom labels are shared persistent metadata in private names.json,
not an RPC requirement or a reason to restart live hosts. Status enrichment must support
pre-naming hosts. Keep the actual v0.2.0 compatibility regression (synthetic old peer in
CI), unique unnamed-session labels, and version-independent rendering snapshots. Do not
claim automatic SQLite TUI titles until ownership mapping is verified; do not guess by cwd.

Attached frontend navigation uses Ctrl+G to return to the board. This explicit user
choice replaces Kiro's native Ctrl+G crew shortcut while attached. Tmux retains its
Ctrl+B prefix. Left Arrow always passes through: do not infer editor emptiness from
keystrokes or parse terminal output to implement an empty-input shortcut.

Frontend lifecycle: keep Crossterm's use-dev-tty backend and explicit terminal-hangup
exit checks; its mio backend can spin inside poll after PTY EOF/EIO. Tests must prove
closed pane frontends exit, not only that tmux removed a pane. The bounded CPU/PTY/host
EOF regression runs in make check and against extracted betas. Native title ownership
may use a bounded, start-time-checked descendant parent chain; main-thread children
lists alone miss Kiro's runtime-thread launcher chain. See docs/INCIDENT-2026-10-26-CPU.md.

Workspace primary-screen scrolling belongs to tmux's scrollback buffer: wheel and
PageUp enter/scroll copy mode, not prompt-history or injected page keys. Preserve
native mouse events for applications that request them and native page keys for
alternate-screen applications. Test actual viewport movement and no input leakage,
not only echoed key bytes. Do not change direct native attach passthrough.
