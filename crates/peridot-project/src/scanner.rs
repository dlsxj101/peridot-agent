use std::fs;
use std::path::Path;

use peridot_common::PeriResult;
use serde_json::Value;

use crate::agents::{find_agents_file, parse_agents_file};
use crate::git::detect_git_state;
use crate::types::{
    BuildSystem, LanguageInfo, ProjectCommands, ProjectProfile, ProjectStructure, SubProject,
};

/// Project scanner for language, build-system, AGENTS, and git signals.
#[derive(Clone, Debug, Default)]
pub struct ProjectScanner;

impl ProjectScanner {
    /// Creates a new project scanner.
    pub fn new() -> Self {
        Self
    }

    /// Scans a project root and returns a minimal profile.
    pub fn scan(&self, root: impl AsRef<Path>) -> PeriResult<ProjectProfile> {
        let root = root.as_ref();
        let mut profile = ProjectProfile::minimal(root);
        profile.has_agents_md = find_agents_file(root).is_some();
        let agents = parse_agents_file(root)?;
        profile.agents_md_overrides = agents.overrides;
        profile.preferences = agents.preferences;
        profile.boundaries = agents.boundaries;
        // AGENTS.md `## commands` is the operator's explicit override.
        // Seed the profile with it *before* detection so the
        // "don't overwrite an already-filled slot" rule in
        // `fill_commands` lets AGENTS.md win over scanner guesses.
        profile.commands = agents.commands;
        detect_root_markers(root, &mut profile);
        detect_ci(root, &mut profile);
        detect_structure(root, &mut profile);
        // Monorepo fallback: when the root has none of the language
        // markers we know about, peek into common sub-directory
        // names (frontend, web, app, server, backend, …). Lots of
        // operator projects keep the Python / FastAPI backend at
        // the root level (no setup.py or pyproject) and the Vite /
        // npm frontend under `frontend/`; the verify_build tool
        // previously fell back to `cargo build --workspace` for
        // these and exited 127 because cargo wasn't installed.
        if profile.commands.build.is_none() {
            detect_monorepo_subdirs(root, &mut profile);
        }
        profile.git = detect_git_state(root);
        Ok(profile)
    }
}

/// Walks one level into common monorepo sub-directory names looking
/// for the language markers `detect_root_markers` checks at the root.
/// When a marker is found, the resulting command is prefixed with
/// `cd <subdir> &&` so the build runs from the right working directory.
/// Only fills commands that haven't already been set by the root pass.
fn detect_monorepo_subdirs(root: &Path, profile: &mut ProjectProfile) {
    const CANDIDATES: &[&str] = &[
        "frontend",
        "web",
        "client",
        "ui",
        "app",
        "apps/web",
        "apps/frontend",
        "packages/web",
        "packages/frontend",
        "backend",
        "server",
        "api",
        "service",
    ];
    for candidate in CANDIDATES {
        let sub = root.join(candidate);
        if !sub.is_dir() {
            continue;
        }
        // package.json (Node/Vite/Next/...)
        if let Ok(pkg) = fs::read_to_string(sub.join("package.json"))
            && let Ok(value) = serde_json::from_str::<Value>(&pkg)
        {
            push_language(profile, "JavaScript", 60);
            set_primary_build(profile, BuildSystem::Node);
            let has_script = |name: &str| {
                value
                    .get("scripts")
                    .and_then(|s| s.get(name))
                    .and_then(|v| v.as_str())
                    .is_some()
            };
            let cmd = |script: &str| format!("cd {candidate} && npm run {script}");
            fill_commands(
                &mut profile.commands,
                ProjectCommands {
                    build: has_script("build").then(|| cmd("build")),
                    test: has_script("test").then(|| cmd("test")),
                    lint: has_script("lint").then(|| cmd("lint")),
                    format: has_script("format").then(|| cmd("format")),
                    dev: has_script("dev").then(|| cmd("dev")),
                },
            );
        }
        // pyproject.toml / requirements.txt (Python/FastAPI/...)
        if sub.join("pyproject.toml").exists() || sub.join("requirements.txt").exists() {
            push_language(profile, "Python", 60);
            set_primary_build(profile, BuildSystem::Python);
            fill_commands(
                &mut profile.commands,
                ProjectCommands {
                    build: None,
                    test: Some(format!("cd {candidate} && pytest")),
                    lint: Some(format!("cd {candidate} && ruff check .")),
                    format: Some(format!("cd {candidate} && ruff format .")),
                    dev: None,
                },
            );
        }
    }
}

fn detect_root_markers(root: &Path, profile: &mut ProjectProfile) {
    if let Ok(cargo_toml) = fs::read_to_string(root.join("Cargo.toml")) {
        push_language(profile, "Rust", 100);
        set_primary_build(profile, BuildSystem::Cargo);
        fill_commands(
            &mut profile.commands,
            ProjectCommands {
                build: Some("cargo build --workspace".to_string()),
                test: Some("cargo test --workspace".to_string()),
                lint: Some("cargo clippy --workspace -- -D warnings".to_string()),
                format: Some("cargo fmt --all".to_string()),
                dev: None,
            },
        );
        detect_cargo_metadata(&cargo_toml, profile);
        add_existing_dirs(profile, root, &["src", "crates"]);
    }

    if let Ok(package_json) = fs::read_to_string(root.join("package.json")) {
        push_language(profile, "JavaScript", 80);
        if root.join("tsconfig.json").exists() {
            push_language(profile, "TypeScript", 20);
        }
        set_primary_build(profile, BuildSystem::Node);
        detect_package_json(root, &package_json, profile);
        add_existing_dirs(
            profile,
            root,
            &["src", "app", "pages", "components", "packages", "apps"],
        );
    }

    if let Ok(pyproject) = fs::read_to_string(root.join("pyproject.toml")) {
        push_language(profile, "Python", 100);
        set_primary_build(profile, BuildSystem::Python);
        detect_pyproject(&pyproject, profile);
        add_existing_dirs(profile, root, &["src", "tests"]);
    } else if let Ok(requirements) = fs::read_to_string(root.join("requirements.txt")) {
        push_language(profile, "Python", 100);
        set_primary_build(profile, BuildSystem::Python);
        fill_commands(
            &mut profile.commands,
            ProjectCommands {
                build: None,
                test: Some("pytest".to_string()),
                lint: Some("ruff check .".to_string()),
                format: Some("ruff format .".to_string()),
                dev: None,
            },
        );
        detect_python_dependency_text(&requirements, profile);
        add_existing_dirs(profile, root, &["src", "tests"]);
    }

    if root.join("go.mod").exists() {
        push_language(profile, "Go", 100);
        set_primary_build(profile, BuildSystem::Go);
        fill_commands(
            &mut profile.commands,
            ProjectCommands {
                build: Some("go build ./...".to_string()),
                test: Some("go test ./...".to_string()),
                lint: None,
                format: Some("gofmt -w .".to_string()),
                dev: None,
            },
        );
    }

    if root.join("Makefile").exists() {
        set_primary_build(profile, BuildSystem::Make);
        profile
            .commands
            .build
            .get_or_insert_with(|| "make".to_string());
    }

    // Gradle: `build.gradle` (Groovy) or `build.gradle.kts` (Kotlin DSL).
    // The wrapper script (`./gradlew`) is preferred when present because
    // it pins the Gradle version per project.
    if root.join("build.gradle").exists() || root.join("build.gradle.kts").exists() {
        push_language(profile, "Java", 70);
        if root.join("build.gradle.kts").exists() {
            push_language(profile, "Kotlin", 30);
        }
        set_primary_build(profile, BuildSystem::Gradle);
        let wrapper = if root.join("gradlew").exists() {
            "./gradlew"
        } else {
            "gradle"
        };
        fill_commands(
            &mut profile.commands,
            ProjectCommands {
                build: Some(format!("{wrapper} build")),
                test: Some(format!("{wrapper} test")),
                lint: None,
                format: None,
                dev: None,
            },
        );
        add_existing_dirs(profile, root, &["src/main", "src/test", "app"]);
    }

    // Maven: `pom.xml` at the root signals a Java project. `mvnw` is the
    // wrapper script when committed.
    if root.join("pom.xml").exists() {
        push_language(profile, "Java", 100);
        set_primary_build(profile, BuildSystem::Maven);
        let wrapper = if root.join("mvnw").exists() {
            "./mvnw"
        } else {
            "mvn"
        };
        fill_commands(
            &mut profile.commands,
            ProjectCommands {
                build: Some(format!("{wrapper} compile")),
                test: Some(format!("{wrapper} test")),
                lint: None,
                format: None,
                dev: None,
            },
        );
        add_existing_dirs(profile, root, &["src/main", "src/test"]);
    }

    // CMake: `CMakeLists.txt` flags C / C++. The typical out-of-source
    // build dir is `build/`, so we surface `cmake --build build` which
    // works with the default Ninja or Make generator.
    if root.join("CMakeLists.txt").exists() {
        push_language(profile, "C++", 100);
        set_primary_build(profile, BuildSystem::CMake);
        fill_commands(
            &mut profile.commands,
            ProjectCommands {
                build: Some("cmake --build build".to_string()),
                test: Some("ctest --test-dir build".to_string()),
                lint: None,
                format: None,
                dev: None,
            },
        );
        add_existing_dirs(profile, root, &["src", "include", "tests"]);
    }

    // Swift Package Manager: `Package.swift` at the root.
    if root.join("Package.swift").exists() {
        push_language(profile, "Swift", 100);
        set_primary_build(profile, BuildSystem::SwiftPm);
        fill_commands(
            &mut profile.commands,
            ProjectCommands {
                build: Some("swift build".to_string()),
                test: Some("swift test".to_string()),
                lint: None,
                format: None,
                dev: None,
            },
        );
        add_existing_dirs(profile, root, &["Sources", "Tests"]);
    }

    // .NET: any *.csproj / *.fsproj / *.vbproj at the root (or a .sln).
    if has_dotnet_project_file(root) {
        push_language(profile, "C#", 100);
        set_primary_build(profile, BuildSystem::Dotnet);
        fill_commands(
            &mut profile.commands,
            ProjectCommands {
                build: Some("dotnet build".to_string()),
                test: Some("dotnet test".to_string()),
                lint: None,
                format: Some("dotnet format".to_string()),
                dev: None,
            },
        );
    }
}

fn has_dotnet_project_file(root: &Path) -> bool {
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && (name.ends_with(".csproj")
                    || name.ends_with(".fsproj")
                    || name.ends_with(".vbproj")
                    || name.ends_with(".sln"))
            {
                return true;
            }
        }
    }
    false
}

fn set_primary_build(profile: &mut ProjectProfile, build_system: BuildSystem) {
    if profile.build_system == BuildSystem::Unknown {
        profile.build_system = build_system;
    }
}

fn fill_commands(target: &mut ProjectCommands, detected: ProjectCommands) {
    if target.build.is_none() {
        target.build = detected.build;
    }
    if target.test.is_none() {
        target.test = detected.test;
    }
    if target.lint.is_none() {
        target.lint = detected.lint;
    }
    if target.format.is_none() {
        target.format = detected.format;
    }
    if target.dev.is_none() {
        target.dev = detected.dev;
    }
}

fn push_language(profile: &mut ProjectProfile, name: &str, ratio: u8) {
    if !profile
        .languages
        .iter()
        .any(|language| language.name == name)
    {
        profile.languages.push(LanguageInfo {
            name: name.to_string(),
            ratio,
        });
    }
}

fn push_unique(values: &mut Vec<String>, value: impl Into<String>) {
    let value = value.into();
    if !value.trim().is_empty() && !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn add_existing_dirs(profile: &mut ProjectProfile, root: &Path, dirs: &[&str]) {
    for dir in dirs {
        let path = root.join(dir);
        if path.exists()
            && !profile
                .important_dirs
                .iter()
                .any(|existing| existing == &path)
        {
            profile.important_dirs.push(path);
        }
    }
}

fn detect_cargo_metadata(cargo_toml: &str, profile: &mut ProjectProfile) {
    for (dependency, framework) in [
        ("axum", "Axum"),
        ("actix-web", "Actix Web"),
        ("tauri", "Tauri"),
        ("leptos", "Leptos"),
        ("dioxus", "Dioxus"),
        ("bevy", "Bevy"),
        ("tokio", "Tokio"),
    ] {
        if cargo_toml.contains(dependency) {
            push_unique(&mut profile.frameworks, framework);
            push_unique(&mut profile.top_dependencies, dependency);
        }
    }
}

fn detect_package_json(root: &Path, package_json: &str, profile: &mut ProjectProfile) {
    let manager = node_package_manager(root);
    let package = serde_json::from_str::<Value>(package_json).unwrap_or(Value::Null);
    let scripts = package.get("scripts").and_then(Value::as_object);
    let command_for = |name: &str| {
        scripts
            .and_then(|scripts| scripts.get(name))
            .and_then(Value::as_str)
            .map(|_| node_script_command(manager, name))
    };
    fill_commands(
        &mut profile.commands,
        ProjectCommands {
            build: command_for("build"),
            test: command_for("test"),
            lint: command_for("lint"),
            format: command_for("format"),
            dev: command_for("dev"),
        },
    );
    if profile.commands.test.is_none() {
        profile.commands.test = Some(node_script_command(manager, "test"));
    }

    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(dependencies) = package.get(section).and_then(Value::as_object) {
            for dependency in dependencies.keys().take(20) {
                push_unique(&mut profile.top_dependencies, dependency);
            }
        }
    }

    for (dependency, framework) in [
        ("next", "Next.js"),
        ("react", "React"),
        ("vue", "Vue"),
        ("svelte", "Svelte"),
        ("@angular/core", "Angular"),
        ("express", "Express"),
        ("vite", "Vite"),
        ("tailwindcss", "Tailwind CSS"),
        ("astro", "Astro"),
        ("@remix-run/react", "Remix"),
    ] {
        if profile
            .top_dependencies
            .iter()
            .any(|existing| existing == dependency)
        {
            push_unique(&mut profile.frameworks, framework);
        }
    }

    if package.get("workspaces").is_some() && profile.structure == ProjectStructure::Single {
        profile.structure = ProjectStructure::Workspace;
    }
}

fn node_package_manager(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if root.join("yarn.lock").exists() {
        "yarn"
    } else if root.join("bun.lockb").exists() || root.join("bun.lock").exists() {
        "bun"
    } else {
        "npm"
    }
}

fn node_script_command(manager: &str, script: &str) -> String {
    match (manager, script) {
        ("npm", "test") => "npm test".to_string(),
        ("npm", _) => format!("npm run {script}"),
        ("yarn", _) => format!("yarn {script}"),
        ("pnpm", _) => format!("pnpm {script}"),
        ("bun", _) => format!("bun run {script}"),
        _ => format!("{manager} run {script}"),
    }
}

fn detect_pyproject(pyproject: &str, profile: &mut ProjectProfile) {
    let lower = pyproject.to_ascii_lowercase();
    fill_commands(
        &mut profile.commands,
        ProjectCommands {
            build: lower
                .contains("[build-system]")
                .then(|| "python -m build".to_string()),
            test: lower.contains("pytest").then(|| "pytest".to_string()),
            lint: lower.contains("ruff").then(|| "ruff check .".to_string()),
            format: lower.contains("ruff").then(|| "ruff format .".to_string()),
            dev: None,
        },
    );
    if lower.contains("black") && profile.commands.format.is_none() {
        profile.commands.format = Some("black .".to_string());
    }
    detect_python_dependency_text(pyproject, profile);
}

fn detect_python_dependency_text(text: &str, profile: &mut ProjectProfile) {
    for (dependency, framework) in [
        ("django", "Django"),
        ("fastapi", "FastAPI"),
        ("flask", "Flask"),
        ("pydantic", "Pydantic"),
        ("pytest", "Pytest"),
        ("ruff", "Ruff"),
    ] {
        if text.to_ascii_lowercase().contains(dependency) {
            push_unique(&mut profile.top_dependencies, dependency);
            push_unique(&mut profile.frameworks, framework);
        }
    }
}

fn detect_ci(root: &Path, profile: &mut ProjectProfile) {
    let workflows = root.join(".github/workflows");
    let Ok(entries) = fs::read_dir(workflows) else {
        return;
    };
    let mut commands = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            continue;
        };
        if !matches!(extension, "yml" | "yaml") {
            continue;
        }
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        for command in github_actions_run_commands(&content) {
            push_unique(&mut commands, command);
        }
    }
    if commands.is_empty() {
        return;
    }
    infer_commands_from_ci(&commands, &mut profile.commands);
    profile.ci = Some(crate::types::CiConfig {
        provider: "GitHub Actions".to_string(),
        commands,
    });
}

fn github_actions_run_commands(content: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start().trim_start_matches("- ");
        let Some(rest) = trimmed.strip_prefix("run:") else {
            continue;
        };
        let rest = rest.trim();
        if rest == "|" || rest == ">" {
            while let Some(next) = lines.peek() {
                let next_trimmed = next.trim();
                if next_trimmed.is_empty() {
                    lines.next();
                    continue;
                }
                if !next.starts_with(' ') && !next.starts_with('\t') {
                    break;
                }
                push_unique(&mut commands, next_trimmed.to_string());
                lines.next();
            }
        } else if !rest.is_empty() {
            push_unique(
                &mut commands,
                rest.trim_matches('"').trim_matches('\'').to_string(),
            );
        }
    }
    commands
}

fn infer_commands_from_ci(commands: &[String], profile_commands: &mut ProjectCommands) {
    for command in commands {
        let lower = command.to_ascii_lowercase();
        if profile_commands.build.is_none()
            && (lower.contains("cargo build")
                || lower.contains("npm run build")
                || lower.contains("pnpm build")
                || lower.contains("yarn build")
                || lower.contains("go build"))
        {
            profile_commands.build = Some(command.clone());
        }
        if profile_commands.test.is_none()
            && (lower.contains("cargo test")
                || lower == "npm test"
                || lower.contains("npm run test")
                || lower.contains("pnpm test")
                || lower.contains("pytest")
                || lower.contains("go test"))
        {
            profile_commands.test = Some(command.clone());
        }
        if profile_commands.lint.is_none()
            && (lower.contains("cargo clippy")
                || lower.contains("npm run lint")
                || lower.contains("pnpm lint")
                || lower.contains("ruff check"))
        {
            profile_commands.lint = Some(command.clone());
        }
    }
}

fn detect_structure(root: &Path, profile: &mut ProjectProfile) {
    if let Ok(cargo_toml) = fs::read_to_string(root.join("Cargo.toml"))
        && cargo_toml.contains("[workspace]")
    {
        profile.structure = ProjectStructure::Workspace;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if matches!(name, ".git" | "target" | "node_modules" | ".peridot") {
            continue;
        }
        let build_system = if path.join("Cargo.toml").exists() {
            Some(BuildSystem::Cargo)
        } else if path.join("package.json").exists() {
            Some(BuildSystem::Node)
        } else if path.join("pyproject.toml").exists() {
            Some(BuildSystem::Python)
        } else {
            None
        };
        if let Some(build_system) = build_system {
            push_sub_project(profile, name.to_string(), path, build_system);
            continue;
        }
        if matches!(name, "apps" | "packages" | "crates" | "services")
            && let Ok(children) = fs::read_dir(&path)
        {
            for child in children.flatten() {
                let child_path = child.path();
                if !child_path.is_dir() {
                    continue;
                }
                let child_name = child_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string();
                if let Some(build_system) = subproject_build_system(&child_path) {
                    push_sub_project(
                        profile,
                        format!("{name}/{child_name}"),
                        child_path,
                        build_system,
                    );
                }
            }
        }
    }
    if profile.sub_projects.len() > 1 && profile.structure == ProjectStructure::Single {
        profile.structure = ProjectStructure::Monorepo;
    }
}

fn subproject_build_system(path: &Path) -> Option<BuildSystem> {
    if path.join("Cargo.toml").exists() {
        Some(BuildSystem::Cargo)
    } else if path.join("package.json").exists() {
        Some(BuildSystem::Node)
    } else if path.join("pyproject.toml").exists() {
        Some(BuildSystem::Python)
    } else if path.join("go.mod").exists() {
        Some(BuildSystem::Go)
    } else {
        None
    }
}

fn push_sub_project(
    profile: &mut ProjectProfile,
    name: String,
    root: std::path::PathBuf,
    build_system: BuildSystem,
) {
    if profile
        .sub_projects
        .iter()
        .any(|existing| existing.root == root)
    {
        return;
    }
    profile.sub_projects.push(SubProject {
        name,
        root,
        build_system,
    });
}
