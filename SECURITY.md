# Security policy

The latest 0.3.x release is supported on Linux x86_64. Use current Kiro CLI and
follow its authentication and tool-approval controls. Older local MVP builds are
not maintained separately.

Report vulnerabilities privately using this repository's GitHub Security →
Report a vulnerability facility. If private reporting is not enabled, contact a
maintainer privately through GitHub before disclosing a vulnerability publicly.
Do not post credentials, real conversations, OAuth URLs or private host logs.
Provide a sanitized reproduction using temporary state and fake Kiro where possible.

The wrapper runs with your user permissions; it is not a sandbox. Another
process under your user account can access your metadata/socket. The host owns
its child processes, reads/writes private wrapper metadata and forwards raw
terminal output. Native Kiro controls its own tools and approvals. Treat an
agent and the directory it runs in as trusted local software.

CI checks the entire Cargo.lock dependency graph with RustSec on pushes, PRs,
weekly schedules and before attaching release binaries. Warnings also fail the
audit; there are no ignored advisories. Dependency automation proposes updates.
A clean scan means no known matching advisory at scan time, not proof that the
program, operating system or separately installed Kiro CLI has no vulnerabilities.

See [security review and scan results](docs/SECURITY-REVIEW.md).
