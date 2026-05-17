//! `@file` auto-complete picker.
//!
//! Triggered when the user types `@` at the start of a token in the chat
//! input. The picker walks the project root (cached on first open) for
//! tracked files, fuzzy-matches them against whatever follows the `@`, and
//! lets the operator tab-complete an absolute-ish project path into the
//! input buffer. The model then sees the literal `@path/to/file.rs` token
//! and can decide to call `file_read` on it -- we do NOT inline the file
//! body, mirroring the Claude Code default where mentions are a navigation
//! hint, not a forced expansion.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Maximum number of suggestions surfaced in the floating picker. Keeps
/// the overlay readable on narrow terminals and stops a 5,000-file
/// repository from streaming everything at once.
pub const AT_PICKER_LIMIT: usize = 8;

/// Files we never want to expose in the picker even when they match by
/// suffix. Hidden directories and build artefacts blow the index up with
/// noise the operator would never `@`-mention.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".peridot",
    ".idea",
    ".vscode",
];

/// Active state of the `@file` overlay. Cleared as soon as the user
/// leaves the `@token` region (types a space, deletes the `@`, etc.).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AtPicker {
    /// Current substring after the `@`. When empty (picker just opened)
    /// the renderer shows the first `AT_PICKER_LIMIT` files.
    pub query: String,
    /// Highlighted suggestion index. Wraps within `[0, suggestions.len())`.
    pub selected: usize,
    /// Byte offset in `state.input` where the `@` lives, so the
    /// insertion path can replace `@<query>` with the chosen path
    /// without splitting on cursor position.
    pub token_start: usize,
}

/// Walks `project_root` once and returns a sorted, deduplicated list of
/// relative paths suitable for the picker index. Hidden / build /
/// dependency directories are pruned at the directory boundary so we
/// don't spend time descending into `target/` or `node_modules/`.
///
/// Limited to `cap` total entries to keep the cache small on very large
/// repositories; common project shapes (under a few thousand source
/// files) fit comfortably.
pub fn build_file_index(project_root: &Path, cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    walk(project_root, project_root, &mut out, cap);
    out.sort();
    out.dedup();
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>, cap: usize) {
    if out.len() >= cap {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= cap {
            return;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if file_type.is_dir() {
            if SKIP_DIRS.iter().any(|skip| *skip == name_str) {
                continue;
            }
            if name_str.starts_with('.') {
                // Other dot-prefixed dirs (e.g., user-added) -- skip by
                // convention. `.peridot` is already in SKIP_DIRS; this
                // line covers everything else with `.foo` shape.
                continue;
            }
            walk(root, &path, out, cap);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if name_str.starts_with('.') {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Fuzzy match `query` against `paths`. Match rules (in priority order):
/// exact suffix > basename starts-with > basename contains > any-position
/// contains. Each tier preserves the alphabetic order of the source list
/// so the picker stays stable as the user types one char at a time.
pub fn filter_paths<'a>(paths: &'a [String], query: &str) -> Vec<&'a String> {
    if query.is_empty() {
        return paths.iter().take(AT_PICKER_LIMIT).collect();
    }
    let q = query.to_ascii_lowercase();
    let mut exact_suffix = Vec::new();
    let mut starts_with = Vec::new();
    let mut basename_contains = Vec::new();
    let mut path_contains = Vec::new();
    for path in paths {
        let lower = path.to_ascii_lowercase();
        let basename = lower.rsplit('/').next().unwrap_or(lower.as_str());
        if basename == q {
            exact_suffix.push(path);
            continue;
        }
        if basename.starts_with(&q) {
            starts_with.push(path);
            continue;
        }
        if basename.contains(&q) {
            basename_contains.push(path);
            continue;
        }
        if lower.contains(&q) {
            path_contains.push(path);
        }
    }
    let mut combined: Vec<&'a String> = Vec::new();
    combined.extend(exact_suffix);
    combined.extend(starts_with);
    combined.extend(basename_contains);
    combined.extend(path_contains);
    combined.into_iter().take(AT_PICKER_LIMIT).collect()
}

/// Inspects `input` and the byte offset `cursor` to decide whether the
/// caret is currently inside an `@token`. Returns the (token_start_byte,
/// query_substring) if so. The token starts at the first `@` looking
/// backwards from the cursor; the token ends as soon as we hit a
/// whitespace character or another `@`, so `@foo bar` only matches when
/// the cursor is on the `foo` chunk.
pub fn current_at_token(input: &str, cursor_chars: usize) -> Option<(usize, String)> {
    let cursor_byte = char_index_to_byte(input, cursor_chars);
    let bytes = input.as_bytes();
    let mut start_byte = cursor_byte;
    while start_byte > 0 {
        let prev = start_byte - 1;
        let ch = bytes[prev] as char;
        if ch == '@' {
            // Token must start at line start or follow whitespace so
            // `email@host` doesn't activate the picker.
            let valid = prev == 0
                || bytes[prev - 1] == b' '
                || bytes[prev - 1] == b'\t'
                || bytes[prev - 1] == b'\n';
            if valid {
                let query = input[prev + 1..cursor_byte].to_string();
                if query.chars().any(char::is_whitespace) {
                    return None;
                }
                return Some((prev, query));
            }
            return None;
        }
        if ch.is_whitespace() {
            return None;
        }
        start_byte = prev;
    }
    None
}

fn char_index_to_byte(input: &str, char_index: usize) -> usize {
    input
        .char_indices()
        .map(|(byte, _)| byte)
        .nth(char_index)
        .unwrap_or(input.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_prioritises_basename_matches() {
        let paths = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "tests/main.rs".to_string(),
            "docs/notes.md".to_string(),
        ];
        let matches = filter_paths(&paths, "main");
        assert_eq!(
            matches.into_iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["src/main.rs", "tests/main.rs"]
        );
    }

    #[test]
    fn empty_query_returns_prefix_window() {
        let paths: Vec<String> = (0..20).map(|i| format!("file{i}.rs")).collect();
        let matches = filter_paths(&paths, "");
        assert_eq!(matches.len(), AT_PICKER_LIMIT);
    }

    #[test]
    fn at_token_requires_word_boundary() {
        // `@foo` at start of input → picker opens with query "foo".
        let (start, query) = current_at_token("@foo", 4).expect("token");
        assert_eq!(start, 0);
        assert_eq!(query, "foo");

        // `email@host` → no picker because `@` is preceded by a non-space char.
        assert!(current_at_token("email@host", 10).is_none());

        // Space between cursor and `@` cancels the token.
        assert!(current_at_token("@foo bar", 8).is_none());
    }
}
