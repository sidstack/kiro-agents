# Sessions: the managed unit, and how it honors native Kiro

This is the target design. The current [MVP](MVP.md) supports fresh V2 launches
and live-process reattach only; history, Kiro UUID tracking, and resume are deferred.

## Definition

A **session** (our term) is the tuple

```
session = ( kiro agent profile  x  directory  x  kiro chat session id  x  generation  x  live process )
```

It is *not* a new kind of agent. It is one ordinary Kiro CLI chat, launched the way Kiro
launches it, kept alive by our host, and navigable from the board.

| Field | Owner | Notes |
| --- | --- | --- |
| agent profile | Kiro | name from `.kiro/agents/`, `~/.kiro/agents/`, or built-in |
| directory | Kiro | the child's `cwd`; Kiro scopes chat sessions, steering, hooks, MCP, skills, permissions to it |
| kiro chat session id | Kiro | UUID; we store it, we never mint or edit it |
| generation (`v2` \| `v3`) | ours, recorded at creation | decides argv and which resume path is legal |
| name, state, unread, draft | ours | registry metadata only |

Display name: `<directory-basename> · <profile>` (e.g. `api-gateway · code-reviewer`).
After the first settled turn, `autoName` may replace the directory-derived title with a
short title. Kiro's own `title` (auto-set from the first user prompt in
`<id>.json`) is read and preferred when present. Home stays `Home`.

## Full native `.kiro` honoring

A session is created by starting Kiro **in the target directory**, not by building a config
for it. The child is launched as:

```sh
# V3
cd <dir> && kiro-cli chat --v3 [--agent <profile>] [--resume-id <kiro-session-id>] [--effort <level>]
# V2
cd <dir> && kiro-cli chat --v2 [--agent <profile>] [--resume-id <kiro-session-id>]
```

Because `cwd` is the project root, Kiro itself resolves and applies:

| Native feature | Path | How we honor it |
| --- | --- | --- |
| Custom agents | `.kiro/agents/*.json\|*.md`, `~/.kiro/agents/` | select by name; never copy, merge, or synthesize a profile |
| Legacy agents | `.amazonq/cli-agents/`, `~/.aws/amazonq/cli-agents/` | discovered, labeled `legacy`, offered only for `--v2` unless `/upgrade-agent` ran |
| Built-in agents | `kiro_default`, `kiro_planner`, `kiro_help` | always offered; never editable |
| Steering | `.kiro/steering/`, `~/.kiro/steering/` | untouched; Kiro injects it |
| Hooks | `.kiro/hooks/*.json`, `~/.kiro/hooks/` | untouched; our telemetry hook is additive and removable |
| MCP servers | `.kiro/settings/mcp.json`, `~/.kiro/settings/mcp.json` | untouched; `includeMcpJson` in the profile decides inclusion |
| Skills / Powers | `.kiro/skills/`, Powers installs | untouched |
| Permissions | `permissions.yaml`, `--trust-all-tools`, `--trust-tools` | never set implicitly for interactive sessions; approvals stay in Kiro's UI |
| `AGENTS.md` | project and nested | untouched |
| Settings | `~/.kiro/settings/cli.json`, workspace scope | read for defaults (`chat.defaultAgent`, `chat.defaultModel`, `chat.agentEngine`); never written |
| Resource inheritance | `chat.disableInheritingDefaultResources` | surfaced in the board, never overridden |

Rules that keep this honest:

1. Never set `KIRO_HOME` to a private sandbox. Sessions use the user's real Kiro home so
   their real `.kiro` setup applies. Isolation, if ever wanted, is an explicit opt-in
   documented as "this session will not see your `.kiro`".
2. Never write into `~/.kiro/` or `<dir>/.kiro/` except the single optional telemetry Hook
   file, which is opt-in, versioned, and removed by `kiro-agents hooks disable`.
3. Never rewrite a profile to "fix" it. If a profile is invalid, run
   `kiro-cli agent validate --path <file>` and show Kiro's error.
4. Never pass `--trust-all-tools` unless the user asked for that session to be unattended,
   and show it on the session row (`unattended` badge) so an approval-free agent is visible.

## Agent selection: every Kiro agent, resolved per directory

Discovery is per directory because scope is per directory.

Authoritative path (preferred): run Kiro's own resolver with the target cwd.

```sh
cd <dir> && kiro-cli agent list --format json-pretty     # profiles Kiro would offer here
cd <dir> && kiro-cli chat --list-models --format json    # models for the model/effort picker
```

Fallback path (when `agent list` output is unavailable or unhelpful): scan
`<dir>/.kiro/agents/`, `$KIRO_AGENT_CONFIG_DIR` (global override; skips the workspace scan),
`~/.kiro/agents/`, then the legacy Amazon Q paths, accepting `.json` and `.md` (frontmatter)
profiles, and mark V2-only profiles (`allowedTools`/`toolsSettings` without `permissions`)
as **needs `/upgrade-agent`** for V3 launches.

Board create flow:

```
Ctrl+N  -> directory picker
        -> profile picker for that directory:
             kiro_default (built-in)      kiro_planner (built-in)
             code-reviewer (workspace)    aws-expert (global)
             legacy-optimizer (legacy, v2 only)
        -> generation picker (v3 default; v2 offered for legacy sessions/profiles)
        -> optional model / effort override
        -> create: spawn child, record session id from Kiro's session file / Kiro's own id
```

Session creation must fail loudly, not silently fall back, when the profile is missing:
Kiro exits `4` when a named agent is unavailable on the selected engine. Map that to
"agent `X` is not available for `<dir>` on `<generation>`" and keep the directory row.

## Kiro's own multi-agent features stay inside Kiro

- `/spawn <task>`, subagents, and the `Ctrl+G` crew monitor are Kiro features. We do not
  reimplement them and we do not intercept `Ctrl+G`.
- Sub-agent chat sessions appear in `~/.kiro/sessions/cli/` with `parent_session_id` set
  (`session_created_reason` is `"subagent"` on *every* session and must not be used).
  Optional read-only board grouping: show children under their parent row, marked
  `kiro subagent`, never as our sessions, never attachable.
- Our value is the axis Kiro does not cover: **many sessions across many directories,
  alive after the terminal closes, switchable from one board.**

## Creating, resuming, and stopping

| Action | Command we run | Notes |
| --- | --- | --- |
| New session | `kiro-cli chat --v3 [--agent p]` in `dir` | capture the new Kiro session id from the newest `<id>.json` for that cwd (read-only), or from Kiro's own reporting once verified in P0 |
| Open idle session | `kiro-cli chat --v3 --resume-id <id>` | same generation only |
| Open imported history | `kiro-cli chat --v2 --resume-id <id>` for V2 files | V3 cannot open a V2 session; format boundary is not migratable |
| Abort a turn | `SIGINT` to the child's process group | Kiro's own cancel semantics apply |
| Stop a session | `SIGTERM`, then reap | row becomes `stopped`; restorable by resume |
| Delete a Kiro session | `kiro-cli chat --delete-session <id>` | only on explicit user request, with confirm |

## History quick-open: finding a stored conversation and resuming it

`Ctrl+O` in the board opens a read-only quick-open over conversations Kiro already
stored on this machine. It is the only path from history to a live session, and a
resume is always an explicit Enter on one row.

**Where rows come from.** Kiro's own listing needs auth, so it cannot answer an
offline board (see `docs/FINDINGS.md`). The adapters read Kiro's stores directly
and read-only:

| Store | Layout | Resume here |
| --- | --- | --- |
| `v1` per-session file store | `<sessions>/cli/<id>.json` metadata, `<id>.jsonl`/`<id>.history` bounded transcript, `<id>.lock` | yes |
| `v2` per-workspace store | `<sessions>/<workspace-hash>/sess_<uuid>/session.json` (also `cli_<uuid>` imported rows) | listed, not resumed |

A row that appears in both stores is one row with both provenances. Unknown schemas
are counted as unrecognised in the coverage line, never guessed.

**Search.** One ranker is shared with the directory and agent pickers
(`src/fuzzy.rs`). Terms must all match; each matches a substring or a fuzzy
subsequence of the title, project, directory, agent, native id, or bounded prompt
text. Snippets show the matching role and window.

**Resume.** Only the `v1` file store is resumable by this wrapper: `kiro-cli chat
--v2 --tui --resume-id <id>` runs in the recorded project directory. Only an id
whose shape Kiro itself recognises (native UUID) is ever placed on a command line.
The child is an ordinary supervised session, so attach, Ctrl+Q detach, rename and
stop behave as for a new session, and the resume id is recorded as metadata.

**Older hosts.** `Resume` is the first action added to protocol v1, so a host that
was already running before this build was installed cannot parse the request and
closes the connection without replying (verified in `docs/FINDINGS.md`). The board
then probes the host once with an action it cannot name: a current host always
answers, an older one also closes. It states the diagnosis that matches — an older
host is told to update (via `kiro-agents shutdown`, which stops its managed
sessions), while a current host that closed mid-request is reported as an unknown
outcome to check against the session list. The board never restarts or replaces a
host by itself.

**Refusals.** Enter reports a reason instead of acting when the conversation is held
by a live Kiro lock, the lock cannot be read, the recorded project directory is gone,
or the row exists only in the newer per-workspace store. A stale or dead lock is
stated, then ordinary rules apply. A foreign process is never taken over and its lock
file is never removed or rewritten. A conversation already supervised by one of our
sessions resumes once and is then attachable, not duplicated. Nothing is replayed,
re-run, migrated or written into Kiro's stores.

**Budgets.** 2000 conversations and 20 000 *inspected* directory entries (every
entry a scan looks at costs budget, not just matching ones), 128 KiB per metadata
file, 32 KiB per transcript, 8 MiB of text overall, and a ten-second scan deadline
enforced inside the adapters, not just between them, so closing the picker cannot
join a runaway scan. The footer states
which stores were searched, how many rows and texts were read, and any truncation,
unrecognised or skipped-store counts, so a partial view is never mistaken for the
whole history.

**Privacy.** Titles, ids, dates and bounded prompt text are used in memory for
ranking and snippets only; credential-looking snippets are redacted and control
characters are stripped before rendering. The registry stores only the native
conversation id. Transcripts are never logged, and native stores are never written.
