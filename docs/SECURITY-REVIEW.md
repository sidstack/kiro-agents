# Security review and scan results

## 2026-10-26 dependency and secret scan

| Check | Scope | Result |
| --- | --- | --- |
| cargo-audit 0.22.2 | Entire Cargo.lock, including development and target-specific dependencies (155 crates) | 0 known vulnerabilities; no advisory warnings |
| Trivy 0.76.1 filesystem vulnerability scan | Cargo.lock | 0 known vulnerabilities |
| Trivy 0.76.1 secret scan | Working tree, excluding .git, target and ignored probe output | 0 detected secrets |

RustSec database: 1,378 advisories, commit
`a78282a1414d6c28cc228c4eae3570451affb0dc` (updated 2026-10-26).
No advisory is ignored or waived. Scan results are time-bound and do not assess
the separately installed Kiro CLI, operating-system packages, or unknown defects.
Machine-readable reports were kept outside the repository; no scan report embeds
private source content or possible secret values in the published project.

Workspace-branch follow-up (2026-10-26): cargo-audit 0.22.2 with RustSec database
commit `3279c99911354c400e1d41e47c7d60e39258b147` checked the current complete lockfile
(156 dependencies); strict `make audit` passed without vulnerabilities or warnings.
No new Rust dependency was added for workspaces. This audit does not scan the
separately installed tmux binary or OS packages; the earlier Trivy result above
is separate evidence, not a new scan of this branch.

Reproduce with:

```sh
cargo install cargo-audit --version 0.22.2 --locked
cargo audit --deny warnings
trivy fs --scanners vuln,secret --skip-dirs .git --skip-dirs target --skip-dirs probes/out .
```

## Existing boundaries

- Host state is metadata only; real transcripts and child environments are not logged.
- Wrapper state and its Unix socket use private filesystem permissions.
- Fresh launches and live-process reattach only; no automatic prompt/tool replay.
- Signals target owned, unreaped child handles, never persisted/foreign PIDs.
- Kiro config and profiles are not rewritten. Interactive tools are never auto-approved.
- Attach is raw PTY passthrough; terminal output is not a trusted structured data source.
- Discovery has bounded output/runtime and cancellation/reaping of its owned process group.

These controls do not isolate mutually untrusted sessions under one OS account.

## Distribution controls

Release and test workflows use immutable action commit pins, minimal token
permissions, read-only PR workflows and disabled checkout credential persistence.
Release jobs audit/test the exact tag and verify Cargo/tag/binary version agreement.
Binary archives contain only the executable, license and public usage/release docs;
SHA256SUMS accompanies each release. Build output and probes are not committed or
shipped as package content. Weekly RustSec scans and Dependabot updates cover new
advisories and dependency updates. See [release policy](RELEASING.md).

Branch beta builds use read-only tokens and run the full suite plus a strict audit
before packaging. The extracted optimized executable is then exercised through all
all fake-Kiro PTY scenarios. Publication runs separately from default-branch code:
it checks the originating run, source SHA, beta version, filenames, archive members
and checksum without executing the downloaded binary. Fork PRs cannot publish.
Betas are explicit prereleases and do not replace latest stable; these checks do not
prove compatibility with authenticated Kiro or every terminal/browser environment.
