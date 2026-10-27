# kiro-agents

Minimal Linux proof of concept for persistent **native Kiro V2 terminal sessions**.
A background Rust host keeps each Kiro process on its own PTY. Detach, close the
frontend, and reconnect to the same live process from another terminal.

The wider design in `docs/` remains a roadmap. MVP v3 improves the interactive
session board with directory and agent pickers and a continuous terminal handoff.
MVP v3 is the product iteration; sessions still launch Kiro’s V2 engine.

## Install

Install the latest stable Linux x86_64 binary without Rust or sudo:

```sh
curl -fsSL https://raw.githubusercontent.com/sidstack/kiro-agents/main/install.sh | sh
```

Select an exact stable or beta release, or choose an installation directory:

```sh
curl -fsSL https://raw.githubusercontent.com/sidstack/kiro-agents/main/install.sh | sh -s -- --version v0.3.1-beta.15.1
```

Pass `--install-dir DIRECTORY` or `--uninstall` after `sh -s --` in the same command.
From a source checkout, use `sh install.sh` with those flags directly.

The installer requires Linux x86_64/glibc 2.35+, curl, tar and coreutils. It checks
the release checksum, archive contents and executable version before atomically
replacing the binary. Re-running upgrades it; uninstall removes only the executable.
Hosts, sessions and Kiro configuration are left running and unchanged. Kiro CLI must
be installed and authenticated separately. The manual download method follows.

For pre-merge testing, branch CI publishes versioned `-beta.RUN.ATTEMPT` binaries
as GitHub prereleases after integration tests and dependency auditing. Betas have
the same installation steps; see [branch beta builds](docs/RELEASING.md#branch-beta-binaries).
Stable releases remain the default for normal installation.

Download the Linux x86_64 archive and `SHA256SUMS` from [GitHub Releases](https://github.com/sidstack/kiro-agents/releases). Verify the checksum, extract it, and install the binary:

```sh
sha256sum --check SHA256SUMS
tar -xzf kiro-agents-v0.3.0-x86_64-unknown-linux-gnu.tar.gz
mkdir -p "$HOME/.local/bin"
install -m 0755 kiro-agents-v0.3.0-x86_64-unknown-linux-gnu/kiro-agents "$HOME/.local/bin/kiro-agents"
export PATH="$HOME/.local/bin:$PATH"
kiro-agents --version
```

The binary requires Linux x86_64 with glibc 2.35+ and a separately installed,
authenticated Kiro CLI. Rust and Python are not runtime dependencies. Binary
assets appear after an automated release completes; local MVP builds alone do
not imply that a GitHub release is available.

To build from a checkout instead, install Rust stable and run:

```sh
cargo install --path . --locked
```

See [release and installation details](docs/RELEASING.md) for upgrades, supported
platforms and automated Semantic Versioning. Conventional Commits drive version
and changelog updates; merging the generated release PR publishes the binary.

## Run

```sh
kiro-cli login                 # authenticate in your own terminal
kiro-agents doctor
kiro-agents                    # open the dashboard; Ctrl+N creates a session
```

Inside Kiro, use it normally. **Ctrl+Q** detaches without stopping Kiro.
Kiro’s Ctrl+G crew shortcut and tmux’s Ctrl+B prefix keep their native behavior.
Left Arrow and other keys retain native behavior.
Exiting inside Kiro ends its process; detach instead to keep it attachable.
When opened from the board, detach returns to the board. Explicit `attach` and
`new` commands return to the shell.
Normal Kiro exit retains its screen/cursor; detach returns the shell to the
bottom of the terminal.

```sh
kiro-agents                    # interactive session board
kiro-agents status
kiro-agents attach <id>         # reconnect to the same child
kiro-agents new /path/to/project --agent <existing-profile>
kiro-agents agents /path/to/project
kiro-agents stop <id>           # stop one owned session
kiro-agents shutdown            # stop all owned sessions and the host
```

The board requires an interactive input and output terminal; piped output is a
plain session list. Up/Down or j/k selects, Enter attaches, Tab switches Live/All,
and / searches. Esc clears search or cancels a form. Ctrl+N opens a directory picker
with current/recent projects and filesystem browsing, followed by a searchable
list of native Kiro agents for the chosen directory. Type a fuzzy query and press
Enter to select the best match; arrow keys are optional. Both legacy BS and DEL
Backspace encodings edit the query. Typing a path is also supported. Tab or Right Arrow
browses a directory; Left Arrow goes to its parent;
Ctrl+U clears the query and Ctrl+R retries discovery. Esc from the agent list goes
back to the directory picker. Ctrl+K asks before stopping the selected live
session. F2 names a session; an empty name restores its automatic title. The board
prefers a manual name, then an exactly matched native Kiro title, a short first-prompt
title, and directory/agent plus a unique ID as the fallback. F2 names work even with an
older running host. Automatic SQLite TUI titles are not supported until a verified
store association exists; use F2 for those sessions. See [session names](docs/SESSION-NAMES.md).
r refreshes; q or Ctrl+Q leaves sessions alive.
Exited, stopped, and failed rows explain why they cannot be attached.

Agent discovery is captured asynchronously with a five-second deadline and a
256 KiB output cap. Its JSON shapes are covered by fixtures; authenticated real
Kiro enumeration remains unverified. Scoped read-only profile discovery supplements
CLI results and provides a fallback when listing fails or has an unsupported format.
The Kiro default and an explicit typed-name choice remain available. No profile is invented.
On accepted attach the board releases its alternate screen while keeping raw mode,
so native Kiro has the same screen choice as direct invocation. Primary-screen output
uses terminal scrollback; native full-screen history stays under Kiro's control.
Wheel/PageUp input is passed through unchanged.
Status refresh runs in the background; idle boards redraw only after changes.

`new --no-attach` starts in the background. The host inherits the environment of
the terminal that first starts it. State lives in `~/.kiro-agents/`, overridable
with `KIRO_AGENTS_HOME`; `KIRO_AGENTS_KIRO_BIN` selects the executable for tests.
Kiro loads your own profiles, steering, MCP, and permissions from the target cwd
and its native configuration. The wrapper never writes Kiro configuration or
profiles and never adds tool-trust flags.

## What is validated

- Rust format, strict Clippy, Rust unit/integration tests, and board snapshots.
- Fake-CLI PTY checks: board navigation/search, create/profile selection, confirmed
  stop, switch with stable PIDs, native exit back to board, error recovery, detach,
  resize, byte forwarding, frontend SIGKILL survival, and terminal restoration.
- Continuous board/native handoff, delayed/refused attach, slow discovery
  cancellation, discovery retry, and idle-board redraw suppression.
- Isolated real CLI capability and startup probes, with no model calls. See
  [findings](docs/FINDINGS.md) for results and limitations.

The user validated native TUI detach/reattach, returning to a pending permission
prompt, and corrected exit cleanup through Guacamole SSH. Active real-task
progress while detached still needs a manual check. No account credentials were
copied into probes. See the [MVP v1 journal](docs/journal/MVP-V1.md) for the
original feedback and validation boundaries, and the
[MVP v2 journal](docs/journal/MVP-V2.md) and
[MVP v3 milestones and journal](docs/journal/MVP-V3.md) for dashboard changes.

## MVP limits

- V2 only, always `kiro-cli chat --v2 --tui`; eight live sessions maximum.
- Status means child liveness (`live`, `exited`, `failed`, `stopped`), not turn state.
- One attached frontend per session. Detach it before moving to another terminal.
- A 2 MiB in-memory byte ring is replayed on attach, followed by forced SIGWINCH.
  This is byte replay, without a screen model; real Kiro repaint fidelity remains
  a manual acceptance check.
- Large historical registries can exceed the 128 KiB status response limit. The
  board shows a recoverable refresh error; pagination is deferred. See
  [findings](docs/FINDINGS.md).
- Wrapper IDs identify managed processes; they are **not** Kiro session UUIDs.
  Fresh launches only: no resume, history import, idle eviction, or prompt replay.
- After a host crash, saved live rows become `failed` on host restart; attach
  refuses them. Inspect/resume history through Kiro itself. No automatic recovery
  spawn or signals to a saved PID.
- The board shows process liveness, with Live and All filters. Historical rows
  cannot be attached. No telemetry hooks, drafts, V3 support, or
  general settings system yet. Automatic names are best-effort and optional. No transcript or child-environment logging.

## Develop

```sh
cargo install --path . --locked
make check                     # fmt, clippy, tests, fake terminal smoke
make audit                     # requires cargo-audit; strict RustSec audit
make probe                     # opt-in real-CLI test; requires a checkout
```

Probes isolate `HOME`, `KIRO_HOME`, Kiro overrides, and XDG roots in a temporary
directory. Only sanitized capability summaries go to ignored `probes/out/`.
Python 3 is used for test peers and probe harnesses, not by the shipped binary.

## Plan and review

Start with [MVP scope and plan review](docs/MVP.md), then
[architecture](docs/ARCHITECTURE.md), [sessions](docs/SESSIONS.md),
[generation support](docs/GENERATION-SUPPORT.md), [Kiro integration](docs/KIRO-INTEGRATION.md),
[full build plan](docs/PLAN.md), [risks](docs/RISKS.md),
[Pi parity](docs/PI-PARITY.md), and [decisions](docs/DECISIONS.md).

## Project policies

[Contributing](CONTRIBUTING.md) · [Security](SECURITY.md) ·
[Code of conduct](CODE_OF_CONDUCT.md) · [MIT license](LICENSE) ·
[Changelog](CHANGELOG.md)

Independent project; not affiliated with or endorsed by Amazon, Kiro, OpenAI or Pi.
