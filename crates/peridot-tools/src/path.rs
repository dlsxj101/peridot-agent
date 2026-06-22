use std::path::{Component, Path, PathBuf};

use peridot_common::{PeriError, PeriResult};
use serde_json::Value;

use crate::ToolContext;

pub fn ensure_within_project(root: &Path, candidate: &Path) -> PeriResult<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|err| PeriError::PathBoundary(root.join(err.to_string())))?;
    // Reject any candidate that contains a `..` segment outright. For a
    // non-existent candidate we canonicalize only the deepest *existing*
    // ancestor and re-attach the tail raw — so a `..` in that tail would
    // survive and let `fs::write` / `create_dir_all` follow it out of the
    // project root, defeating the lexical `starts_with(&root)` check below
    // (which does not interpret `..`). Mirrors the guard in
    // `tools/skill.rs`. `existing`-path canonicalisation already folds
    // `..`, but rejecting up front keeps the boundary obvious and covers
    // the partial-resolution path too.
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PeriError::PathBoundary(candidate.to_path_buf()));
    }
    let path = if candidate.exists() {
        candidate
            .canonicalize()
            .map_err(|_| PeriError::PathBoundary(candidate.to_path_buf()))?
    } else {
        // The candidate path doesn't exist yet, so we can't canonicalize
        // it directly. Walk up until we hit an existing ancestor we
        // *can* canonicalize, then re-attach the missing tail. This is
        // what makes `file_write` work for files inside nested
        // directories that the agent intends to create in the same
        // turn — without this, every `mkdir -p`-implying write fails
        // with a confusing "path outside project boundary" error
        // because the parent directory hasn't been created yet.
        resolve_partial_canonical(candidate)
            .ok_or_else(|| PeriError::PathBoundary(candidate.to_path_buf()))?
    };

    // Defence in depth: lexically fold any residual `.`/`..` (without
    // touching the filesystem) before the boundary check, since
    // `starts_with` compares components literally and would otherwise be
    // fooled by a `..` that slipped through the partial-resolution tail.
    let path = lexically_normalize(&path);
    if path.starts_with(&root) {
        Ok(path)
    } else {
        Err(PeriError::PathBoundary(path))
    }
}

/// Folds `.` and `..` components without consulting the filesystem. Used as
/// a belt-and-braces normalisation before the lexical `starts_with(&root)`
/// boundary check; `..` at the very front (which would escape above the
/// root) is preserved so such paths still fail the boundary check.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Find the deepest existing ancestor of `candidate`, canonicalize it,
/// then re-attach the non-existing tail. Returns `None` only when none
/// of the path's components — not even the root — can be canonicalised,
/// which on Unix is essentially "the filesystem is broken." The returned
/// path is not itself guaranteed to be canonical past the live ancestor,
/// but `starts_with(&root)` is still correct because `root` is already
/// canonical and the live ancestor's canonical form is a superset of
/// the project root (or it's not, in which case the boundary check
/// catches it correctly).
fn resolve_partial_canonical(candidate: &Path) -> Option<PathBuf> {
    // Walk ancestors from deepest to shallowest looking for the first
    // one that actually exists on disk and can be canonicalised.
    let mut current = candidate;
    while let Some(parent) = current.parent() {
        if parent.as_os_str().is_empty() {
            break;
        }
        if let Ok(canonical_parent) = parent.canonicalize() {
            // Rebuild the relative tail by stripping `parent` off
            // `candidate`. If the strip fails for any reason
            // (shouldn't, but be defensive), fall back to just
            // appending the original file_name.
            let tail = candidate.strip_prefix(parent).ok();
            return Some(match tail {
                Some(tail) => canonical_parent.join(tail),
                None => canonical_parent.join(candidate.file_name()?),
            });
        }
        current = parent;
    }
    // Last-resort: try `.` as the working directory anchor. This
    // covers paths relative to cwd whose entire ancestry is still
    // virtual (e.g. running the agent from a freshly-created tmp
    // dir with no existing children).
    let cwd = Path::new(".").canonicalize().ok()?;
    Some(cwd.join(candidate))
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

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
// The test module sits in the middle of the file because the
// downstream helper (`ensure_not_denied`) only matters for one of the
// `workspace_path` callers and reads better grouped with it.
mod tests {
    use super::*;
    use std::fs;

    fn tmp_project(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "peridot-path-test-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn allows_existing_file_inside_root() {
        let root = tmp_project("existing");
        let file = root.join("hello.txt");
        fs::write(&file, "hi").unwrap();
        let resolved = ensure_within_project(&root, &file).unwrap();
        assert!(resolved.ends_with("hello.txt"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn allows_new_file_in_existing_subdir() {
        let root = tmp_project("new_in_existing");
        fs::create_dir_all(root.join("sub")).unwrap();
        let candidate = root.join("sub/new.txt");
        let resolved = ensure_within_project(&root, &candidate).unwrap();
        assert!(resolved.ends_with("sub/new.txt"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn allows_new_file_in_deeply_nested_nonexistent_dir() {
        // This is the regression from the Java+Vue scaffold session:
        // the LLM tries to write `backend/src/main/java/.../Foo.java`
        // before any of those directories exist. Old behaviour: hard
        // PathBoundary error because parent.canonicalize() failed.
        // New behaviour: ancestor walk finds the project root, joins
        // the relative tail, and the boundary check passes.
        let root = tmp_project("deep_nested");
        let candidate = root.join("backend/src/main/java/com/example/Foo.java");
        let resolved = ensure_within_project(&root, &candidate).unwrap();
        assert!(resolved.ends_with("backend/src/main/java/com/example/Foo.java"));
        // And critically, the resolved path is still inside the
        // canonical root — the boundary check protects us.
        let canonical_root = root.canonicalize().unwrap();
        assert!(resolved.starts_with(&canonical_root));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_new_file_outside_root() {
        // Even with the partial-canonical resolution path, anything
        // that resolves outside the project root must still fail.
        let root = tmp_project("outside");
        let outside = std::env::temp_dir().join("definitely-not-under-root.txt");
        let result = ensure_within_project(&root, &outside);
        assert!(matches!(result, Err(PeriError::PathBoundary(_))));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_path_traversal_via_dotdot() {
        // `..` traversal should be caught by the canonicalisation step
        // — the live ancestor canonicalisation strips the `..` and we
        // compare against the root.
        let root = tmp_project("traversal");
        let candidate = root.join("../escape.txt");
        let result = ensure_within_project(&root, &candidate);
        assert!(matches!(result, Err(PeriError::PathBoundary(_))));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_dotdot_tail_through_nonexistent_dir() {
        // Regression: when the candidate doesn't exist, only the deepest
        // existing ancestor was canonicalised and the `..` tail survived,
        // so `<root>/nope/../../escaped.txt` passed the lexical boundary
        // check and let a write escape the project root. Must be rejected.
        let root = tmp_project("dotdot_tail");
        let candidate = root.join("nope/../../escaped.txt");
        let result = ensure_within_project(&root, &candidate);
        assert!(matches!(result, Err(PeriError::PathBoundary(_))));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lexically_normalize_folds_dot_and_dotdot() {
        assert_eq!(
            lexically_normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }
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
