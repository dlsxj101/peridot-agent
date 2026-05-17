//! Tool contracts, registry, and permission helpers.

pub mod audit;
pub mod hooks;
mod mcp_adapter;
mod path;
mod registry;
mod tools;

pub use mcp_adapter::{McpToolAdapter, register_mcp_tools};
pub use path::ensure_within_project;
pub use registry::{Tool, ToolContext, ToolDescriptor, ToolRegistry};
pub use tools::{
    AgentAskUserTool, AgentDelegateTool, AgentDoneTool, AgentMemorySearchTool, AgentScratchpadTool,
    FileListTool, FilePatchTool, FileReadTool, FileSearchTool, FileWriteTool, GhPrCreateTool,
    GhPrMergeTool, GhPrStatusTool, GitBranchTool, GitCommitTool, GitDiffTool, GitLogTool,
    GitPushTool, GitStatusTool, PlanCreateTool, PlanUpdateTool, ShellExecTool, VerifyBuildTool,
    VerifyLintTool, VerifyTestTool, register_builtin_tools,
};

#[cfg(test)]
mod tests;
