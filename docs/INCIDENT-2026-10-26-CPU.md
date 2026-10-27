# Kiro orphan-process CPU incident — 2026-10-26

Observed around 23:34–23:36 UTC on VM101 (Ubuntu application VM on Proxmox).
User reported loud CPU AIO fans. Read-only checks identified Kiro processes as
the dominant CPU consumers; user explicitly authorized termination.

## Evidence

- Physical host: 28 online logical CPUs, approximately 84% CPU utilization;
  CPU package temperature 84°C.
- VM101: 100% guest CPU utilization, 41 running tasks in one sample, load
  averages 40.15 / 37.23 / 26.50. Its host QEMU process used approximately
  2001% CPU (about 20 logical CPUs).
- Numerous processes named `kiro-agents` or `kiro-agent's $b` (the latter is
  the truncated Linux comm value) were consuming CPU. The inspected process
  list showed PPID 1 for the high-CPU Kiro processes.
- Examples: PID 2857202 (`kiro-agents`) had accumulated approximately
  258 minutes CPU time; PID 3894103 had accumulated approximately 16 minutes
  28 seconds in an earlier sample. Individual processes reached about 100%
  CPU in an interval sample.
- GPU VM108 was nearly idle in the confirming host sample (~1% QEMU CPU).
  Earlier GPU readings were 31°C, 15 W, 3% utilization. This was not evidence
  of GPU inference causing the fan noise.

## Containment and verification

Selected only current-user processes whose `/proc/PID/comm` equaled
`kiro-agents` or started with `kiro-agent's`. Sent SIGTERM to 42 processes;
40 still matched after three seconds and received SIGKILL. Final matching
process count was zero. No application containers, VMs, or inference services
were restarted.

At 23:36:40 UTC, host CPU usage had fallen to approximately 1.5% and CPU
package temperature to 46°C. This strongly links the Kiro workload to the heat.
Fan RPM was not exposed, so actual fan slowdown was not measured.

## Investigation handoff

Root cause is unconfirmed. PPID 1 indicates reparenting but does not establish
whether each process was an intended detached host or a leaked worker.
Executable paths, full argv, stacks, installed binary version, process start
times, and original launch/exit sequence were not captured before termination.
The repository checkout must not be assumed to match the running binaries.

Investigate lifecycle cleanup and owned-child reaping, plus background-worker,
IPC/PTY EOF, disconnected-client and timer loops that might spin after their
parent exits. Reproduce with fake Kiro and temporary state; add a regression
that checks bounded CPU use and cleanup after frontend/host termination.
Preserve intentional detached-session behavior and never kill foreign Kiro
processes. No source fix or reproduction has been attempted in this incident.

## Repository follow-up

A subsequent bounded fake-Kiro/temporary-state reproduction closed a dashboard's
PTY master while it was waiting for input. Its frontend survived and consumed
0.49 CPU seconds in a 0.5-second sample; the harness immediately killed and reaped
that owned process. This confirms a frontend hangup spin with the same CPU/signal
signature, without proving that it explains every process in the original incident.

The selected Crossterm 0.29 mio backend retries terminal EOF/EIO inside its read
loop without returning to the application. Consequently the signal handler can
set its exit flag while the dashboard remains trapped inside input polling.
Tmux removing a pane does not prove that this frontend exited. The workspace test's
temporary executable name explains the `kiro-agent's $b` process-name variant.

The source fix selects Crossterm's error-propagating `use-dev-tty` backend and
checks terminal HUP/ERR before each dashboard input cycle. A new bounded regression
requires frontends to exit after idle/attached PTY hangup, checks low CPU use of an
intentionally detached host, and checks host EOF without spinning or prompt replay.
Workspace tests now verify that closed pane frontends exit and guarantee emergency
cleanup only for still-identical processes launched by that private test server.
The regression runs against debug builds and extracted branch beta binaries.

The focused regressions pass locally. Full CI/optimized-beta results are recorded
on the corresponding PR; authenticated Kiro/browser acceptance remains separate.
