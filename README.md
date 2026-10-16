# kiro-agents (planning)

Persistent **Kiro CLI sessions** across projects, with a session board you can open,
switch, and reattach to — the Kiro counterpart of
[`pi-codex-agents`](../pi-codex-agents).

Status: **planning only. No code yet.** This directory holds the design, the verified
Kiro CLI integration facts, the V2 + V3 support matrix, and the build order.

```sh
mkdir -p ~/Projects/kiro-agents && cd ~/Projects/kiro-agents   # this directory
```

## What it is

A supervisor that keeps `kiro-cli chat` processes alive in the background — one per
project directory, each running a Kiro agent profile natively — and lets you move a
terminal between them.

```text
your terminal
   kiro-agents (frontend: session board + passthrough attach)
        |  private JSONL over a Unix socket
        v
   kiro-agents-host (persistent daemon, survives closing your terminal)
        |- session: ~/Projects/api-gateway   agent: kiro_default   gen: v3   running
        |- session: ~/Projects/infra         agent: code-reviewer  gen: v3   needs input
        |- session: ~/Projects/notes         agent: kiro_planner   gen: v2   idle
        `- each session = one real `kiro-cli chat` process on its own PTY
```

Close the terminal, sessions keep running. Open a new terminal, `/sessions` (or
`kiro-agents board`) reconnects you to the same host and the same live processes.

## Vocabulary (deliberate)

| Term | Meaning |
| --- | --- |
| **Kiro agent** | Kiro's own concept: an agent profile in `.kiro/agents/*.json\|.md` or `~/.kiro/agents/`, plus built-ins (`kiro_default`, `kiro_planner`, `kiro_help`). We never invent or rewrite these. |
| **Kiro chat session** | Kiro's own persisted conversation, identified by a Kiro session UUID, scoped to a directory. Owned entirely by Kiro. |
| **Session** (ours) | A managed background unit: **one Kiro agent profile + one directory + one Kiro chat session + one live `kiro-cli` process**, supervised by our host. Navigable in the session board. |
| **Session board** | The command center: list, filter, create, switch, attach, abort, stop. |
| **Host** | The persistent daemon that owns the sessions and their processes. |
| **Attach / detach** | Lending your terminal to one session's process, and taking it back. |

We say **session**, not "agent", for our entity. Kiro owns the word *agent*.

## Non-goals (decided)

- **No ACP client.** We do not drive `kiro-cli acp` and we do not rebuild Kiro's UI.
- **No terminal emulation, no re-rendering, no scraping.** While attached, the wrapper is
  a raw byte pipe between your terminal and the session's PTY, plus resize forwarding.
  Kiro's own TUI renders itself, exactly as when you launch it by hand — the same
  "full native Pi experience" pi-codex-agents gives, but by passthrough instead of
  in-process embedding (Kiro is a closed binary; Pi is an embeddable SDK).
- **No writing to Kiro's session store.** Kiro owns `~/.kiro/sessions/`. We keep our own
  registry of *pointers* (session id, cwd, agent, generation, state, draft).
- **No synthetic agent profiles.** A session launches Kiro inside the project directory so
  Kiro loads `.kiro/agents`, `.kiro/steering`, `.kiro/hooks`, `.kiro/settings/mcp.json`,
  `.kiro/skills`, `AGENTS.md`, and `permissions.yaml` natively.

## Read in this order

1. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — host, sessions, attach, persistence, IPC
2. [docs/SESSIONS.md](docs/SESSIONS.md) — session model, spawning, native `.kiro` honoring, agent selection
3. [docs/GENERATION-SUPPORT.md](docs/GENERATION-SUPPORT.md) — Kiro V2 + V3 dual support
4. [docs/KIRO-INTEGRATION.md](docs/KIRO-INTEGRATION.md) — verified Kiro facts and citations
5. [docs/PLAN.md](docs/PLAN.md) — phases P0..P5, crates, tests
6. [docs/RISKS.md](docs/RISKS.md) — unknowns, probes, mitigations
7. [docs/PI-PARITY.md](docs/PI-PARITY.md) — what carries over from pi-codex-agents and what cannot
8. [docs/DECISIONS.md](docs/DECISIONS.md) — the 13 locked decisions and when to revisit them
9. [docs/FINDINGS.md](docs/FINDINGS.md) — empty until P0 probes run; findings beat docs

## Key constraint discovered during research

Kiro CLI has **no extension API** (closed Rust binary). It exposes: a CLI surface
(`kiro-cli chat`, `agent`, `mcp`, `settings`, `doctor`), on-disk session files, JSON Hooks
that run shell commands with event JSON on stdin, headless `--output-format stream-json`,
and an ACP server (`kiro-cli acp`). Since we want the native TUI and no ACP, the supported
integration surface is: **process supervision + PTY passthrough + Hooks + session files
(read-only) + CLI subcommands.**

## Naming and layout (proposed)

- Repo/dir: `kiro-agents` · binary: `kiro-agents` · daemon: `kiro-agents-host`
- State: `~/.kiro-agents/` (override `KIRO_AGENTS_HOME`) — never inside `~/.kiro/`
- Commands: `kiro-agents` (board), `new [dir] [--agent NAME] [--v2|--v3]`, `attach <id>`,
  `status`, `stop`, `restart`, `agents` (list Kiro profiles for a directory)
