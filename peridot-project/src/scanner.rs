use std::fs;
use std::path::Path;

use peridot_common::PeriResult;

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
        detect_root_markers(root, &mut profile);
        detect_structure(root, &mut profile);
        profile.git = detect_git_state(root);
        Ok(profile)
    }
}

fn detect_root_markers(root: &Path, profile: &mut ProjectProfile) {
    if root.join("Cargo.toml").exists() {
        profile.languages.push(LanguageInfo {
            name: "Rust".to_string(),
            ratio: 100,
        });
        profile.build_system = BuildSystem::Cargo;
        profile.commands = ProjectCommands {
            build: Some("cargo build --workspace".to_string()),
            test: Some("cargo test --workspace".to_string()),
            lint: Some("cargo clippy --workspace -- -D warnings".to_string()),
            format: Some("cargo fmt --all".to_string()),
            dev: None,
        };
        profile.important_dirs.extend(
            ["src", "crates"]
                .iter()
                .map(|dir| root.join(dir))
                .filter(|path| path.exists()),
        );
        return;
    }

    if root.join("package.json").exists() {
        profile.languages.push(LanguageInfo {
            name: "JavaScript".to_string(),
            ratio: 80,
        });
        if root.join("tsconfig.json").exists() {
            profile.languages.push(LanguageInfo {
                name: "TypeScript".to_string(),
                ratio: 20,
            });
        }
        profile.build_system = BuildSystem::Node;
        profile.commands = ProjectCommands {
            build: Some("npm run build".to_string()),
            test: Some("npm test".to_string()),
            lint: Some("npm run lint".to_string()),
            format: None,
            dev: Some("npm run dev".to_string()),
        };
        return;
    }

    if root.join("pyproject.toml").exists() || root.join("requirements.txt").exists() {
        profile.languages.push(LanguageInfo {
            name: "Python".to_string(),
            ratio: 100,
        });
        profile.build_system = BuildSystem::Python;
        profile.commands = ProjectCommands {
            build: None,
            test: Some("pytest".to_string()),
            lint: Some("ruff check .".to_string()),
            format: Some("ruff format .".to_string()),
            dev: None,
        };
        return;
    }

    if root.join("go.mod").exists() {
        profile.languages.push(LanguageInfo {
            name: "Go".to_string(),
            ratio: 100,
        });
        profile.build_system = BuildSystem::Go;
        profile.commands = ProjectCommands {
            build: Some("go build ./...".to_string()),
            test: Some("go test ./...".to_string()),
            lint: None,
            format: Some("gofmt -w .".to_string()),
            dev: None,
        };
        return;
    }

    if root.join("Makefile").exists() {
        profile.build_system = BuildSystem::Make;
        profile.commands.build = Some("make".to_string());
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
            profile.sub_projects.push(SubProject {
                name: name.to_string(),
                root: path,
                build_system,
            });
        }
    }
    if profile.sub_projects.len() > 1 && profile.structure == ProjectStructure::Single {
        profile.structure = ProjectStructure::Monorepo;
    }
}
