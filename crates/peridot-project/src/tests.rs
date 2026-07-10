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
fn detects_node_scripts_frameworks_dependencies_and_workspaces() {
    let root = std::env::temp_dir().join(format!("peridot-project-node-{}", std::process::id()));
    fs::create_dir_all(root.join("packages/web")).unwrap();
    fs::write(
        root.join("package.json"),
        r#"{
            "scripts": {
                "build": "next build",
                "test": "vitest run",
                "lint": "eslint .",
                "dev": "next dev"
            },
            "workspaces": ["packages/*"],
            "dependencies": {
                "next": "latest",
                "react": "latest",
                "express": "latest"
            },
            "devDependencies": {
                "vite": "latest",
                "tailwindcss": "latest"
            }
        }"#,
    )
    .unwrap();
    fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
    fs::write(root.join("tsconfig.json"), "{}").unwrap();
    fs::write(root.join("packages/web/package.json"), "{}").unwrap();

    let profile = ProjectScanner::new().scan(&root).unwrap();

    assert_eq!(profile.build_system, BuildSystem::Node);
    assert_eq!(profile.structure, ProjectStructure::Workspace);
    assert_eq!(profile.commands.build.as_deref(), Some("pnpm build"));
    assert_eq!(profile.commands.test.as_deref(), Some("pnpm test"));
    assert_eq!(profile.commands.lint.as_deref(), Some("pnpm lint"));
    assert_eq!(profile.commands.dev.as_deref(), Some("pnpm dev"));
    assert!(
        profile
            .languages
            .iter()
            .any(|language| language.name == "TypeScript")
    );
    assert!(profile.frameworks.contains(&"Next.js".to_string()));
    assert!(profile.frameworks.contains(&"React".to_string()));
    assert!(profile.frameworks.contains(&"Vite".to_string()));
    assert!(
        profile
            .top_dependencies
            .contains(&"tailwindcss".to_string())
    );
    assert_eq!(profile.sub_projects.len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn detects_python_tools_and_github_actions_commands() {
    let root =
        std::env::temp_dir().join(format!("peridot-project-python-ci-{}", std::process::id()));
    fs::create_dir_all(root.join(".github/workflows")).unwrap();
    fs::write(
        root.join("pyproject.toml"),
        r#"
[build-system]
requires = ["hatchling"]

[project]
dependencies = ["fastapi", "pydantic"]

[project.optional-dependencies]
dev = ["pytest", "ruff"]

[tool.pytest.ini_options]
testpaths = ["tests"]
"#,
    )
    .unwrap();
    fs::write(
        root.join(".github/workflows/ci.yml"),
        r#"
name: ci
jobs:
  test:
    steps:
      - run: python -m build
      - run: pytest
      - run: |
          ruff check .
          ruff format --check .
"#,
    )
    .unwrap();

    let profile = ProjectScanner::new().scan(&root).unwrap();

    assert_eq!(profile.build_system, BuildSystem::Python);
    assert_eq!(profile.commands.build.as_deref(), Some("python -m build"));
    assert_eq!(profile.commands.test.as_deref(), Some("pytest"));
    assert_eq!(profile.commands.lint.as_deref(), Some("ruff check ."));
    assert_eq!(profile.commands.format.as_deref(), Some("ruff format ."));
    assert!(profile.frameworks.contains(&"FastAPI".to_string()));
    assert!(profile.frameworks.contains(&"Pydantic".to_string()));
    assert!(profile.top_dependencies.contains(&"pytest".to_string()));
    let ci = profile.ci.unwrap();
    assert_eq!(ci.provider, "GitHub Actions");
    assert!(ci.commands.contains(&"ruff format --check .".to_string()));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn detects_rust_framework_dependencies_without_overwriting_cargo_defaults() {
    let root = std::env::temp_dir().join(format!(
        "peridot-project-rust-frameworks-{}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"

[dependencies]
axum = "0.8"
tokio = "1"
"#,
    )
    .unwrap();

    let profile = ProjectScanner::new().scan(&root).unwrap();

    assert_eq!(profile.build_system, BuildSystem::Cargo);
    assert_eq!(
        profile.commands.lint.as_deref(),
        Some("cargo clippy --workspace -- -D warnings")
    );
    assert!(profile.frameworks.contains(&"Axum".to_string()));
    assert!(profile.frameworks.contains(&"Tokio".to_string()));
    assert!(profile.top_dependencies.contains(&"axum".to_string()));
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
fn agents_commands_override_scanner_detection() {
    // A Cargo project would normally detect `cargo build --workspace`,
    // but an explicit AGENTS.md `## commands` build entry must win.
    let root = std::env::temp_dir().join(format!(
        "peridot-project-agents-commands-{}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::write(
        root.join("AGENTS.md"),
        "## commands\n\
         - build: just build\n\
         - test: just test\n\
         - lint: just clippy\n",
    )
    .unwrap();

    let profile = ProjectScanner::new().scan(&root).unwrap();

    assert_eq!(profile.commands.build.as_deref(), Some("just build"));
    assert_eq!(profile.commands.test.as_deref(), Some("just test"));
    assert_eq!(profile.commands.lint.as_deref(), Some("just clippy"));
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

#[test]
fn detects_gradle_kotlin_dsl() {
    let root = std::env::temp_dir().join(format!("peridot-project-gradle-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("build.gradle.kts"), "plugins {}\n").unwrap();
    let profile = ProjectScanner::new().scan(&root).unwrap();
    assert_eq!(profile.build_system, BuildSystem::Gradle);
    assert!(
        profile
            .languages
            .iter()
            .any(|l| l.name == "Kotlin" || l.name == "Java"),
        "expected Kotlin/Java languages: {:?}",
        profile.languages
    );
    assert!(
        profile
            .commands
            .test
            .as_deref()
            .unwrap_or("")
            .contains("test"),
        "expected gradle test command, got {:?}",
        profile.commands.test
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn detects_maven_pom() {
    let root = std::env::temp_dir().join(format!("peridot-project-maven-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("pom.xml"), "<project/>\n").unwrap();
    let profile = ProjectScanner::new().scan(&root).unwrap();
    assert_eq!(profile.build_system, BuildSystem::Maven);
    assert!(profile.languages.iter().any(|l| l.name == "Java"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn detects_cmake_project() {
    let root = std::env::temp_dir().join(format!("peridot-project-cmake-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\n",
    )
    .unwrap();
    let profile = ProjectScanner::new().scan(&root).unwrap();
    assert_eq!(profile.build_system, BuildSystem::CMake);
    assert!(profile.languages.iter().any(|l| l.name == "C++"));
    assert_eq!(
        profile.commands.build.as_deref(),
        Some("cmake --build build")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn detects_swift_package() {
    let root = std::env::temp_dir().join(format!("peridot-project-swift-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();
    let profile = ProjectScanner::new().scan(&root).unwrap();
    assert_eq!(profile.build_system, BuildSystem::SwiftPm);
    assert!(profile.languages.iter().any(|l| l.name == "Swift"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn detects_dotnet_csproj() {
    let root = std::env::temp_dir().join(format!("peridot-project-dotnet-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("App.csproj"), "<Project/>\n").unwrap();
    let profile = ProjectScanner::new().scan(&root).unwrap();
    assert_eq!(profile.build_system, BuildSystem::Dotnet);
    assert!(profile.languages.iter().any(|l| l.name == "C#"));
    fs::remove_dir_all(root).unwrap();
}
