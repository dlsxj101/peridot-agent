use std::fs;

use peridot_common::{ExecutionMode, PermissionMode};

use crate::{BuildSystem, ProjectScanner, ProjectStructure};

#[test]
fn detects_rust_workspace() {
    let root = std::env::temp_dir().join(format!("peridot-project-rust-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();

    let profile = ProjectScanner::new().scan(&root).unwrap();

    assert_eq!(profile.build_system, BuildSystem::Cargo);
    assert_eq!(profile.structure, ProjectStructure::Workspace);
    assert_eq!(
        profile.commands.build.as_deref(),
        Some("cargo build --workspace")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parses_agents_boundaries() {
    let root = std::env::temp_dir().join(format!("peridot-project-agents-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("AGENTS.md"),
        "## boundaries\n- DO NOT modify generated/\n",
    )
    .unwrap();

    let profile = ProjectScanner::new().scan(&root).unwrap();

    assert!(profile.has_agents_md);
    assert_eq!(
        profile.agents_md_overrides,
        vec!["boundaries: - DO NOT modify generated/"]
    );
    assert_eq!(profile.boundaries, vec!["generated/"]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parses_agents_preferences() {
    let root = std::env::temp_dir().join(format!(
        "peridot-project-agents-preferences-{}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("AGENTS.md"),
        "## preferences\n\
         default_mode: goal\n\
         default_permission: safe\n\
         ask_before_install: true\n\
         ask_before_delete: false\n\
         auto_commit: true\n\
         commit_frequency: logical_unit\n\
         branch_prefix: peridot/\n",
    )
    .unwrap();

    let profile = ProjectScanner::new().scan(&root).unwrap();

    assert_eq!(profile.preferences.default_mode, Some(ExecutionMode::Goal));
    assert_eq!(
        profile.preferences.default_permission,
        Some(PermissionMode::Safe)
    );
    assert_eq!(profile.preferences.ask_before_install, Some(true));
    assert_eq!(profile.preferences.ask_before_delete, Some(false));
    assert_eq!(profile.preferences.auto_commit, Some(true));
    assert_eq!(
        profile.preferences.commit_frequency.as_deref(),
        Some("logical_unit")
    );
    assert_eq!(
        profile.preferences.branch_prefix.as_deref(),
        Some("peridot/")
    );
    fs::remove_dir_all(root).unwrap();
}
