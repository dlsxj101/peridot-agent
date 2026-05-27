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

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct CodeMapIndex {
    pub version: u32,
    pub generated_at_unix: u64,
    pub report: CodeMapReport,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct CodeMapReport {
    pub walked_files: usize,
    pub symbols: Vec<CodeMapSymbol>,
    pub todos: Vec<CodeMapTodo>,
    pub symbols_truncated: bool,
    pub todos_truncated: bool,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct CodeMapSymbol {
    pub path: String,
    pub line: usize,
    pub kind: String,
    pub name: String,
    pub signature: String,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
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

pub(crate) fn refresh_code_map_index(
    project_root: &Path,
    max_symbols: usize,
    max_todos: usize,
) -> Result<CodeMapIndex> {
    let index = CodeMapIndex {
        version: 1,
        generated_at_unix: unix_seconds(),
        report: build_code_map(project_root, max_symbols, max_todos),
    };
    write_code_map_index(project_root, &index)?;
    Ok(index)
}

pub(crate) fn load_code_map_index(project_root: &Path) -> Result<Option<CodeMapIndex>> {
    let path = code_map_index_path(project_root);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let index = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(index))
}

pub(crate) fn load_or_refresh_code_map_index(
    project_root: &Path,
    max_symbols: usize,
    max_todos: usize,
) -> Result<CodeMapIndex> {
    if let Some(index) = load_code_map_index(project_root)? {
        return Ok(index);
    }
    refresh_code_map_index(project_root, max_symbols, max_todos)
}

pub(crate) fn search_code_map_index(index: &CodeMapIndex, query: &str) -> CodeMapReport {
    let tokens = search_tokens(query);
    if tokens.is_empty() {
        return index.report.clone();
    }
    CodeMapReport {
        walked_files: index.report.walked_files,
        symbols: index
            .report
            .symbols
            .iter()
            .filter(|symbol| symbol_matches(symbol, &tokens))
            .cloned()
            .collect(),
        todos: index
            .report
            .todos
            .iter()
            .filter(|todo| todo_matches(todo, &tokens))
            .cloned()
            .collect(),
        symbols_truncated: index.report.symbols_truncated,
        todos_truncated: index.report.todos_truncated,
    }
}

pub(crate) fn locate_code_map_symbols(index: &CodeMapIndex, query: &str) -> CodeMapReport {
    let tokens = search_tokens(query);
    if tokens.is_empty() {
        return CodeMapReport {
            walked_files: index.report.walked_files,
            symbols: Vec::new(),
            todos: Vec::new(),
            symbols_truncated: index.report.symbols_truncated,
            todos_truncated: false,
        };
    }
    let mut symbols = index
        .report
        .symbols
        .iter()
        .filter(|symbol| symbol_matches(symbol, &tokens))
        .cloned()
        .collect::<Vec<_>>();
    symbols.sort_by_key(|symbol| {
        (
            symbol_locate_rank(symbol, query),
            symbol.path.clone(),
            symbol.line,
        )
    });
    CodeMapReport {
        walked_files: index.report.walked_files,
        symbols,
        todos: Vec::new(),
        symbols_truncated: index.report.symbols_truncated,
        todos_truncated: false,
    }
}

pub(crate) fn code_map_index_path(project_root: &Path) -> PathBuf {
    project_root.join(".peridot").join("codemap.json")
}

fn write_code_map_index(project_root: &Path, index: &CodeMapIndex) -> Result<()> {
    let path = code_map_index_path(project_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(index).context("failed to serialize code map index")?;
    fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))
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

fn search_tokens(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn symbol_matches(symbol: &CodeMapSymbol, tokens: &[String]) -> bool {
    let haystack = format!(
        "{} {} {} {} {}",
        symbol.path, symbol.line, symbol.kind, symbol.name, symbol.signature
    )
    .to_ascii_lowercase();
    tokens.iter().all(|token| haystack.contains(token))
}

fn todo_matches(todo: &CodeMapTodo, tokens: &[String]) -> bool {
    let haystack =
        format!("{} {} {} {}", todo.path, todo.line, todo.marker, todo.text).to_ascii_lowercase();
    tokens.iter().all(|token| haystack.contains(token))
}

fn symbol_locate_rank(symbol: &CodeMapSymbol, query: &str) -> u8 {
    let name = symbol.name.to_ascii_lowercase();
    let query = query.trim().to_ascii_lowercase();
    if name == query {
        return 0;
    }
    if name.starts_with(&query) {
        return 1;
    }
    if name.contains(&query) {
        return 2;
    }
    3
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_writes_and_loads_code_map_index() {
        let root = temp_project("index");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub fn indexed() {}\n// TODO: wire search\n",
        )
        .unwrap();

        let index = refresh_code_map_index(&root, 100, 100).unwrap();
        assert_eq!(index.version, 1);
        assert!(code_map_index_path(&root).is_file());
        assert!(
            index
                .report
                .symbols
                .iter()
                .any(|symbol| symbol.name == "indexed")
        );
        assert!(
            index
                .report
                .todos
                .iter()
                .any(|todo| todo.text.contains("TODO"))
        );

        let loaded = load_code_map_index(&root).unwrap().unwrap();
        assert_eq!(loaded.report.symbols[0].name, "indexed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_filters_index_symbols_and_todos() {
        let index = CodeMapIndex {
            version: 1,
            generated_at_unix: 42,
            report: CodeMapReport {
                walked_files: 2,
                symbols: vec![
                    CodeMapSymbol {
                        path: "src/lib.rs".to_string(),
                        line: 10,
                        kind: "struct".to_string(),
                        name: "Runner".to_string(),
                        signature: "pub struct Runner;".to_string(),
                    },
                    CodeMapSymbol {
                        path: "src/main.rs".to_string(),
                        line: 20,
                        kind: "fn".to_string(),
                        name: "serve".to_string(),
                        signature: "pub fn serve() {}".to_string(),
                    },
                ],
                todos: vec![CodeMapTodo {
                    path: "src/lib.rs".to_string(),
                    line: 30,
                    marker: "TODO".to_string(),
                    text: "TODO: wire runner search".to_string(),
                }],
                symbols_truncated: false,
                todos_truncated: false,
            },
        };

        let report = search_code_map_index(&index, "runner src/lib");
        assert_eq!(report.walked_files, 2);
        assert_eq!(report.symbols.len(), 1);
        assert_eq!(report.symbols[0].name, "Runner");
        assert_eq!(report.todos.len(), 1);

        let report = search_code_map_index(&index, "serve");
        assert_eq!(report.symbols.len(), 1);
        assert_eq!(report.symbols[0].name, "serve");
        assert!(report.todos.is_empty());
    }

    #[test]
    fn locate_returns_ranked_symbol_only_matches() {
        let index = CodeMapIndex {
            version: 1,
            generated_at_unix: 42,
            report: CodeMapReport {
                walked_files: 2,
                symbols: vec![
                    CodeMapSymbol {
                        path: "src/lib.rs".to_string(),
                        line: 30,
                        kind: "struct".to_string(),
                        name: "RunnerConfig".to_string(),
                        signature: "pub struct RunnerConfig;".to_string(),
                    },
                    CodeMapSymbol {
                        path: "src/main.rs".to_string(),
                        line: 10,
                        kind: "struct".to_string(),
                        name: "Runner".to_string(),
                        signature: "pub struct Runner;".to_string(),
                    },
                ],
                todos: vec![CodeMapTodo {
                    path: "src/lib.rs".to_string(),
                    line: 44,
                    marker: "TODO".to_string(),
                    text: "TODO: Runner".to_string(),
                }],
                symbols_truncated: false,
                todos_truncated: false,
            },
        };

        let report = locate_code_map_symbols(&index, "runner");
        assert_eq!(report.symbols.len(), 2);
        assert_eq!(report.symbols[0].name, "Runner");
        assert!(report.todos.is_empty());
    }

    fn temp_project(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "peridot-codemap-{name}-{}-{}",
            std::process::id(),
            unix_seconds()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }
}
