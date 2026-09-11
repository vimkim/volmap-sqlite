use std::process::Command;

#[test]
fn executable_reports_version_and_content_identity_without_a_database_or_build_paths() {
    let output = Command::new(env!("CARGO_BIN_EXE_volmap-sqlite"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with(concat!("volmap-sqlite ", env!("CARGO_PKG_VERSION"))));
    assert!(text.contains("build "));
    assert!(text.contains("rustc "));
    assert!(!text.contains(env!("CARGO_MANIFEST_DIR")));
    let identity = text
        .split("build ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    assert_eq!(identity.len(), 64);
    assert!(identity.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
