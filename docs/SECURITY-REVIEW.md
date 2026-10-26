# Security review and scan results

## 2026-10-26 dependency and secret scan

| Check | Scope | Result |
| --- | --- | --- |
| cargo-audit 0.22.2 | Entire Cargo.lock, including development and target-specific dependencies (155 crates) | 0 known vulnerabilities; no advisory warnings |
| Trivy filesystem vulnerability scan | Cargo.lock | 0 known vulnerabilities |
| Trivy secret scan | Working tree, excluding .git, target and ignored probe output | 0 detected secrets |

RustSec database: 1,378 advisories, commit
`a78282a1414d6c28cc228c4eae3570451affb0dc` (updated 2026-10-26).
No advisory is ignored or waived. Scan results are time-bound and do not assess
the separately installed Kiro CLI, operating-system packages, or unknown defects.
Machine-readable reports were kept outside the repository; no scan report embeds
private source content or possible secret values in the published project.

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
