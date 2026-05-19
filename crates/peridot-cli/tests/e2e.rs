#![cfg(feature = "e2e")]

use std::fs;
use std::io::{BufRead, BufReader, Write};
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

#[test]
fn env_headless_enables_scriptable_output() {
    let root = temp_project("env-headless");
    let response_file = root.join("responses.jsonl");
    fs::write(
        &response_file,
        r#"{"action":"agent_done","parameters":{"summary":"env headless done"}}
"#,
    )
    .unwrap();

    let output = Command::new(peridot())
        .env("PERIDOT_HEADLESS", "1")
        .args([
            "--project",
            root.to_str().unwrap(),
            "--output",
            "json",
            "--mock-response-file",
            response_file.to_str().unwrap(),
            "run",
            "finish from env",
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
    assert_eq!(
        summary["turns"][0]["tool_result"]["summary"],
        "env headless done"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn headless_text_output_stays_text_for_agent_runs() {
    let root = temp_project("headless-text");
    let response_file = root.join("responses.jsonl");
    fs::write(
        &response_file,
        r#"{"action":"agent_done","parameters":{"summary":"text mode done"}}
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
            "finish as text",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("stopped=Done turns=1"));
    assert!(!stdout.trim_start().starts_with('{'));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn headless_max_turns_exits_three() {
    let root = temp_project("max-turns");
    let response_file = root.join("responses.jsonl");
    fs::write(
        &response_file,
        r#"{"action":"plan_update","parameters":{"update":"still working"}}
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
            "--max-turns",
            "1",
            "--mock-response-file",
            response_file.to_str().unwrap(),
            "run",
            "never finish",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["stopped_reason"], "MaxTurns");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn headless_budget_exhaustion_exits_two() {
    let root = temp_project("budget");
    let response_file = root.join("responses.jsonl");
    fs::write(
        &response_file,
        r#"{"text":"{\"action\":\"plan_update\",\"parameters\":{\"update\":\"costly step\"}}","usage":{"input_tokens":1,"output_tokens":1,"cache_read_tokens":0,"cache_creation_tokens":0,"estimated_cost_usd":1.25}}
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
            "--budget",
            "0.50",
            "--mock-response-file",
            response_file.to_str().unwrap(),
            "run",
            "spend too much",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["stopped_reason"], "Budget");
    assert_eq!(summary["usage"]["estimated_cost_usd"], 1.25);

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
fn daemon_responds_to_version_echo_and_shutdown() {
    // Drives the daemon end-to-end over real stdio so the framing,
    // flushing, and dispatcher all run in the production code path.
    // Extension developers should be able to read this test and crib
    // the wire format exactly.
    let root = temp_project("daemon");

    let mut child = Command::new(peridot())
        .args(["--project", root.to_str().unwrap(), "daemon"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // 1. peridot.version
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"peridot.version"}}"#
    )
    .unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(parsed["id"], 1);
    assert!(parsed["result"]["version"].is_string());

    // 2. peridot.echo
    line.clear();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"peridot.echo","params":{{"text":"hi"}}}}"#
    )
    .unwrap();
    reader.read_line(&mut line).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(parsed["id"], 2);
    assert_eq!(parsed["result"]["echo"], "hi");

    // 3. shutdown — closes the loop cleanly.
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":3,"method":"shutdown"}}"#).unwrap();
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "daemon exited with {status:?}");

    fs::remove_dir_all(root).unwrap();
}
