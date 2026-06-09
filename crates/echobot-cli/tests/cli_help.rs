//! Integration smoke test: verify the `echobot` CLI binary's `--help`
//! text advertises the three top-level subcommands (chat, app, gateway).
//!
//! The binary is rebuilt as part of `cargo test --workspace`, so the
//! `env!("CARGO_BIN_EXE_echobot")` macro gives us the right path
//! against the freshly-compiled artifact.

use std::process::Command;

#[test]
fn cli_help_lists_all_subcommands() {
    let bin = env!("CARGO_BIN_EXE_echobot");
    let output = Command::new(bin)
        .arg("--help")
        .output()
        .expect("failed to execute echobot --help");

    assert!(
        output.status.success(),
        "echobot --help exited with {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for needle in ["chat", "app", "gateway"] {
        assert!(
            stdout.contains(needle),
            "expected `--help` output to mention `{needle}`, got:\n{stdout}",
        );
    }
}
