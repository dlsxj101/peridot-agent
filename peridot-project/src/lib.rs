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
