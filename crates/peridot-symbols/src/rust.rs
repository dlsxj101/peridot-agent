//! Rust symbol extraction backed by `tree-sitter-rust`.

use crate::{
    LanguageSymbols, LocalBinding, Reference, Symbol, SymbolKind, collect_references_by_kind,
    field_name, local_binding, nearest_ancestor, parse, symbol_at, walk_nodes,
};

/// Node kinds that introduce a value scope a parameter governs.
const RUST_PARAM_SCOPES: &[&str] = &[
    "function_item",
    "function_signature_item",
    "closure_expression",
];

/// Rust symbol extraction backed by `tree-sitter-rust`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RustSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}

/// Maps a tree-sitter Rust node kind to a [`SymbolKind`].
fn node_kind(node_kind: &str) -> Option<SymbolKind> {
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
    let type_node = node
        .child_by_field_name("type")
        .or_else(|| node.child_by_field_name("name"))?;
    type_node.utf8_text(source.as_bytes()).ok()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier" | "type_identifier" | "field_identifier" | "shorthand_field_identifier"
    )
}

impl LanguageSymbols for RustSymbols {
    fn outline(&self, source: &str) -> Vec<Symbol> {
        let Some(tree) = parse(&language(), source) else {
            return Vec::new();
        };
        let mut symbols = Vec::new();
        collect(tree.root_node(), source, None, &mut symbols);
        symbols
    }

    fn references(&self, source: &str, name: &str) -> Vec<Reference> {
        let Some(tree) = parse(&language(), source) else {
            return Vec::new();
        };
        let mut refs = Vec::new();
        collect_references_by_kind(
            tree.root_node(),
            source,
            name,
            is_identifier_kind,
            &mut refs,
        );
        refs
    }

    fn local_bindings(&self, source: &str, name: &str) -> Vec<LocalBinding> {
        let Some(tree) = parse(&language(), source) else {
            return Vec::new();
        };
        let mut bindings = Vec::new();
        walk_nodes(tree.root_node(), &mut |node| match node.kind() {
            // `fn f(foo: T)` / typed closure params `|foo: T|`, including
            // destructuring like `(a, b): (T, U)`. The parameter governs its
            // function or closure.
            "parameter" => {
                if let Some(pattern) = node.child_by_field_name("pattern")
                    && let Some(scope) = nearest_ancestor(node, RUST_PARAM_SCOPES)
                {
                    push_pattern(pattern, name, source, &scope, &mut bindings);
                }
            }
            // Bare closure params `|foo|` are patterns directly under
            // `closure_parameters`.
            "closure_parameters" => {
                if let Some(scope) = nearest_ancestor(node, &["closure_expression"]) {
                    push_pattern(node, name, source, &scope, &mut bindings);
                }
            }
            // `let <pattern> = …`, incl. `let (a, b) = …`, governs the rest of
            // its block.
            "let_declaration" => {
                if let Some(pattern) = node.child_by_field_name("pattern")
                    && let Some(scope) = nearest_ancestor(node, &["block"])
                {
                    push_pattern(pattern, name, source, &scope, &mut bindings);
                }
            }
            // `if let Some(x) = …` / `while let … = …`: the pattern binds for
            // the enclosing `if`/`while` expression.
            "let_condition" => {
                if let Some(pattern) = node.child_by_field_name("pattern")
                    && let Some(scope) =
                        nearest_ancestor(node, &["if_expression", "while_expression"])
                {
                    push_pattern(pattern, name, source, &scope, &mut bindings);
                }
            }
            // `for <pattern> in …` binds for the loop body.
            "for_expression" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    push_pattern(pattern, name, source, &node, &mut bindings);
                }
            }
            // `match` arm patterns bind for that arm.
            "match_arm" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    push_pattern(pattern, name, source, &node, &mut bindings);
                }
            }
            _ => {}
        });
        bindings
    }
}

/// Binding-leaf node kinds inside a Rust pattern.
const RUST_PATTERN_BINDINGS: &[&str] = &["identifier", "shorthand_field_identifier"];

/// Collects every binding of `name` in `pattern` (handling tuple / struct /
/// tuple-struct destructuring, skipping the constructor/type path) and pushes a
/// [`LocalBinding`] governed by `scope` for each.
fn push_pattern(
    pattern: tree_sitter::Node,
    name: &str,
    source: &str,
    scope: &tree_sitter::Node,
    out: &mut Vec<LocalBinding>,
) {
    let mut tokens = Vec::new();
    crate::collect_pattern_idents(
        pattern,
        name,
        source,
        RUST_PATTERN_BINDINGS,
        &["type"],
        &mut tokens,
    );
    for token in tokens {
        out.push(local_binding(&token, scope));
    }
}

/// Depth-first walk that records definitions and threads the enclosing
/// `impl`/`trait` type down as the `container` for associated items.
fn collect(
    node: tree_sitter::Node,
    source: &str,
    container: Option<String>,
    out: &mut Vec<Symbol>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let Some(kind) = node_kind(child.kind()) else {
            // Descend through wrapper nodes (e.g. `declaration_list`) so we
            // still reach items nested one level down.
            collect(child, source, container.clone(), out);
            continue;
        };

        let (name, child_container) = if kind == SymbolKind::Impl {
            // The impl block itself is recorded under its type name, and its
            // members get that type as their container.
            let type_name = impl_or_trait_type_name(&child, source).map(str::to_string);
            (type_name.clone(), type_name)
        } else if kind == SymbolKind::Trait {
            let trait_name = field_name(&child, source).map(str::to_string);
            (trait_name.clone(), trait_name)
        } else {
            (
                field_name(&child, source).map(str::to_string),
                container.clone(),
            )
        };

        if let Some(name) = name {
            out.push(symbol_at(&child, kind, name, container.clone()));
        }

        // Recurse so nested items (impl methods, items inside `mod`) are
        // captured with the right container.
        collect(child, source, child_container, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{outline_rust, references_rust};

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
