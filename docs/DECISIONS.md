# Decisions log

| # | Decision | Rationale | Status |
| --- | --- | --- | --- |
| 1 | Standalone supervisor, not a Pi extension | Kiro is a closed binary with no extension API; a Pi-hosted Kiro board would need a VT emulator to render a foreign TUI | locked |
| 2 | Rust: `portable-pty` + `ratatui` + `tokio` | matches Kiro's stack, single binary, no Node runtime dependency; reuse pi-codex-agents' *design*, not its code | locked |
| 3 | New directory `~/Projects/kiro-agents` | sibling of `pi-codex-agents` | done |
| 4 | **Native Kiro TUI only.** No ACP client, no terminal emulation, no re-rendering, no scraping | the requirement is the same experience as launching `kiro-cli chat` yourself, just supervised | locked |
| 5 | Attach = host-owned PTY + raw byte pipe + resize | the only mechanism that keeps the native TUI intact *and* keeps the process alive after the terminal closes | locked, weakest point (see RISKS #3) |
| 6 | Our entity is a **Session**; Kiro keeps the word *agent* | avoid collision with `.kiro/agents` profiles and `--agent`; a session = agent profile + directory + Kiro chat session + generation + live process | locked |
| 7 | Sessions honor native `.kiro` fully: launch Kiro **in** the project directory, never synthesize profiles or sandbox `KIRO_HOME` | project `.kiro/agents`, steering, hooks, MCP, skills, `AGENTS.md`, `permissions.yaml` must apply exactly as Kiro applies them | locked |
| 8 | Every Kiro agent is spawnable, resolved per directory: built-ins (`kiro_default`, `kiro_planner`, `kiro_help`), workspace `.kiro/agents/`, global `~/.kiro/agents/`, legacy Amazon Q paths, `.json` and `.md` profiles | parity with Kiro's own picker; `kiro-cli agent list` is the authoritative resolver, filesystem scan is fallback | locked |
| 9 | Support **both** Kiro V2 and V3, generation recorded per session, cross-generation resume refused | Kiro states V2/V3 session formats are incompatible and V2 cannot open a V3 session; `/upgrade-agent` migrates profiles, not sessions | locked |
| 10 | Kiro's own multi-agent features stay inside Kiro (`/spawn`, subagents, `Ctrl+G` crew monitor) | our value is cross-directory persistent sessions, not a second orchestration layer | locked |
| 11 | Status inferred from child liveness + opt-in Hooks + `.lock`/`.jsonl` + idle timer | no runtime API to ask; Hooks are Kiro's sanctioned event channel | locked |
| 12 | Never write to `~/.kiro/` or `.kiro/` except one opt-in telemetry Hook file | Kiro's own guidance: session storage is server-owned | locked |
| 13 | Plan docs only in this pass; P0 probes before any product code | several load-bearing assumptions are third-party reconstructions or undocumented | current state |

## Revisit when

- **#5**: if P0 shows reattach fidelity is unacceptable, the fallbacks are tmux-owned PTYs or
  an ACP-rendered board. Both were rejected once; re-open only with evidence.
- **#11**: if Hooks prove unreliable in PTY-launched sessions, consider a read-only
  `.jsonl` tail reducer as the primary signal, or reconsider ACP for *status only*.
- **#9**: if Kiro deprecates the V2 harness on the target machines, drop the V2 adapter and
  keep the generation field for history rows.
