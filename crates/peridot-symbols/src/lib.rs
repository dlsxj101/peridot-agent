//! Semantic symbol extraction for Peridot (Beyond-v1 feature F1 foundation).
//!
//! Today the agent finds code through `file_search` (glob) and `shell_exec`
//! (grep) plus the persisted `.peridot/codemap.json` text index. This crate
//! is the first semantic layer: it parses source with
//! [tree-sitter](https://tree-sitter.github.io/) and returns structured
//! symbols (functions, structs, enums, traits, impls, modules, ...) with line
//! ranges, so callers can build `symbol_outline` / `symbol_definition` tools
//! that attach exact definitions instead of whole grep dumps.
//!
//! Language support is behind the [`LanguageSymbols`] trait so future
//! languages (TypeScript, Go, Python) plug in without changing callers, the
//! same trait-boundary pattern the rest of the workspace uses. Rust is the
//! first implementation ([`RustSymbols`]).

use serde::{Deserialize, Serialize};

/// The kind of a source symbol. Additive: new variants may appear as more
/// node types are recognized, so match with a wildcard arm when exhaustive
/// handling is not required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Const,
    Static,
    TypeAlias,
    Macro,
}

impl SymbolKind {
    /// A short, stable lower-case label, useful for outline rendering.
    pub fn label(self) -> &'static str {
        match self {
            SymbolKind::Function => "fn",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Impl => "impl",
            SymbolKind::Module => "mod",
            SymbolKind::Const => "const",
            SymbolKind::Static => "static",
            SymbolKind::TypeAlias => "type",
            SymbolKind::Macro => "macro",
        }
    }
}

/// A single named definition found in a source file.
///
/// Line numbers are 1-based and inclusive, matching the convention used by
/// the existing code-map and editor surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    /// The symbol's own name (e.g. `run_session_task`).
    pub name: String,
    /// What kind of definition this is.
    pub kind: SymbolKind,
    /// 1-based first line of the definition.
    pub start_line: usize,
    /// 1-based last line of the definition (inclusive).
    pub end_line: usize,
    /// For associated items, the owning `impl`/`trait` type
    /// (e.g. `DaemonState` for a method inside `impl DaemonState`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
}

impl Symbol {
    /// `"<kind> <container::>name"` — a compact one-line outline label.
    pub fn outline_label(&self) -> String {
        match &self.container {
            Some(owner) => format!("{} {}::{}", self.kind.label(), owner, self.name),
            None => format!("{} {}", self.kind.label(), self.name),
        }
    }
}

/// A single token-level occurrence of an identifier in source.
///
/// Produced by [`LanguageSymbols::references`]: an AST-aware scan that only
/// matches real identifier tokens, so occurrences inside comments and string
/// literals are excluded — the key improvement over a textual grep. Includes
/// the definition site itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// 1-based line of the occurrence.
    pub line: usize,
    /// 1-based column (character offset within the line) of the occurrence.
    pub column: usize,
}

/// A per-language symbol extractor. Implementations are stateless and cheap to
/// construct; parser setup happens per call so they stay `Send + Sync`.
pub trait LanguageSymbols {
    /// Parse `source` and return its top-level and associated symbols in
    /// source order. Returns an empty vector for input that fails to parse;
    /// tree-sitter is error-tolerant, so partial files still yield the
    /// symbols it could recover.
    fn outline(&self, source: &str) -> Vec<Symbol>;

    /// All symbols named `name`, in source order. Default implementation
    /// filters [`LanguageSymbols::outline`].
    fn definitions(&self, source: &str, name: &str) -> Vec<Symbol> {
        self.outline(source)
            .into_iter()
            .filter(|s| s.name == name)
            .collect()
    }

    /// All identifier-token occurrences of `name`, in source order. AST-aware:
    /// occurrences inside comments and string literals are skipped. The
    /// definition site is included.
    fn references(&self, source: &str, name: &str) -> Vec<Reference>;
}

/// Rust symbol extraction backed by `tree-sitter-rust`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RustSymbols;

/// Maps a tree-sitter Rust node kind to a [`SymbolKind`].
fn rust_node_kind(node_kind: &str) -> Option<SymbolKind> {
    Some(match node_kind {
        "function_item" | "function_signature_item" => SymbolKind::Function,
        "struct_item" => SymbolKind::Struct,
        "enum_item" => SymbolKind::Enum,
        "union_item" => SymbolKind::Struct,
        "trait_item" => SymbolKind::Trait,
        "impl_item" => SymbolKind::Impl,
        "mod_item" => SymbolKind::Module,
        "const_item" => SymbolKind::Const,
        "static_item" => SymbolKind::Static,
        "type_item" => SymbolKind::TypeAlias,
        "macro_definition" => SymbolKind::Macro,
        _ => return None,
    })
}

/// Returns the source text of an `impl`/`trait` item's type name, used as the
/// `container` for nested associated items and as the name of the `impl`
/// itself. For `impl Trait for Type` this returns `Type`.
fn impl_or_trait_type_name<'a>(node: &tree_sitter::Node, source: &'a str) -> Option<&'a str> {
    // `impl_item` exposes a `type` field for the implementing type; `trait_item`
    // exposes `name`. Prefer the implementing type so methods read as
    // `Type::method`.
    let type_node = node
        .child_by_field_name("type")
        .or_else(|| node.child_by_field_name("name"))?;
    type_node.utf8_text(source.as_bytes()).ok()
}

impl LanguageSymbols for RustSymbols {
    fn outline(&self, source: &str) -> Vec<Symbol> {
        let mut parser = tree_sitter::Parser::new();
        let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        if parser.set_language(&language).is_err() {
            return Vec::new();
        }
        let Some(tree) = parser.parse(source, None) else {
            return Vec::new();
        };

        let mut symbols = Vec::new();
        collect_rust(tree.root_node(), source, None, &mut symbols);
        symbols
    }

    fn references(&self, source: &str, name: &str) -> Vec<Reference> {
        let mut parser = tree_sitter::Parser::new();
        let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        if parser.set_language(&language).is_err() {
            return Vec::new();
        }
        let Some(tree) = parser.parse(source, None) else {
            return Vec::new();
        };

        let mut refs = Vec::new();
        collect_rust_references(tree.root_node(), source, name, &mut refs);
        refs
    }
}

/// Leaf node kinds that count as identifier usages for reference search.
fn is_rust_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier" | "type_identifier" | "field_identifier" | "shorthand_field_identifier"
    )
}

/// Depth-first walk recording every identifier-token whose text equals `name`.
fn collect_rust_references(
    node: tree_sitter::Node,
    source: &str,
    name: &str,
    out: &mut Vec<Reference>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.child_count() == 0 {
            if is_rust_identifier_kind(child.kind())
                && child.utf8_text(source.as_bytes()) == Ok(name)
            {
                let pos = child.start_position();
                out.push(Reference {
                    line: pos.row + 1,
                    column: pos.column + 1,
                });
            }
        } else {
            collect_rust_references(child, source, name, out);
        }
    }
}

/// Depth-first walk that records definitions and threads the enclosing
/// `impl`/`trait` type down as the `container` for associated items.
fn collect_rust(
    node: tree_sitter::Node,
    source: &str,
    container: Option<String>,
    out: &mut Vec<Symbol>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let node_kind = child.kind();
        let Some(kind) = rust_node_kind(node_kind) else {
            // Descend through wrapper nodes (e.g. `declaration_list`) so we
            // still reach items nested one level down.
            collect_rust(child, source, container.clone(), out);
            continue;
        };

        let (name, child_container) = if kind == SymbolKind::Impl {
            // The impl block itself is recorded under its type name, and its
            // members get that type as their container.
            let type_name = impl_or_trait_type_name(&child, source).map(str::to_string);
            (type_name.clone(), type_name)
        } else if kind == SymbolKind::Trait {
            let trait_name = child
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(str::to_string);
            (trait_name.clone(), trait_name)
        } else {
            let name = child
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(str::to_string);
            (name, container.clone())
        };

        if let Some(name) = name {
            out.push(Symbol {
                name,
                kind,
                start_line: child.start_position().row + 1,
                end_line: child.end_position().row + 1,
                container: container.clone(),
            });
        }

        // Recurse so nested items (impl methods, items inside `mod`) are
        // captured with the right container.
        collect_rust(child, source, child_container, out);
    }
}

/// Convenience wrapper: outline a Rust source string.
pub fn outline_rust(source: &str) -> Vec<Symbol> {
    RustSymbols.outline(source)
}

/// Convenience wrapper: find identifier-token references to `name` in Rust source.
pub fn references_rust(source: &str, name: &str) -> Vec<Reference> {
    RustSymbols.references(source, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
pub struct DaemonState {
    out: u32,
}

impl DaemonState {
    pub fn new() -> Self {
        Self { out: 0 }
    }
    fn helper(&self) -> u32 {
        self.out
    }
}

pub enum Mode {
    Plan,
    Execute,
}

pub trait Runner {
    fn run(&self);
}

const MAX: usize = 8;

pub fn free_function(x: u32) -> u32 {
    x + 1
}

mod inner {
    pub fn nested() {}
}
"#;

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_top_level_items() {
        let symbols = outline_rust(SAMPLE);

        let s = find(&symbols, "DaemonState").expect("struct");
        assert_eq!(s.kind, SymbolKind::Struct);
        assert!(s.container.is_none());

        assert_eq!(find(&symbols, "Mode").unwrap().kind, SymbolKind::Enum);
        assert_eq!(find(&symbols, "Runner").unwrap().kind, SymbolKind::Trait);
        assert_eq!(find(&symbols, "MAX").unwrap().kind, SymbolKind::Const);
        assert_eq!(
            find(&symbols, "free_function").unwrap().kind,
            SymbolKind::Function
        );
        assert_eq!(find(&symbols, "inner").unwrap().kind, SymbolKind::Module);
    }

    #[test]
    fn associates_impl_methods_with_their_type() {
        let symbols = outline_rust(SAMPLE);

        let new_fn = symbols
            .iter()
            .find(|s| s.name == "new" && s.kind == SymbolKind::Function)
            .expect("new method");
        assert_eq!(new_fn.container.as_deref(), Some("DaemonState"));

        let helper = symbols
            .iter()
            .find(|s| s.name == "helper")
            .expect("helper method");
        assert_eq!(helper.container.as_deref(), Some("DaemonState"));
        assert_eq!(helper.outline_label(), "fn DaemonState::helper");
    }

    #[test]
    fn captures_nested_module_function() {
        let symbols = outline_rust(SAMPLE);
        let nested = find(&symbols, "nested").expect("nested fn");
        assert_eq!(nested.kind, SymbolKind::Function);
    }

    #[test]
    fn line_numbers_are_one_based_and_inclusive() {
        let symbols = outline_rust("fn a() {}\nfn b() {\n}\n");
        let a = find(&symbols, "a").unwrap();
        assert_eq!(a.start_line, 1);
        assert_eq!(a.end_line, 1);
        let b = find(&symbols, "b").unwrap();
        assert_eq!(b.start_line, 2);
        assert_eq!(b.end_line, 3);
    }

    #[test]
    fn definitions_filters_by_name() {
        let defs = RustSymbols.definitions(SAMPLE, "new");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "new");
    }

    #[test]
    fn malformed_source_is_error_tolerant() {
        // A valid function followed by a truncated one: tree-sitter recovers
        // the well-formed item instead of dropping the whole file.
        let symbols = outline_rust("fn good() {}\nfn broken(  {");
        assert!(
            symbols.iter().any(|s| s.name == "good"),
            "valid item should survive a later parse error: {symbols:?}"
        );
    }

    #[test]
    fn references_find_all_identifier_occurrences() {
        let source = "\
fn target() {}
fn caller() {
    target();
    let x = target;
}
";
        let refs = references_rust(source, "target");
        // definition + two usages
        assert_eq!(refs.len(), 3, "{refs:?}");
        assert_eq!(refs[0].line, 1); // definition
        assert_eq!(refs[1].line, 3); // call
        assert_eq!(refs[2].line, 4); // value position
    }

    #[test]
    fn references_skip_comments_and_strings() {
        let source = "\
fn needle() {}
fn other() {
    // needle in a comment
    let s = \"needle in a string\";
    needle();
}
";
        let refs = references_rust(source, "needle");
        // Only the definition (line 1) and the real call (line 5); the
        // comment and string occurrences are not identifier tokens.
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 5);
        assert!(refs[1].column >= 1);
    }

    #[test]
    fn references_empty_for_unknown_name() {
        assert!(references_rust("fn a() {}", "zzz").is_empty());
    }

    #[test]
    fn symbol_serializes_without_null_container() {
        let symbols = outline_rust("fn solo() {}");
        let json = serde_json::to_string(&symbols[0]).unwrap();
        assert!(
            !json.contains("container"),
            "container should be omitted: {json}"
        );
    }
}
