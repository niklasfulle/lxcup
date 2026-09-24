use std::process::Command;

#[test]
fn version_command_runs_through_the_binary_entrypoint() {
    let output = Command::new(env!("CARGO_BIN_EXE_lxcup-cli"))
        .arg("version")
        .output()
        .expect("CLI binary must start");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("lxcup "));
}
