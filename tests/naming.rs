use std::process::Command;

#[test]
fn cli_uses_qued_name() {
    let version = Command::new(env!("CARGO_BIN_EXE_qued"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        concat!("qued ", env!("CARGO_PKG_VERSION"))
    );

    let help = Command::new(env!("CARGO_BIN_EXE_qued"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("Usage: qued")
    );
}

#[test]
fn startup_reads_qued_configuration_directory() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    std::fs::create_dir_all(config.join("qued")).unwrap();
    std::fs::write(
        config.join("qued/config.yaml"),
        "theme: daylight\nmodel: mock\n",
    )
    .unwrap();
    let check = Command::new(env!("CARGO_BIN_EXE_qued"))
        .arg("--check")
        .env("XDG_CONFIG_HOME", &config)
        .env("APPDATA", &config)
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("LOCALAPPDATA", root.path().join("state"))
        .output()
        .unwrap();
    assert!(check.status.success());
    let output = String::from_utf8(check.stdout).unwrap();
    assert!(output.starts_with(concat!("Qued ", env!("CARGO_PKG_VERSION"))));
    assert!(output.contains("daylight"));
    assert!(output.contains("MOCK backend"));
}
