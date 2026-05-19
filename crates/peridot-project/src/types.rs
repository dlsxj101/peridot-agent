use std::path::PathBuf;

use peridot_common::{ExecutionMode, PermissionMode};
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
    /// Gradle build (Java / Kotlin / Android).
    Gradle,
    /// Maven build (Java).
    Maven,
    /// CMake build (C / C++).
    CMake,
    /// Swift Package Manager.
    SwiftPm,
    /// .NET project (csproj / fsproj / vbproj).
    Dotnet,
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

/// Structured preferences parsed from AGENTS.md.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectPreferences {
    /// Default execution mode requested by the project.
    pub default_mode: Option<ExecutionMode>,
    /// Default permission mode requested by the project.
    pub default_permission: Option<PermissionMode>,
    /// Whether dependency installation commands require explicit user approval.
    pub ask_before_install: Option<bool>,
    /// Whether destructive delete/history commands require explicit user approval.
    pub ask_before_delete: Option<bool>,
    /// Whether the agent should commit completed logical units automatically.
    pub auto_commit: Option<bool>,
    /// Preferred commit cadence, such as "logical_unit".
    pub commit_frequency: Option<String>,
    /// Preferred branch prefix for agent-created branches.
    pub branch_prefix: Option<String>,
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
    /// Structured AGENTS preferences.
    pub preferences: ProjectPreferences,
    /// Parsed path boundaries that must not be modified.
    pub boundaries: Vec<String>,
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
            preferences: ProjectPreferences::default(),
            boundaries: Vec::new(),
        }
    }
}
