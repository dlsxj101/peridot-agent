use std::path::{Path, PathBuf};

/// Text attachment loaded from a workspace-local file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextAttachment {
    pub(crate) path: String,
    pub(crate) bytes: usize,
    pub(crate) media_type: Option<String>,
    pub(crate) content: Option<String>,
}

pub(crate) fn load_text_attachment(
    project_root: &Path,
    requested_path: &str,
    max_bytes: usize,
) -> Result<TextAttachment, String> {
    let requested_path = requested_path.trim();
    if requested_path.is_empty() {
        return Err("attach: missing path".to_string());
    }
    let root = project_root
        .canonicalize()
        .map_err(|err| format!("attach: failed to resolve workspace root: {err}"))?;
    let path = resolve_attachment_path(&root, requested_path);
    let canonical = path
        .canonicalize()
        .map_err(|err| format!("attach: failed to read {requested_path}: {err}"))?;
    if !canonical.starts_with(&root) {
        return Err("attach: refusing to read outside the workspace".to_string());
    }
    let metadata = std::fs::metadata(&canonical)
        .map_err(|err| format!("attach: failed to stat {}: {err}", canonical.display()))?;
    let byte_len = metadata.len() as usize;
    let media_type = image_media_type(&canonical);
    if byte_len > max_bytes && media_type.is_none() {
        return Err(format!(
            "attach: {} is {} bytes, above the {} byte limit",
            display_relative(&root, &canonical),
            byte_len,
            max_bytes
        ));
    }
    if byte_len > max_bytes {
        return Ok(TextAttachment {
            path: display_relative(&root, &canonical),
            bytes: byte_len,
            media_type,
            content: None,
        });
    }
    let bytes = std::fs::read(&canonical)
        .map_err(|err| format!("attach: failed to read {}: {err}", canonical.display()))?;
    let content = match String::from_utf8(bytes) {
        Ok(content) => Some(content),
        Err(_) if media_type.is_some() => None,
        Err(_) => {
            return Err("attach: only UTF-8 text or image files are supported for now".to_string());
        }
    };
    Ok(TextAttachment {
        path: display_relative(&root, &canonical),
        bytes: byte_len,
        media_type,
        content,
    })
}

pub(crate) fn attachment_plan_reminder(attachment: &TextAttachment) -> String {
    if let Some(content) = attachment.content.as_ref() {
        return format!(
            "[attachment]\npath: {}\nbytes: {}\n\n```text\n{}\n```",
            attachment.path, attachment.bytes, content
        );
    }
    format!(
        "[attachment]\npath: {}\nbytes: {}\nmedia_type: {}\ncontent: <not inlined; image attachment placeholder>",
        attachment.path,
        attachment.bytes,
        attachment
            .media_type
            .as_deref()
            .unwrap_or("application/octet-stream")
    )
}

fn resolve_attachment_path(root: &Path, requested_path: &str) -> PathBuf {
    let path = PathBuf::from(requested_path);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn image_media_type(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let media_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    Some(media_type.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_rejects_parent_escape() {
        let root = temp_project("escape");
        let outside = root.parent().unwrap().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let err = load_text_attachment(&root, "../outside.txt", 1024).unwrap_err();
        assert!(err.contains("outside the workspace"));
        let _ = std::fs::remove_file(outside);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn attachment_loads_relative_utf8_file() {
        let root = temp_project("utf8");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
        let attachment = load_text_attachment(&root, "src/lib.rs", 1024).unwrap();
        assert_eq!(attachment.path, "src/lib.rs");
        assert!(attachment_plan_reminder(&attachment).contains("pub fn demo()"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn attachment_uses_placeholder_for_images() {
        let root = temp_project("image");
        std::fs::write(root.join("screen.png"), [0x89, b'P', b'N', b'G']).unwrap();
        let attachment = load_text_attachment(&root, "screen.png", 1024).unwrap();
        assert_eq!(attachment.media_type.as_deref(), Some("image/png"));
        assert!(attachment.content.is_none());
        assert!(attachment_plan_reminder(&attachment).contains("image attachment placeholder"));
        let _ = std::fs::remove_dir_all(root);
    }

    fn temp_project(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "peridot-attach-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }
}
