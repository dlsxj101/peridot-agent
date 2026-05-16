//! Project scanning and AGENTS.md profile types.

mod agents;
mod git;
mod scanner;
#[cfg(test)]
mod tests;
mod types;

pub use scanner::ProjectScanner;
pub use types::{
    BuildSystem, CiConfig, GitState, LanguageInfo, ProjectCommands, ProjectPreferences,
    ProjectProfile, ProjectStructure, SubProject,
};

/// Returns the first AGENTS-style instruction file found under `root`, in
/// priority order: `.peridot/AGENTS.md`, `AGENTS.md`, `CLAUDE.md`, and
/// `.github/copilot-instructions.md`. Returns `None` when no file exists.
pub fn locate_agents_md(root: &std::path::Path) -> Option<std::path::PathBuf> {
    agents::find_agents_file(root)
}
