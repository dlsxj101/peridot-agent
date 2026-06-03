//! Semantic symbol extraction for Peridot (Beyond-v1 feature F1).
//!
//! The agent finds code through `file_search` (glob) and `shell_exec`
//! (grep) plus the persisted `.peridot/codemap.json` text index. This crate
//! is the semantic layer: it parses source with
//! [tree-sitter](https://tree-sitter.github.io/) and returns structured
//! symbols (functions, structs, classes, enums, traits, methods, ...) with
//! line ranges, so the `file_outline` / `symbol_definition` /
//! `symbol_references` tools attach exact definitions instead of whole grep
//! dumps.
//!
//! Language support is behind the [`LanguageSymbols`] trait so languages plug
//! in without changing callers, the same trait-boundary pattern the rest of
//! the workspace uses. Implemented: Rust, TypeScript/JavaScript/JSX, Python,
//! Go, Java, Ruby, C, C++, C#, PHP, Bash, Scala, Lua, Kotlin, Swift, Haskell,
//! Elixir, Zig, OCaml, Dart, Elm, and Julia. Callers usually
//! dispatch by file extension via [`outline_for_extension`] and
//! [`references_for_extension`], falling back to their own heuristic when the
//! extension has no grammar.

use serde::{Deserialize, Serialize};

mod bash;
mod c_family;
mod csharp;
mod dart;
mod elixir;
mod elm;
mod go;
mod haskell;
mod java;
mod julia;
mod kotlin;
mod lua;
mod ocaml;
mod php;
mod python;
mod ruby;
mod rust;
mod scala;
mod swift;
mod typescript;
mod zig;

pub use bash::BashSymbols;
pub use c_family::CFamilySymbols;
pub use csharp::CSharpSymbols;
pub use dart::DartSymbols;
pub use elixir::ElixirSymbols;
pub use elm::ElmSymbols;
pub use go::GoSymbols;
pub use haskell::HaskellSymbols;
pub use java::JavaSymbols;
pub use julia::JuliaSymbols;
pub use kotlin::KotlinSymbols;
pub use lua::LuaSymbols;
pub use ocaml::OCamlSymbols;
pub use php::PhpSymbols;
pub use python::PythonSymbols;
pub use ruby::RubySymbols;
pub use rust::RustSymbols;
pub use scala::ScalaSymbols;
pub use swift::SwiftSymbols;
pub use typescript::TypeScriptSymbols;
pub use zig::ZigSymbols;

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
    Class,
    Interface,
    Method,
    Variable,
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
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Method => "method",
            SymbolKind::Variable => "var",
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
    /// For associated items, the owning type/class
    /// (e.g. `DaemonState` for a method inside `impl DaemonState`, or the
    /// class name for a TypeScript/Python method).
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
    /// Whether this occurrence is the symbol's definition site (vs. a usage).
    /// Set by the dispatch/convenience entry points, which cross-reference the
    /// occurrence against the file's outline; the raw per-language
    /// [`LanguageSymbols::references`] walk leaves it `false`.
    #[serde(default)]
    pub is_definition: bool,
    /// The fully-qualified lexical scope chain (`outer::…::inner`) of the
    /// outline symbols that enclose this occurrence — every nested module,
    /// namespace, type, and function body the reference sits in, from outermost
    /// to innermost (e.g. `ui::Widget::render`). `None` for occurrences at file
    /// scope. A definition occurrence reports its *parent* scope rather than
    /// itself, so a definition is never listed as enclosing itself. Like
    /// [`Reference::is_definition`], it is filled by the dispatch/convenience
    /// entry points, not the raw per-language walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
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

/// Returns the symbol extractor for a file extension (lower-case, no dot), or
/// `None` when no tree-sitter grammar is wired in for it. Callers fall back to
/// their textual heuristic in the `None` case.
pub fn language_for_extension(extension: &str) -> Option<Box<dyn LanguageSymbols>> {
    match extension {
        "rs" => Some(Box::new(RustSymbols)),
        "ts" | "mts" | "cts" => Some(Box::new(TypeScriptSymbols::typescript())),
        // The TSX grammar is a superset that also parses plain JS and JSX.
        "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(Box::new(TypeScriptSymbols::tsx())),
        "py" | "pyi" => Some(Box::new(PythonSymbols)),
        "go" => Some(Box::new(GoSymbols)),
        "java" => Some(Box::new(JavaSymbols)),
        "rb" => Some(Box::new(RubySymbols)),
        "c" | "h" => Some(Box::new(CFamilySymbols::c())),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Some(Box::new(CFamilySymbols::cpp())),
        "cs" => Some(Box::new(CSharpSymbols)),
        "php" => Some(Box::new(PhpSymbols)),
        "sh" | "bash" => Some(Box::new(BashSymbols)),
        "scala" | "sc" => Some(Box::new(ScalaSymbols)),
        "lua" => Some(Box::new(LuaSymbols)),
        "kt" | "kts" => Some(Box::new(KotlinSymbols)),
        "swift" => Some(Box::new(SwiftSymbols)),
        "hs" => Some(Box::new(HaskellSymbols)),
        "ex" | "exs" => Some(Box::new(ElixirSymbols)),
        "zig" => Some(Box::new(ZigSymbols)),
        "ml" | "mli" => Some(Box::new(OCamlSymbols)),
        "dart" => Some(Box::new(DartSymbols)),
        "elm" => Some(Box::new(ElmSymbols)),
        "jl" => Some(Box::new(JuliaSymbols)),
        _ => None,
    }
}

/// Whether a tree-sitter grammar is wired in for a file extension (lower-case,
/// no dot). Callers gating directory walks on "is this a source file?" should
/// use this so the walk set never drifts from [`language_for_extension`].
pub fn supports_extension(extension: &str) -> bool {
    language_for_extension(extension).is_some()
}

/// Outlines `source` using the grammar for `extension`, or `None` when the
/// extension has no grammar.
pub fn outline_for_extension(extension: &str, source: &str) -> Option<Vec<Symbol>> {
    language_for_extension(extension).map(|lang| lang.outline(source))
}

/// Finds references to `name` in `source` using the grammar for `extension`,
/// or `None` when the extension has no grammar.
pub fn references_for_extension(
    extension: &str,
    source: &str,
    name: &str,
) -> Option<Vec<Reference>> {
    language_for_extension(extension).map(|lang| marked_references(lang.as_ref(), source, name))
}

/// Runs a language's reference walk, then annotates each occurrence against the
/// file outline: flags the ones sitting on a definition line for `name` as
/// definitions, and records the innermost enclosing symbol as the occurrence's
/// [`Reference::scope`]. This is what distinguishes the def site from usages and
/// locates each usage's lexical container without per-language tracking.
fn marked_references(lang: &dyn LanguageSymbols, source: &str, name: &str) -> Vec<Reference> {
    let mut references = lang.references(source, name);
    let outline = lang.outline(source);
    let definition_lines: std::collections::HashSet<usize> = outline
        .iter()
        .filter(|symbol| symbol.name == name)
        .map(|symbol| symbol.start_line)
        .collect();
    for reference in &mut references {
        reference.is_definition = definition_lines.contains(&reference.line);
        reference.scope = enclosing_scope(&outline, reference.line, reference.is_definition);
    }
    references
}

/// The fully-qualified lexical scope chain (`outer::…::inner`) of the
/// outline symbols that enclose `line`, from outermost to innermost. A
/// definition occurrence (`is_def`) excludes the symbol that starts on its own
/// line, so a definition reports its *parent* scope rather than itself.
/// Returns `None` for occurrences that sit at file scope.
///
/// The path is built from the line ranges of the enclosing symbols, so nested
/// modules / namespaces / types all contribute a component (`ui::Widget::render`
/// rather than just `Widget::render`). Each symbol's [`Symbol::container`] is
/// folded in as well, so a method whose enclosing type is not itself a separate
/// outline node (it lives only in the `container` field) still names its owner.
/// Adjacent duplicate components are collapsed so a type that appears both as an
/// enclosing node and as a member's `container` is not repeated.
fn enclosing_scope(outline: &[Symbol], line: usize, is_def: bool) -> Option<String> {
    let mut enclosers: Vec<&Symbol> = outline
        .iter()
        .filter(|symbol| symbol.start_line <= line && line <= symbol.end_line)
        .filter(|symbol| !(is_def && symbol.start_line == line))
        .collect();
    if enclosers.is_empty() {
        return None;
    }
    // Outermost first: earliest start, then widest range on ties.
    enclosers.sort_by_key(|symbol| (symbol.start_line, std::cmp::Reverse(symbol.end_line)));

    // Collapse adjacent duplicates so a type that shows up both as an enclosing
    // node and as a member's `container` is named once.
    let mut path: Vec<&str> = Vec::new();
    for symbol in &enclosers {
        if let Some(owner) = symbol.container.as_deref()
            && path.last() != Some(&owner)
        {
            path.push(owner);
        }
        if path.last() != Some(&symbol.name.as_str()) {
            path.push(symbol.name.as_str());
        }
    }
    Some(path.join("::"))
}

/// Convenience wrapper: outline a Rust source string.
pub fn outline_rust(source: &str) -> Vec<Symbol> {
    RustSymbols.outline(source)
}

/// Convenience wrapper: find identifier-token references to `name` in Rust
/// source, with definition sites flagged.
pub fn references_rust(source: &str, name: &str) -> Vec<Reference> {
    marked_references(&RustSymbols, source, name)
}

// ---- shared tree-sitter helpers used by the language modules ----

/// Parses `source` with `language`, returning `None` on setup or parse
/// failure. tree-sitter is error-tolerant, so partial files still parse.
pub(crate) fn parse(language: &tree_sitter::Language, source: &str) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(language).ok()?;
    parser.parse(source, None)
}

/// Builds a [`Symbol`] from a definition `node` (1-based inclusive line range).
pub(crate) fn symbol_at(
    node: &tree_sitter::Node,
    kind: SymbolKind,
    name: String,
    container: Option<String>,
) -> Symbol {
    Symbol {
        name,
        kind,
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        container,
    }
}

/// Returns the text of `node`'s `name` field, if present.
pub(crate) fn field_name<'a>(node: &tree_sitter::Node, source: &'a str) -> Option<&'a str> {
    node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
}

/// Returns the text of the first direct child of `node` whose node kind is
/// `kind`. Used to read a declaration's name when it is a named child rather
/// than a `name` field (e.g. a Zig `identifier`, an Elm `upper_case_identifier`).
pub(crate) fn first_child_text<'a>(
    node: &tree_sitter::Node,
    kind: &str,
    source: &'a str,
) -> Option<&'a str> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
        .and_then(|child| child.utf8_text(source.as_bytes()).ok())
}

/// Returns the text of the first descendant (depth-first, self included) whose
/// node kind is `kind`. Used to dig a name out of a wrapper node, e.g. the
/// receiver type of a Go method.
pub(crate) fn first_descendant_text<'a>(
    node: tree_sitter::Node,
    kind: &str,
    source: &'a str,
) -> Option<&'a str> {
    if node.kind() == kind {
        return node.utf8_text(source.as_bytes()).ok();
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(text) = first_descendant_text(child, kind, source) {
            return Some(text);
        }
    }
    None
}

/// Depth-first walk recording every leaf identifier token whose text equals
/// `name`, using `is_identifier` to decide which leaf kinds count. Shared by
/// every language's [`LanguageSymbols::references`].
pub(crate) fn collect_references_by_kind(
    node: tree_sitter::Node,
    source: &str,
    name: &str,
    is_identifier: fn(&str) -> bool,
    out: &mut Vec<Reference>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.child_count() == 0 {
            if is_identifier(child.kind()) && child.utf8_text(source.as_bytes()) == Ok(name) {
                let pos = child.start_position();
                out.push(Reference {
                    line: pos.row + 1,
                    column: pos.column + 1,
                    is_definition: false,
                    scope: None,
                });
            }
        } else {
            collect_references_by_kind(child, source, name, is_identifier, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_routes_by_extension() {
        assert!(
            outline_for_extension("rs", "fn a() {}")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("ts", "function a() {}")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("py", "def a():\n    pass\n")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("go", "package m\nfunc a() {}")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("java", "class A { void a() {} }")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("rb", "def a\nend\n")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("c", "int a(void) { return 0; }")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("cpp", "class A { void a() {} };")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("cs", "class A { void a() {} }")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("php", "<?php function a() {}")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("sh", "a() { echo hi; }")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("scala", "object A { def a(): Int = 0 }")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("lua", "function a() end")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("kt", "fun a() {}")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("swift", "func a() {}")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("hs", "a :: Int\na = 1\n")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("ex", "defmodule M do\n  def a, do: 1\nend\n")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("zig", "fn a() void {}")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("ml", "let a x = x")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("dart", "int a() => 0;")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("elm", "a : Int\na = 1\n")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(
            outline_for_extension("jl", "a(x) = x")
                .unwrap()
                .iter()
                .any(|s| s.name == "a")
        );
        assert!(outline_for_extension("txt", "anything").is_none());
    }

    #[test]
    fn references_dispatch_routes_by_extension() {
        let refs = references_for_extension("py", "def a():\n    pass\na()\n", "a").unwrap();
        assert_eq!(refs.len(), 2);
        assert!(references_for_extension("md", "a a a", "a").is_none());
    }

    #[test]
    fn references_flag_the_definition_site() {
        // def on line 1, call on line 3.
        let refs = references_for_extension("py", "def a():\n    pass\na()\n", "a").unwrap();
        let def = refs
            .iter()
            .find(|r| r.is_definition)
            .expect("definition flagged");
        assert_eq!(def.line, 1);
        // Exactly one definition; the call is a usage.
        assert_eq!(refs.iter().filter(|r| r.is_definition).count(), 1);
        assert!(refs.iter().any(|r| !r.is_definition && r.line == 3));
    }

    #[test]
    fn references_record_their_enclosing_scope() {
        // `helper` is called once at file scope and once inside `caller`.
        let source = "\
fn helper() {}
fn caller() {
    helper();
}
helper();
";
        let refs = references_rust(source, "helper");
        // The definition reports its parent scope (file scope) as None.
        let def = refs.iter().find(|r| r.is_definition).expect("definition");
        assert_eq!(def.scope, None);
        // The call inside `caller` is scoped to it.
        let inside = refs
            .iter()
            .find(|r| !r.is_definition && r.line == 3)
            .expect("call inside caller");
        assert_eq!(inside.scope.as_deref(), Some("caller"));
        // The trailing top-level call has no enclosing definition.
        let top = refs
            .iter()
            .find(|r| !r.is_definition && r.line == 5)
            .expect("top-level call");
        assert_eq!(top.scope, None);
    }

    #[test]
    fn enclosing_scope_qualifies_methods_with_their_container() {
        // A usage inside an impl method is scoped as `Container::method`.
        let source = "\
struct Widget;
impl Widget {
    fn render(&self) {
        helper();
    }
}
";
        let refs = references_rust(source, "helper");
        let inside = refs.iter().find(|r| r.line == 4).expect("call in render");
        assert_eq!(inside.scope.as_deref(), Some("Widget::render"));
    }

    #[test]
    fn enclosing_scope_reports_the_full_nesting_chain() {
        // A call nested module > impl > method reports every enclosing level,
        // outermost first, with the type named once despite appearing both as
        // an enclosing node and as the method's container.
        let source = "\
mod ui {
    struct Widget;
    impl Widget {
        fn render(&self) {
            helper();
        }
    }
    fn helper() {}
}
";
        let refs = references_rust(source, "helper");
        let call = refs
            .iter()
            .find(|r| !r.is_definition && r.line == 5)
            .expect("call in render");
        assert_eq!(call.scope.as_deref(), Some("ui::Widget::render"));
        // A module-level definition reports the module as its parent scope,
        // picked up purely from range nesting (its `container` is None).
        let def = refs.iter().find(|r| r.is_definition).expect("definition");
        assert_eq!(def.scope.as_deref(), Some("ui"));
    }

    #[test]
    fn enclosing_scope_chains_nested_python_classes() {
        // Language-agnostic: nested classes contribute their own components,
        // sourced from each symbol's `container` field.
        let source = "\
class Outer:
    class Inner:
        def method(self):
            helper()
";
        let refs = references_for_extension("py", source, "helper").unwrap();
        let call = refs.iter().find(|r| r.line == 4).expect("call in method");
        assert_eq!(call.scope.as_deref(), Some("Outer::Inner::method"));
    }
}
