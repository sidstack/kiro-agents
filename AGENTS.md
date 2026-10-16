# Repository Guidelines

Planning-stage repository. Read `docs/ARCHITECTURE.md`, `docs/SESSIONS.md`, and
`docs/GENERATION-SUPPORT.md` before changing lifecycle or Kiro-launch behavior. Update
`docs/FINDINGS.md` whenever a probe contradicts a doc claim; findings beat documentation.

## Structure

- `docs/` — design, Kiro facts, V2/V3 matrix, phases, risks, pi-codex-agents parity.
- `src/` (planned) — `host/` lifecycle, persistence, locks, IPC; `session/` state machine and
  PTY supervision; `attach/` passthrough; `board/` ratatui UI; `kiro/` argv, profiles,
  history, generation; `hooks/` optional telemetry; `config.rs`; `protocol.rs`.
- `probes/` — P0 scripts and transcripts; never shipped in the package.
- `tests/` — unit + integration, plus `tests/fake-kiro` and `#[ignore]`d real-CLI probes.

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

## Build, test, and probe commands (planned)

```sh
rustup toolchain install stable          # one-time; Rust is not installed on this machine
cargo fmt --check && cargo clippy --all-targets -- -D warnings
cargo test
cargo test --features probe -- --ignored   # requires a real kiro-cli and a throwaway KIRO_HOME
make probe                                 # writes probes/out/ and docs/FINDINGS.md
cargo run --bin kiro-agents -- --help
```

Real-CLI probes must use a temporary `KIRO_HOME`/`KIRO_SESSIONS_DIR` and a scratch directory.
Never run them against the user's real `~/.kiro`, and never make paid model calls in CI.

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
for PTY integration. Keep real-CLI tests `#[ignore]`d and probe-gated.

## Commits and PRs

Branch from `main`, PR into `main`. Concise imperative subjects
(`Add V2 resume legality checks`, `Record P0 session-lock findings`). PRs state what changed,
why, and which checks ran; include `docs/FINDINGS.md` deltas when a probe changed a
assumption. Never commit real transcripts, `host.log`, or Kiro auth state.
