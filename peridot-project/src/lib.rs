//! Project scanning and AGENTS.md profile types.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use peridot_common::PeriResult;
use serde::{Deserialize, Serialize};

/// Detected project language.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LanguageInfo {
    /// Language name.
    pub name: String,
    /// Rough percentage of repository signals.
    pub ratio: u8,
}

/// Common project build system.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildSystem {
    /// Rust Cargo workspace or package.
    Cargo,
    /// Node package manager based project.
    Node,
    /// Python project.
    Python,
    /// Go modules.
    Go,
    /// Make based project.
    Make,
    /// Unknown or unsupported build system.
    #[default]
    Unknown,
}

/// Project verification commands.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectCommands {
    /// Build command.
    pub build: Option<String>,
    /// Test command.
    pub test: Option<String>,
    /// Lint command.
    pub lint: Option<String>,
    /// Format command.
    pub format: Option<String>,
    /// Development server command.
    pub dev: Option<String>,
}

/// Repository structure category.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStructure {
    /// Single project.
    #[default]
    Single,
    /// Cargo/npm/etc workspace.
    Workspace,
    /// Multiple independent projects.
    Monorepo,
}

/// Detected subproject.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubProject {
    /// Subproject name.
    pub name: String,
    /// Subproject root path.
    pub root: PathBuf,
    /// Subproject build system.
    pub build_system: BuildSystem,
}

/// Git snapshot for a scanned project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GitState {
    /// Current branch name.
    pub branch: Option<String>,
    /// Number of dirty files.
    pub dirty_files: usize,
}

/// CI configuration summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CiConfig {
    /// CI provider name.
    pub provider: String,
    /// Commands inferred from CI jobs.
    pub commands: Vec<String>,
}

/// Project profile injected into the harness prompt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectProfile {
    /// Project name.
    pub name: String,
    /// Project root.
    pub root: PathBuf,
    /// Detected languages.
    pub languages: Vec<LanguageInfo>,
    /// Detected frameworks.
    pub frameworks: Vec<String>,
    /// Primary build system.
    pub build_system: BuildSystem,
    /// Common project commands.
    pub commands: ProjectCommands,
    /// Repository structure.
    pub structure: ProjectStructure,
    /// Subprojects in a monorepo/workspace.
    pub sub_projects: Vec<SubProject>,
    /// Important directories.
    pub important_dirs: Vec<PathBuf>,
    /// Git snapshot.
    pub git: Option<GitState>,
    /// Top dependency names.
    pub top_dependencies: Vec<String>,
    /// CI config summary.
    pub ci: Option<CiConfig>,
    /// Whether an AGENTS-style instruction file exists.
    pub has_agents_md: bool,
    /// Parsed AGENTS overrides.
    pub agents_md_overrides: Vec<String>,
}

impl ProjectProfile {
    /// Creates a minimal project profile.
    pub fn minimal(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project")
            .to_string();
        Self {
            name,
            root,
            languages: Vec::new(),
            frameworks: Vec::new(),
            build_system: BuildSystem::Unknown,
            commands: ProjectCommands::default(),
            structure: ProjectStructure::Single,
            sub_projects: Vec::new(),
            important_dirs: Vec::new(),
            git: None,
            top_dependencies: Vec::new(),
            ci: None,
            has_agents_md: false,
            agents_md_overrides: Vec::new(),
        }
    }
}

/// Project scanner skeleton.
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
        profile.agents_md_overrides = parse_agents_overrides(root)?;
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

fn find_agents_file(root: &Path) -> Option<PathBuf> {
    [
        ".peridot/AGENTS.md",
        "AGENTS.md",
        "CLAUDE.md",
        ".github/copilot-instructions.md",
    ]
    .iter()
    .map(|path| root.join(path))
    .find(|path| path.exists())
}

fn parse_agents_overrides(root: &Path) -> PeriResult<Vec<String>> {
    let Some(path) = find_agents_file(root) else {
        return Ok(Vec::new());
    };
    let content = fs::read_to_string(&path)
        .map_err(|err| peridot_common::PeriError::Parse(format!("{}: {err}", path.display())))?;
    let mut overrides = Vec::new();
    let mut section = String::new();
    for line in content.lines() {
        if let Some(stripped) = line.strip_prefix("## ") {
            section = stripped.trim().to_ascii_lowercase();
            continue;
        }
        if matches!(section.as_str(), "commands" | "boundaries" | "preferences") {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                overrides.push(format!("{section}: {trimmed}"));
            }
        }
    }
    Ok(overrides)
}

fn detect_git_state(root: &Path) -> Option<GitState> {
    if !root.join(".git").exists() {
        return None;
    }
    let branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    let dirty_files = Command::new("git")
        .args(["status", "--short"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).lines().count())
        .unwrap_or(0);
    Some(GitState {
        branch,
        dirty_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rust_workspace() {
        let root =
            std::env::temp_dir().join(format!("peridot-project-rust-{}", std::process::id()));
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
        let root =
            std::env::temp_dir().join(format!("peridot-project-agents-{}", std::process::id()));
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
        fs::remove_dir_all(root).unwrap();
    }
}
