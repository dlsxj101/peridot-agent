#![cfg(feature = "e2e")]

use std::fs;
use std::process::Command;

fn peridot() -> &'static str {
    env!("CARGO_BIN_EXE_peridot")
}

fn temp_project(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("peridot-e2e-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn setup_scan_and_verify_work_in_headless_mode() {
    let root = temp_project("setup");

    let setup = Command::new(peridot())
        .args(["--project", root.to_str().unwrap(), "--headless", "setup"])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    assert!(root.join(".peridot/config.toml").exists());

    let scan = Command::new(peridot())
        .args([
            "--project",
            root.to_str().unwrap(),
            "--headless",
            "--output",
            "json",
            "scan",
        ])
        .output()
        .unwrap();
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    assert!(String::from_utf8_lossy(&scan.stdout).contains("\"root\""));

    let verify = Command::new(peridot())
        .args([
            "--project",
            root.to_str().unwrap(),
            "--headless",
            "--output",
            "json",
            "verify",
        ])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mock_agent_loop_creates_file() {
    let root = temp_project("mock");
    let response_file = root.join("responses.jsonl");
    fs::write(
        &response_file,
        r#"{"action":"file_write","parameters":{"path":"hello.py","content":"print(\"Hello World\")\n"}}
{"action":"agent_done","parameters":{"summary":"created hello.py"}}
"#,
    )
    .unwrap();

    let output = Command::new(peridot())
        .args([
            "--project",
            root.to_str().unwrap(),
            "--headless",
            "--output",
            "json",
            "--mock-response-file",
            response_file.to_str().unwrap(),
            "run",
            "create hello.py",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("hello.py")).unwrap(),
        "print(\"Hello World\")\n"
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn lifecycle_hooks_run_for_mock_agent_loop() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_project("hooks");
    let hooks_dir = root.join(".peridot/hooks");
    fs::create_dir_all(&hooks_dir).unwrap();
    let script = hooks_dir.join("lifecycle.sh");
    fs::write(&script, "#!/bin/sh\necho \"$1:$2\" >> lifecycle.log\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        root.join(".peridot/config.toml"),
        r#"
[[hooks.lifecycle]]
event = "session_*"
run = ".peridot/hooks/lifecycle.sh {session_id} {status}"
"#,
    )
    .unwrap();
    let response_file = root.join("responses.jsonl");
    fs::write(
        &response_file,
        r#"{"action":"agent_done","parameters":{"summary":"done"}}
"#,
    )
    .unwrap();

    let output = Command::new(peridot())
        .args([
            "--project",
            root.to_str().unwrap(),
            "--headless",
            "--mock-response-file",
            response_file.to_str().unwrap(),
            "run",
            "finish",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(root.join("lifecycle.log")).unwrap();
    assert!(log.contains(":running"));
    assert!(log.contains(":Done"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn headless_direct_tool_failure_exits_four() {
    let root = temp_project("headless-failure");
    let output = Command::new(peridot())
        .args([
            "--project",
            root.to_str().unwrap(),
            "--headless",
            "--output",
            "json",
            r#"{"action":"verify_build","parameters":{"command":"exit 7"}}"#,
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"success\": false"));

    fs::remove_dir_all(root).unwrap();
}
