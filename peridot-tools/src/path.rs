use std::path::{Path, PathBuf};

use peridot_common::{PeriError, PeriResult};
use serde_json::Value;

use crate::ToolContext;

pub fn ensure_within_project(root: &Path, candidate: &Path) -> PeriResult<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|err| PeriError::PathBoundary(root.join(err.to_string())))?;
    let path = if candidate.exists() {
        candidate
            .canonicalize()
            .map_err(|_| PeriError::PathBoundary(candidate.to_path_buf()))?
    } else {
        let parent = candidate.parent().unwrap_or_else(|| Path::new("."));
        let parent = parent
            .canonicalize()
            .map_err(|_| PeriError::PathBoundary(candidate.to_path_buf()))?;
        parent.join(candidate.file_name().unwrap_or_default())
    };

    if path.starts_with(&root) {
        Ok(path)
    } else {
        Err(PeriError::PathBoundary(path))
    }
}

pub(crate) fn required_str<'a>(params: &'a Value, key: &str) -> PeriResult<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| PeriError::Tool(format!("missing string parameter: {key}")))
}

pub(crate) fn workspace_path(ctx: &ToolContext, params: &Value) -> PeriResult<PathBuf> {
    let raw = required_str(params, "path")?;
    let candidate = ctx.project_root.join(raw);
    let path = ensure_within_project(&ctx.project_root, &candidate)?;
    ensure_not_denied(ctx, &path)?;
    Ok(path)
}

fn ensure_not_denied(ctx: &ToolContext, path: &Path) -> PeriResult<()> {
    for denied in &ctx.denied_paths {
        let denied = if denied.is_absolute() {
            denied.clone()
        } else {
            ctx.project_root.join(denied)
        };
        let denied = if denied.exists() {
            denied.canonicalize().unwrap_or(denied)
        } else {
            let parent = denied.parent().unwrap_or(&ctx.project_root);
            parent
                .canonicalize()
                .map(|parent| parent.join(denied.file_name().unwrap_or_default()))
                .unwrap_or(denied)
        };
        if path.starts_with(&denied) {
            return Err(PeriError::PermissionDenied(format!(
                "AGENTS boundary blocks modification of {}",
                path.display()
            )));
        }
    }
    Ok(())
}
