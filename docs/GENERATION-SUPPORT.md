# Supporting both Kiro CLI V2 and V3

This is the target design. The current [MVP](MVP.md) supports fresh V2 launches
and live-process reattach only; history, Kiro UUID tracking, and resume are deferred.

Kiro CLI ships two agent harnesses side by side. They are **not** interchangeable, and their
saved session formats are **not backward compatible**: "V2 cannot open a V3 session because
the stored session formats differ; that is a cross-version migration boundary." Supporting
both is therefore a first-class design requirement, not a flag.

## Where the generation lives

- Recorded per session in our registry at creation: `generation: "v2" | "v3"`.
- Never inferred from a Kiro version string alone.
- `defaultGeneration` config (`v3` today) plus `chat.agentEngine` (Kiro's saved default) as
  the fallback when the user did not choose.
- Explicit override per session: `kiro-agents new <dir> --v2|--v3`, and `--agent-engine`
  is accepted on `kiro-cli chat` as well as at the root.

## Launch plan per generation

| Concern | V2 (`--v2`) | V3 (`--v3`) |
| --- | --- | --- |
| Interactive launch | `kiro-cli chat --v2 [--agent p] [--resume-id id]` | `kiro-cli chat --v3 [--agent p] [--resume-id id] [--effort l]` |
| Harness default source | `chat.agentEngine` unless flag given | `chat.agentEngine`; explicit flag wins for that invocation |
| Interface | terminal UI **and** classic (non-TUI) interface available | terminal UI only; V3 does not run the classic interface |
| Session picker in UI | `/chat` | `/chat` and `/sessions` (V3 `/sessions` dashboard, `chat.sessionDashboard.scope`) |
| Model pin at launch | `--model <id>` overrides agent model | `--model <id>` overrides agent model and `chat.defaultModel` |
| Effort at launch | `--effort <level>` | `--effort <level>`, applies only to that started session |
| Tool trust | `--trust-tools` uses CLI 2.x tool names | `--trust-tools` needs exact internal tool IDs; **interactive V3 ignores `--trust-tools`**; session-wide trust is `--trust-all-tools` or `/tools trust-all` |
| Agent profiles | `allowedTools`, `toolAliases`, `toolsSettings`, `keyboardShortcut` | `permissions.rules`, `excludedTools`, `includeMcpJson`, `includePowers`, `welcomeMessage`, `.md` profiles; profiles with legacy trust fields and no `permissions` are **skipped by V3** until `/upgrade-agent` |
| Hooks in profiles | camelCase object form (`agentSpawn`, `preToolUse`) | PascalCase array form; standalone `.kiro/hooks/*.json` (global `~/.kiro/hooks/` also loaded) |
| Hook triggers available | `agentSpawn`, prompt/tool/file subset | `SessionStart`, `SessionEnd`, `PromptSubmit`, `AgentStop`, `PreToolUse`, `PostToolUse`, `FileCreate/Save/Delete`, `PreTaskExecution`, `PostTaskExecution`, `Manual` |
| Built-in AWS tool | `use_aws` (alias `aws`) available | removed; MCP servers required |
| Session store | `~/.kiro/sessions/cli/<id>.json\|.jsonl\|.history\|.lock` four-file bundle (+ `data.sqlite3` index) | V3 store; layout is not a client-readable contract |
| Resume legality | V2 sessions only | V3 sessions only |
| Conversation rewind | `/checkpoint` (classic only) | `/rewind` conversation branches |
| Knowledge / code intelligence | experimental, `chat.enableKnowledge true` | enabled by default in V3 (`chat.enableKnowledge`, `chat.enableCodeIntelligence`) |
| Headless (for future automation) | `--no-interactive --output-format stream-json` | same, plus interruption record, workflow settle, `--sandbox`, exit 3/4 gating |

## Generation resolver

```
resolve(dir, requested, kiro_session_id?) -> Generation
  1. explicit --v2 / --v3 wins
  2. registry generation for an existing session wins
  3. profile shape: legacy-only profile (allowedTools/toolsSettings, no permissions)
        -> v2, and report "run /upgrade-agent to use it on v3"
  4. session-file provenance (P0 probe): V2 bundle vs V3 store entry
  5. otherwise config.defaultGeneration, then Kiro's chat.agentEngine
```

Hard rules:

1. **Never resume across generations.** If a user opens a V2 session and asks for V3, refuse
   with the reason and offer: keep the V2 session, or start a fresh V3 session in the same
   directory with the same profile. `/upgrade-agent` migrates *profiles*, not sessions.
2. **Never auto-upgrade a profile.** `/upgrade-agent` is destructive-ish (it backs up to
   `.json.bak` and rewrites configs). We surface it; the user runs it.
3. **Never mix hook formats.** Our telemetry hook is written in the standalone PascalCase
   form, which V3 loads globally. For V2 sessions we do not rely on it and fall back to
   `.lock` + `.jsonl` liveness signals.
4. **Per-generation feature availability in the board.** A V2 row never offers V3-only
   affordances, and vice versa. Unknown capability = not shown, never shown-and-failing.
5. **Adapter boundary.** `src/kiro/launch.rs` is the only place that knows argv per
   generation; `src/kiro/profiles.rs` the only place that knows profile schema
   differences; `src/kiro/history.rs` the only place that reads session files. Everything
   else is generation-agnostic.

## Detection probes required before implementation (P0)

These must be answered on a real install, not from docs:

1. Does `~/.kiro/sessions/cli/` still receive V3 sessions, or does V3 write only to the
   V3 store (`data.sqlite3` / a different directory)? What exactly distinguishes a V2 file
   from a V3 file (schema keys, `version`, directory)?
2. Does `--resume-id <v2-id>` under `--v3` fail cleanly (exit code + message) or silently
   start a replacement session? Docs say the terminal UI no longer silently starts a
   replacement when `--resume-id` does not match a local session — confirm for both engines.
3. Does `kiro-cli agent list --format json-pretty` list built-ins, workspace, global, and
   legacy profiles, and does it respect cwd? What is its exact JSON shape?
4. Does `kiro-cli chat --list-sessions` report generation per session?
5. Does a V3 session create `<id>.lock` with the same `{"pid","started_at"}` shape as V2?
6. Which Hook triggers actually fire for `kiro-cli chat --v3` in a terminal PTY, and what is
   the exact stdin JSON for each?
7. Does `kiro-cli chat` honor `KIRO_SESSIONS_DIR` / `KIRO_DATA_DIR` / `KIRO_AGENT_CONFIG_DIR`
   in the version installed, and does `KIRO_HOME` cover all V3 agent state?

Findings go in `docs/FINDINGS.md` with transcripts. Any answer that contradicts the docs
wins over the docs.
