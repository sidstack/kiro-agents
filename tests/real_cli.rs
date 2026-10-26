#[cfg(feature = "probe")]
#[test]
#[ignore = "requires real Kiro; isolated, no credentials or model calls"]
fn isolated_v2_cli_capabilities() {
    let status = std::process::Command::new("python3")
        .env("KIRO_AGENTS_PROBE_BIN", env!("CARGO_BIN_EXE_kiro-agents"))
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("probes/v2.py"))
        .status()
        .expect("python3 must be available");
    assert!(status.success());
}
