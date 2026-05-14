#![cfg(feature = "e2e")]

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

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

#[test]
fn top_level_headless_goal_outputs_json_summary() {
    let root = temp_project("headless-goal");
    let response_file = root.join("responses.jsonl");
    fs::write(
        &response_file,
        r#"{"action":"agent_done","parameters":{"summary":"goal completed"}}
{"satisfied":true,"reason":"objective verified"}
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
            "--mode",
            "goal",
            "--permission",
            "yolo",
            "--mock-response-file",
            response_file.to_str().unwrap(),
            "finish the goal",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["stopped_reason"], "Done");
    assert_eq!(summary["turns"][0]["tool_name"], "agent_done");
    assert_eq!(
        summary["turns"][0]["tool_result"]["summary"],
        "goal completed"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn headless_reads_task_from_stdin_pipe() {
    let root = temp_project("stdin");
    let response_file = root.join("responses.jsonl");
    fs::write(
        &response_file,
        r#"{"action":"agent_done","parameters":{"summary":"piped task done"}}
"#,
    )
    .unwrap();

    let mut child = Command::new(peridot())
        .args([
            "--project",
            root.to_str().unwrap(),
            "--headless",
            "--output",
            "json",
            "--mock-response-file",
            response_file.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"finish from stdin\n")
        .unwrap();

    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["stopped_reason"], "Done");
    assert_eq!(
        summary["turns"][0]["tool_result"]["summary"],
        "piped task done"
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

#[cfg(unix)]
#[test]
fn plan_completed_lifecycle_hook_runs_for_done_plan() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_project("plan-hook");
    let hooks_dir = root.join(".peridot/hooks");
    fs::create_dir_all(&hooks_dir).unwrap();
    let script = hooks_dir.join("lifecycle.sh");
    fs::write(&script, "#!/bin/sh\necho \"$1:$2:$3\" >> lifecycle.log\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        root.join(".peridot/config.toml"),
        r#"
[[hooks.lifecycle]]
event = "plan_completed"
run = ".peridot/hooks/lifecycle.sh plan {status} {summary}"
"#,
    )
    .unwrap();
    let response_file = root.join("responses.jsonl");
    fs::write(
        &response_file,
        r#"{"action":"agent_done","parameters":{"summary":"planned"}}
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
            "plan",
            "plan it",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(root.join("lifecycle.log")).unwrap();
    assert!(log.contains("plan:done:plan_file=todo.md"));

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn verify_command_runs_verification_passed_hook() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_project("verify-hook");
    let hooks_dir = root.join(".peridot/hooks");
    fs::create_dir_all(&hooks_dir).unwrap();
    let script = hooks_dir.join("verify.sh");
    fs::write(&script, "#!/bin/sh\necho \"$1:$2\" >> verify.log\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        root.join(".peridot/config.toml"),
        r#"
[[hooks.event]]
event = "verification_passed"
run = ".peridot/hooks/verify.sh {stage} {status}"
"#,
    )
    .unwrap();

    let output = Command::new(peridot())
        .args(["--project", root.to_str().unwrap(), "--headless", "verify"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(root.join("verify.log")).unwrap();
    assert!(log.contains("diff_review:passed"));

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
