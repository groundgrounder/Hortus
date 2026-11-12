use std::fs;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_hortus")
}

#[test]
fn guide_prints_the_built_in_text_without_creating_directories() {
    let root = tempfile::TempDir::new().unwrap();
    let config = root.path().join("config");
    let output = Command::new(binary())
        .arg("guide")
        .env("HOME", root.path())
        .env("HORTUS_CONFIG_DIR", &config)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        hortus_config::AGENT_GUIDE
    );
    assert!(
        !config.exists(),
        "hortus guide created the config directory"
    );
}

#[test]
fn guide_override_is_written_verbatim_and_errors_are_english() {
    let root = tempfile::TempDir::new().unwrap();
    let config = root.path().join("config");
    fs::create_dir(&config).unwrap();
    let guide = config.join("agent-guide.md");
    let text = "  spaces \"quotes\" $(x);\n中文\n";
    fs::write(&guide, text).unwrap();
    let output = Command::new(binary())
        .arg("guide")
        .env("HOME", root.path())
        .env("HORTUS_CONFIG_DIR", &config)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), text);

    fs::write(&guide, b"  \n").unwrap();
    let empty = Command::new(binary())
        .arg("guide")
        .env("HOME", root.path())
        .env("HORTUS_CONFIG_DIR", &config)
        .output()
        .unwrap();
    assert!(!empty.status.success());
    assert_eq!(
        String::from_utf8(empty.stderr).unwrap().trim(),
        "agent-guide.md is empty"
    );

    fs::write(&guide, b"\xff\xfe").unwrap();
    let invalid = Command::new(binary())
        .arg("guide")
        .env("HOME", root.path())
        .env("HORTUS_CONFIG_DIR", &config)
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert_eq!(
        String::from_utf8(invalid.stderr).unwrap().trim(),
        "agent-guide.md is not valid UTF-8"
    );
}

#[test]
fn unknown_arguments_exit_non_zero_and_help_stays_english() {
    let root = tempfile::TempDir::new().unwrap();
    for arguments in [vec!["guide", "extra"], vec!["unknown"], vec!["mermaid"]] {
        let output = Command::new(binary())
            .args(&arguments)
            .env("HOME", root.path())
            .env("HORTUS_CONFIG_DIR", root.path().join("config"))
            .output()
            .unwrap();
        assert!(!output.status.success(), "{arguments:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.is_ascii(), "errors must stay English: {stderr:?}");
    }
    let help = Command::new(binary()).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("hortus guide")
    );
}
