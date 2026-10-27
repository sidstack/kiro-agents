# Kiro CLI integration reference

Local validation: Kiro CLI 2.32.0; see [FINDINGS.md](FINDINGS.md) for observed facts
and blocked probes. The reference below is not a promise of locally tested behavior.

Research snapshot: 2026-10-16. Kiro CLI current line referenced by docs: CLI 2.31.x / V3
harness, CLI v1.26+ agent-name-as-positional behavior. Kiro CLI is **closed source**
(github.com/kirodotdev/Kiro hosts issues; KiroCrew is the open-source companion).

Legend: **[doc]** documented by Kiro · **[3p]** third-party reconstruction, verify locally
· **[probe]** must be confirmed on the target machine before implementation.

## CLI surface we use

`kiro-cli chat [OPTIONS] [INPUT]` **[doc]**

| Flag | Use in kiro-agents |
| --- | --- |
| `--v2` / `--v3` / `--agent-engine <v1\|v2\|v3>` | generation selection; `--v2` and `--v3` cannot combine; explicit flag beats saved `chat.agentEngine` |
| `--agent <name>` | launch a chosen Kiro agent profile |
| `--resume`, `-r` | resume newest matching local session in cwd |
| `--resume-id <id>` | resume a specific session (our idle-open path) |
| `--resume-picker` | not used (our board replaces it) |
| `--list-sessions` | session discovery fallback for a cwd |
| `--delete-session <id>` | explicit user-requested deletion only |
| `--list-models [--format json]` | model picker |
| `--model <id>`, `--effort <low\|medium\|high\|xhigh\|max>` | per-session overrides |
| `--trust-all-tools` | only for explicitly unattended sessions, shown as a badge |
| `--trust-tools=<ids>` | V3 needs exact internal tool IDs; **interactive V3 ignores it** |
| `--wrap always\|never\|auto` | PTY/relay friendliness probe |
| `--require-mcp-startup` | optional gate for sessions that need MCP (exit 3) |
| `--no-interactive`, `--output-format stream-json`, `--sandbox` | out of scope for the board; reserved for a later automation mode |

Other commands we use: `kiro-cli agent list|create|edit|validate|set-default`,
`kiro-cli mcp list|status`, `kiro-cli settings list|<key>`, `kiro-cli whoami --format json`,
`kiro-cli doctor`, `kiro-cli version`, `kiro-cli diagnostic --force`. **[doc]**

Exit codes: `0` success · `1` general/auth/invalid args · `3` required MCP startup failure ·
`4` requested agent not found (non-interactive V2/V3, also when `chat.defaultAgent` is
unavailable). **[doc]**

## Session storage

`~/.kiro/sessions/cli/` four-file bundle per session **[3p]**, confirmed in part by Kiro docs
(sessions saved every turn, per directory, UUID ids, `KIRO_SESSIONS_DIR` override):

```
<id>.json     metadata sidecar: session_id, cwd, created_at, updated_at, title,
              parent_session_id, session_state { agent_name, conversation_metadata,
              rts_model_state { model_info, context_usage_percentage }, permissions }
<id>.jsonl    conversation log: kinds Prompt | AssistantMessage | ToolResults
<id>.history  plain-text user prompt log (includes slash commands)
<id>.lock     {"pid":12927,"started_at":"..."} present only while the process is alive
```

Facts that matter to us **[3p]**:

- `.json` is rewritten every turn — watch `.jsonl` mtime, do not tail `.json`.
- `meta.timestamp` is **unix seconds**, not milliseconds.
- `session_created_reason` is `"subagent"` on *every* session; use `parent_session_id`
  (null = top-level) to detect sub-agents.
- `agent_name` is null on sub-agents; they inherit the parent persona.
- Tool names are lowercase snake_case: `read, write, edit, shell, grep, glob, web_fetch,
  web_search, subagent, todo, introspect`; tool inputs often carry a UI-only
  `__tool_use_purpose` field.
- `data.sqlite3` (macOS `~/Library/Application Support/kiro-cli/`, Linux
  `~/.local/share/kiro-cli/`, `KIRO_DATA_DIR` override) is a secondary index/cache.
- Kiro's own guidance for programmatic clients: **treat storage as server-owned**, do not
  write Kiro session files, do not depend on the V3 directory layout. We keep our registry
  as pointers only. **[doc]**

## Hooks (our only sanctioned live-signal channel)

Standalone Hook files: `<dir>/.kiro/hooks/*.json` and (CLI V3) `~/.kiro/hooks/*.json`.
Schema `{ "version": "v1", "hooks": [ { name, trigger, matcher, action:{type:"command",
command}, timeout, enabled, confirm } ] }`. Command actions receive event JSON on **stdin**
(`hook_event_name`, `cwd`, `session_id`, `tool_name`, `tool_input` for tool events) and
`USER_PROMPT` env for `PromptSubmit`. Hook exit codes: `0` ok, `2` (PreToolUse only) blocks
the tool with stderr to the LLM, other = failure warning. **[doc]**

Triggers available in CLI V3: `SessionStart`, `SessionEnd`, `PromptSubmit`, `AgentStop`,
`PreToolUse`, `PostToolUse`, `FileCreate`, `FileSave`, `FileDelete`, `PreTaskExecution`,
`PostTaskExecution`, `Manual`. CLI 2.x uses camelCase (`agentSpawn`) compatibility. **[doc]**

Our telemetry hook (opt-in) uses only `SessionStart`, `PromptSubmit`, `AgentStop`,
`SessionEnd`, never `PreToolUse`, and never exits `2`, so it can never block or alter an
agent. **[probe: exact payloads and firing reliability in a PTY-launched V3 session]**

## Environment variables

| Variable | Meaning |
| --- | --- |
| `KIRO_HOME` | global Kiro dir: MCP config, sign-in, permissions, sessions, Powers, settings, custom agents, Skills, Steering, Workflows, V3 agent state |
| `KIRO_SESSIONS_DIR` | override the file session directory **[3p]** |
| `KIRO_DATA_DIR` | override the `data.sqlite3` root **[3p]** |
| `KIRO_AGENT_CONFIG_DIR` | global agent scan directory; skips the workspace scan **[doc]** |
| `KIRO_API_KEY` | headless auth |
| `KIRO_LOG_LEVEL`, `KIRO_LOG_NO_COLOR`, `KIRO_CHAT_LOG_FILE` | logging |
| `KIRO_NO_SYNCHRONIZED=1` | disable synchronized output when a terminal path renders badly **[doc]** |
| `KIRO_HEADLESS_WORKFLOW_TIMEOUT_SECS` | headless workflow settle timeout |
| `KIROCREW_HOME`, `KIROCREW_PORT` | Kiro Crew gateway (separate product) |

Logs: macOS `$TMPDIR/kiro-log/`, Linux `$XDG_RUNTIME_DIR/kiro-log/` or `/tmp/kiro-log/`.

## Kiro's own multi-agent features (do not duplicate)

- `/spawn <task> [--name n]` starts a parallel long-running session; `Ctrl+G` opens the
  **crew monitor** for spawned/subagent status, `Ctrl+D`/`Ctrl+U` navigate, `q` closes. **[doc]**
- Sub-agents are agent-driven delegation inside a task graph; `/spawn` is user-driven. **[doc]**
- Kiro Crew (`kiro-cli crew`, `kirocrew`, `~/.kiro/crew`, port 5476) is a separate persistent
  multi-channel product built on an ACP harness. Out of scope; noted so we do not collide. **[doc]**

## Kiro terminal UI keys and wrapper override

`Ctrl+C` exit · `Ctrl+D` exit / navigate monitor · `Ctrl+G` crew monitor · `Ctrl+X` activity
tray / spec review · `Ctrl+T` transcript or tangent · `Ctrl+O` expand output · `Ctrl+R`
history search · `Ctrl+S` fuzzy search · `Ctrl+J` newline · `Ctrl+U` half-page / monitor nav ·
`Ctrl+Z` twice suspend (reported broken in the TUI: issue 6864) · `Tab`, `Shift+Tab` plan
mode · `Esc` cancel · `!` shell escape · `/editor` opens `$EDITOR` · `/fullscreen` dedicated
full-terminal view · `KIRO_NO_SYNCHRONIZED` escape hatch. **[doc]**

The wrapper reserves Ctrl+G for detach/dashboard navigation by explicit user choice,
overriding Kiro's crew shortcut while attached. Tmux retains Ctrl+B, and Left Arrow
always passes through. There is no editor-state integration or configurable shortcut
in the current implementation.

## ACP (known, deliberately out of scope)

`kiro-cli acp` (V2) and `kiro-cli acp --agent-engine=v3` implement the Agent Client Protocol
over newline-delimited JSON-RPC 2.0: `initialize`, `session/new|list|load|fork|prompt|cancel|
set_mode|set_config_option`, `session/update` (`agent_message_chunk`, `agent_thought_chunk`,
`tool_call`, `tool_call_update`, `available_commands_update`, `current_mode_update`,
`config_option_update`, `session_info_update` with `_meta.kiro.kind` = `context_usage` /
`turn_completion`), `session/request_permission`, and advertised `_kiro/*` extensions
(`agentCapabilities._meta.kiro.extensionMethods`). **[doc]**

Recorded because it is the obvious alternative transport and because a future *automation*
mode (headless runs, CI, programmatic spawn) should use ACP rather than PTY scraping. The
board itself stays native-TUI passthrough by decision.

## Sources

- https://kiro.dev/docs/reference/cli-commands/
- https://kiro.dev/docs/reference/slash-commands/
- https://kiro.dev/docs/reference/exit-codes/
- https://kiro.dev/docs/reference/settings/
- https://kiro.dev/docs/cli/chat/ · /chat/session-management/ · /chat/settings/ · /chat/configuration/
- https://kiro.dev/docs/cli/terminal-ui/
- https://kiro.dev/docs/cli/headless/
- https://kiro.dev/docs/cli/v3/ · /cli/v3/agent-config/ · /cli/v3/acp-migration/ · /cli/v3/hooks-migration/
- https://kiro.dev/docs/cli/acp/
- https://kiro.dev/docs/custom-agents/ · /custom-agents/configuration-reference/ · /custom-agents/subagents/
- https://kiro.dev/docs/hooks/ · /hooks/types/ · /hooks/actions/
- https://kiro.dev/docs/configuration/ · /docs/mcp/configuration/ · /docs/crew/
- https://github.com/neochoon/agenthud/blob/main/docs/schemas/kiro-session.md **[3p]**
- https://vshulcz.github.io/deja-vu/registry/kiro.html **[3p]**
- https://github.com/janekbaraniewski/openusage/blob/main/docs/site/docs/providers/kiro.md **[3p]**
- https://github.com/kirodotdev/Kiro/issues/6864 (Ctrl+Z suspend in TUI)
