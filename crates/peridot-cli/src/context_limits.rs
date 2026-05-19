use super::*;

pub(super) fn project_context_limits_from_config(
    project_root: &Path,
    config: &ContextConfig,
) -> ContextLimits {
    let mut limits = project_context_limits(project_root);
    limits.hard_limit_tokens = config.hard_limit;
    limits.compaction_threshold_tokens = config.compaction_threshold;
    limits.offload_threshold_chars = config.offload_threshold_chars;
    limits
}
