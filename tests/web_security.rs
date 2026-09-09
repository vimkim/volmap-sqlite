//! Production-process HTTP contract; the harness uses only Python's standard library.
#[test]
fn production_web_security() {
    let output = std::process::Command::new("python3")
        .arg("tests/support/web_security.py")
        .arg(env!("CARGO_BIN_EXE_volmap-sqlite"))
        .output()
        .expect("run HTTP security harness");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
