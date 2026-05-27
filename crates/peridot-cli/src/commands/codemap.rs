use super::*;

const MAX_SYMBOL_FILE_BYTES: u64 = 256 * 1024;
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".peridot",
    "target",
    "node_modules",
    "dist",
    "build",
    ".next",
    ".idea",
    ".vscode",
];
const TODO_MARKERS: &[&str] = &["TODO", "FIXME", "HACK", "XXX", "BUG"];

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct CodeMapReport {
    pub walked_files: usize,
    pub symbols: Vec<CodeMapSymbol>,
    pub todos: Vec<CodeMapTodo>,
    pub symbols_truncated: bool,
    pub todos_truncated: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct CodeMapSymbol {
    pub path: String,
    pub line: usize,
    pub kind: String,
    pub name: String,
    pub signature: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct CodeMapTodo {
    pub path: String,
    pub line: usize,
    pub marker: String,
    pub text: String,
}

pub(crate) fn build_code_map(
    project_root: &Path,
    max_symbols: usize,
    max_todos: usize,
) -> CodeMapReport {
    let mut report = CodeMapReport {
        walked_files: 0,
        symbols: Vec::new(),
        todos: Vec::new(),
        symbols_truncated: false,
        todos_truncated: false,
    };
    walk_code_map(
        project_root,
        project_root,
        max_symbols,
        max_todos,
        &mut report,
    );
    report
}

fn walk_code_map(
    root: &Path,
    path: &Path,
    max_symbols: usize,
    max_todos: usize,
    report: &mut CodeMapReport,
) {
    if path.is_dir() {
        if should_skip_dir(path) {
            return;
        }
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        let mut entries = entries
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            walk_code_map(root, &entry, max_symbols, max_todos, report);
        }
        return;
    }
    if !is_source_file(path) {
        return;
    }
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.len() > MAX_SYMBOL_FILE_BYTES {
        return;
    }
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    report.walked_files += 1;
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    for (line_idx, line) in content.lines().enumerate() {
        if report.symbols.len() < max_symbols {
            if let Some((kind, name)) = detect_symbol(line, extension) {
                report.symbols.push(CodeMapSymbol {
                    path: relative.clone(),
                    line: line_idx + 1,
                    kind,
                    name,
                    signature: line.trim().to_string(),
                });
            }
        } else if detect_symbol(line, extension).is_some() {
            report.symbols_truncated = true;
        }
        if report.todos.len() < max_todos {
            if let Some((marker, text)) = detect_todo(line) {
                report.todos.push(CodeMapTodo {
                    path: relative.clone(),
                    line: line_idx + 1,
                    marker,
                    text,
                });
            }
        } else if detect_todo(line).is_some() {
            report.todos_truncated = true;
        }
    }
}

fn should_skip_dir(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    SKIP_DIRS.contains(&name)
}

fn is_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "py"
                | "go"
                | "java"
                | "kt"
                | "swift"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
        )
    )
}

fn detect_symbol(line: &str, extension: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') {
        return None;
    }
    match extension {
        "rs" => detect_rust_symbol(trimmed),
        "ts" | "tsx" | "js" | "jsx" => detect_js_symbol(trimmed),
        "py" => detect_python_symbol(trimmed),
        "go" => detect_go_symbol(trimmed),
        _ => detect_generic_symbol(trimmed),
    }
}

fn detect_rust_symbol(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("pub ")?;
    for kind in ["async fn", "fn", "struct", "enum", "trait", "impl", "mod"] {
        if let Some(rest) = rest.strip_prefix(kind) {
            return Some((kind.to_string(), symbol_name(rest)));
        }
    }
    None
}

fn detect_js_symbol(line: &str) -> Option<(String, String)> {
    let rest = line
        .strip_prefix("export ")
        .or_else(|| line.strip_prefix("export default "))?;
    for kind in [
        "async function",
        "function",
        "class",
        "interface",
        "type",
        "const",
        "let",
    ] {
        if let Some(rest) = rest.strip_prefix(kind) {
            return Some((kind.to_string(), symbol_name(rest)));
        }
    }
    None
}

fn detect_python_symbol(line: &str) -> Option<(String, String)> {
    if let Some(rest) = line.strip_prefix("class ") {
        return Some(("class".to_string(), symbol_name(rest)));
    }
    line.strip_prefix("def ")
        .map(|rest| ("def".to_string(), symbol_name(rest)))
}

fn detect_go_symbol(line: &str) -> Option<(String, String)> {
    if let Some(rest) = line.strip_prefix("func ") {
        let name = rest
            .strip_prefix('(')
            .and_then(|value| value.split_once(')'))
            .map(|(_, rest)| symbol_name(rest.trim_start()))
            .unwrap_or_else(|| symbol_name(rest));
        return Some(("func".to_string(), name));
    }
    line.strip_prefix("type ")
        .map(|rest| ("type".to_string(), symbol_name(rest)))
}

fn detect_generic_symbol(line: &str) -> Option<(String, String)> {
    for kind in [
        "public class",
        "public interface",
        "public enum",
        "public fun",
        "func",
    ] {
        if let Some(rest) = line.strip_prefix(kind) {
            return Some((kind.to_string(), symbol_name(rest)));
        }
    }
    None
}

fn detect_todo(line: &str) -> Option<(String, String)> {
    TODO_MARKERS.iter().find_map(|marker| {
        line.find(marker).map(|idx| {
            let text = line[idx..].trim().to_string();
            ((*marker).to_string(), text)
        })
    })
}

fn symbol_name(rest: &str) -> String {
    rest.trim_start()
        .trim_start_matches('<')
        .split(|c: char| {
            c.is_whitespace() || matches!(c, '(' | '<' | '{' | ':' | '=' | '[' | ';' | ',' | ')')
        })
        .find(|part| !part.is_empty())
        .unwrap_or("<anonymous>")
        .trim_matches('&')
        .to_string()
}
