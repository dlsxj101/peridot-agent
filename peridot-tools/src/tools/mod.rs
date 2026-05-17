pub(crate) mod agent;
mod command;
mod file;
mod git;
pub(crate) mod plan;
pub(crate) mod shell;
mod verify;
mod web;

pub use agent::{
    AgentAskUserTool, AgentDelegateTool, AgentDoneTool, AgentMemorySearchTool, AgentScratchpadTool,
};
pub use file::{FileListTool, FilePatchTool, FileReadTool, FileSearchTool, FileWriteTool};
pub use git::{
    GhPrCreateTool, GhPrMergeTool, GhPrStatusTool, GitBranchTool, GitCommitTool, GitDiffTool,
    GitLogTool, GitPushTool, GitStatusTool,
};
pub use plan::{PlanCreateTool, PlanUpdateTool};
pub use shell::ShellExecTool;
pub use verify::{VerifyBuildTool, VerifyLintTool, VerifyTestTool};
pub use web::{WebFetchTool, WebSearchTool};

use peridot_common::PeriResult;

use crate::ToolRegistry;

/// Registers the initial built-in tools required by the engine loop.
pub fn register_builtin_tools(registry: &mut ToolRegistry) -> PeriResult<()> {
    registry.register(ShellExecTool)?;
    registry.register(FileReadTool)?;
    registry.register(FileWriteTool)?;
    registry.register(FilePatchTool)?;
    registry.register(FileSearchTool)?;
    registry.register(FileListTool)?;
    registry.register(PlanCreateTool)?;
    registry.register(PlanUpdateTool)?;
    registry.register(GitStatusTool)?;
    registry.register(GitDiffTool)?;
    registry.register(GitLogTool)?;
    registry.register(GitCommitTool)?;
    registry.register(GitBranchTool)?;
    registry.register(GitPushTool)?;
    registry.register(GhPrCreateTool)?;
    registry.register(GhPrStatusTool)?;
    registry.register(GhPrMergeTool)?;
    registry.register(VerifyBuildTool)?;
    registry.register(VerifyTestTool)?;
    registry.register(VerifyLintTool)?;
    registry.register(WebSearchTool)?;
    registry.register(WebFetchTool)?;
    registry.register(AgentScratchpadTool)?;
    registry.register(AgentAskUserTool)?;
    registry.register(AgentDelegateTool)?;
    registry.register(AgentMemorySearchTool)?;
    registry.register(AgentDoneTool)?;
    Ok(())
}
