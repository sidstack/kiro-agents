# Findings (P0 output — currently empty)

Fill this from `probes/out/` transcripts. Every entry states: command run, Kiro version,
observed behavior, and which doc claim it confirms or contradicts.

| # | Question | Source claim | Observed | Action |
| --- | --- | --- | --- | --- |
| 1 | Does V3 write `~/.kiro/sessions/cli/<id>.{json,jsonl,history,lock}`? | `[3p]` agenthud schema (V2-era) | | |
| 2 | `.lock` shape and removal on clean exit / crash | `{"pid","started_at"}` | | |
| 3 | `--resume-id` across generations | formats incompatible | | |
| 4 | Two live processes, same session id | unknown | | |
| 5 | Two live sessions, same directory | sessions are per-directory | | |
| 6 | Detached PTY output before the child blocks | ~64 KiB PTY buffer | | |
| 7 | Forced same-size resize triggers a full repaint | assumed | | |
| 8 | `kiro-cli agent list --format json-pretty` shape and cwd scoping | documented command | | |
| 9 | Legacy `.amazonq/cli-agents/` and `KIRO_AGENT_CONFIG_DIR` behavior | documented | | |
| 10 | Hook triggers firing in a PTY-launched V3 session + exact stdin JSON | documented triggers | | |
| 11 | Global `~/.kiro/hooks/` loaded for `kiro-cli chat --v3` | documented V3 | | |
| 12 | Exit codes 1 / 3 / 4 reproduced | documented | | |
| 13 | `/editor`, `!` shell escape, interactive `sudo`/`ssh` prompts under attach | documented features | | |
| 14 | `Ctrl+Z` behavior in the current TUI | issue 6864 | | |
| 15 | `KIRO_NO_SYNCHRONIZED` needed for clean rendering in our path | documented escape hatch | | |
| 16 | Concurrent-session limits per account | unknown | | |

## Contradictions found

(none yet)

## Decisions forced by findings

(none yet)
