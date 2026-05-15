use peridot_common::{AgentPhase, ExecutionMode, PeriError, PeriResult, ToolGroup};

pub(crate) fn ensure_tool_allowed(
    mode: ExecutionMode,
    phase: AgentPhase,
    group: ToolGroup,
    name: &str,
) -> PeriResult<()> {
    if mode == ExecutionMode::Plan {
        let allowed = matches!(
            group,
            ToolGroup::File | ToolGroup::Git | ToolGroup::Plan | ToolGroup::Agent | ToolGroup::Web
        ) && !matches!(
            name,
            "file_write" | "file_patch" | "shell_exec" | "agent_delegate"
        );
        if !allowed {
            return Err(PeriError::PermissionDenied(format!(
                "Plan mode blocks tool {name}"
            )));
        }
    }

    if phase == AgentPhase::Verifying {
        let allowed = matches!(group, ToolGroup::Verify | ToolGroup::File | ToolGroup::Plan);
        if !allowed {
            return Err(PeriError::PermissionDenied(format!(
                "Verifying phase blocks tool {name}"
            )));
        }
    }

    Ok(())
}

/// Returns the tool groups available for a state.
pub fn allowed_tool_groups(mode: ExecutionMode, phase: AgentPhase) -> Vec<ToolGroup> {
    if mode == ExecutionMode::Plan {
        return vec![
            ToolGroup::File,
            ToolGroup::Git,
            ToolGroup::Plan,
            ToolGroup::Agent,
            ToolGroup::Web,
        ];
    }
    if phase == AgentPhase::Verifying {
        return vec![ToolGroup::Verify, ToolGroup::File, ToolGroup::Plan];
    }
    vec![
        ToolGroup::Shell,
        ToolGroup::File,
        ToolGroup::Git,
        ToolGroup::Web,
        ToolGroup::Plan,
        ToolGroup::Verify,
        ToolGroup::Agent,
        ToolGroup::Mcp,
    ]
}
