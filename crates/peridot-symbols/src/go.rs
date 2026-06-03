//! Go symbol extraction backed by `tree-sitter-go`.

use crate::{
    LanguageSymbols, LocalBinding, Reference, Symbol, SymbolKind, collect_references_by_kind,
    first_descendant_text, local_binding, nearest_ancestor, parse, symbol_at, walk_nodes,
};

/// Go symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct GoSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_go::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier" | "type_identifier" | "field_identifier")
}

impl LanguageSymbols for GoSymbols {
    fn outline(&self, source: &str) -> Vec<Symbol> {
        let Some(tree) = parse(&language(), source) else {
            return Vec::new();
        };
        let mut symbols = Vec::new();
        collect(tree.root_node(), source, &mut symbols);
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
            // `func f(foo int)` — the `identifier` children name the params;
            // the trailing `type_identifier` is the type. Params govern the
            // enclosing function / method / closure.
            "parameter_declaration" | "variadic_parameter_declaration" => {
                if let Some(scope) = nearest_ancestor(
                    node,
                    &["function_declaration", "method_declaration", "func_literal"],
                ) {
                    push_named_identifiers(&node, name, source, &scope, &mut bindings);
                }
            }
            // `bar := …` and `var bar = …` govern the rest of their block;
            // package-level `var` (no enclosing block) is a module symbol.
            "short_var_declaration" => {
                if let Some(left) = node.child_by_field_name("left")
                    && let Some(scope) = nearest_ancestor(node, &["block"])
                {
                    push_named_identifiers(&left, name, source, &scope, &mut bindings);
                }
            }
            "var_spec" => {
                if let Some(scope) = nearest_ancestor(node, &["block"]) {
                    push_named_identifiers(&node, name, source, &scope, &mut bindings);
                }
            }
            // `for k, v := range …` binds the loop variables for the loop body.
            "range_clause" => {
                if let Some(left) = node.child_by_field_name("left")
                    && let Some(scope) = nearest_ancestor(node, &["for_statement"])
                {
                    push_named_identifiers(&left, name, source, &scope, &mut bindings);
                }
            }
            _ => {}
        });
        bindings
    }
}

/// Pushes a [`LocalBinding`] for every direct `identifier` child of `parent`
/// whose text is `name`, governed by `scope`. Used for Go's comma-separated
/// declarations (`a, b int`, `a, b := …`) whose names are sibling identifiers.
fn push_named_identifiers(
    parent: &tree_sitter::Node,
    name: &str,
    source: &str,
    scope: &tree_sitter::Node,
    out: &mut Vec<LocalBinding>,
) {
    let mut cursor = parent.walk();
    for child in parent.children(&mut cursor) {
        if child.kind() == "identifier" && child.utf8_text(source.as_bytes()) == Ok(name) {
            out.push(local_binding(&child, scope));
        }
    }
}

/// The receiver type of a method, e.g. `Scanner` for `func (s *Scanner) Scan()`.
fn receiver_type(method: &tree_sitter::Node, source: &str) -> Option<String> {
    let receiver = method.child_by_field_name("receiver")?;
    first_descendant_text(receiver, "type_identifier", source).map(str::to_string)
}

/// Maps a `type_spec`'s declared type node to a [`SymbolKind`].
fn type_spec_kind(type_spec: &tree_sitter::Node) -> SymbolKind {
    match type_spec.child_by_field_name("type").map(|n| n.kind()) {
        Some("struct_type") => SymbolKind::Struct,
        Some("interface_type") => SymbolKind::Interface,
        _ => SymbolKind::TypeAlias,
    }
}

fn collect(node: tree_sitter::Node, source: &str, out: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(name) = crate::field_name(&child, source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Function,
                        name.to_string(),
                        None,
                    ));
                }
            }
            "method_declaration" => {
                if let Some(name) = crate::field_name(&child, source) {
                    let container = receiver_type(&child, source);
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Method,
                        name.to_string(),
                        container,
                    ));
                }
            }
            // `type ( ... )` / `type Name struct {...}` — one or more type_spec.
            "type_declaration" => {
                let mut spec_cursor = child.walk();
                for spec in child.children(&mut spec_cursor) {
                    if spec.kind() != "type_spec" {
                        continue;
                    }
                    if let Some(name) = crate::field_name(&spec, source) {
                        out.push(symbol_at(
                            &spec,
                            type_spec_kind(&spec),
                            name.to_string(),
                            None,
                        ));
                    }
                }
            }
            // `const ( ... )` / `const X = ...` — one or more const_spec.
            "const_declaration" => {
                let mut spec_cursor = child.walk();
                for spec in child.children(&mut spec_cursor) {
                    if spec.kind() != "const_spec" {
                        continue;
                    }
                    if let Some(name) = crate::field_name(&spec, source) {
                        out.push(symbol_at(&spec, SymbolKind::Const, name.to_string(), None));
                    }
                }
            }
            // source_file and other wrappers.
            _ => collect(child, source, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
package main

type Scanner struct {
    count int
}

type Runner interface {
    Run()
}

const MaxItems = 8

func FreeFunction(x int) int {
    return x + 1
}

func (s *Scanner) Scan() int {
    return s.count
}
"#;

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_top_level_declarations() {
        let symbols = GoSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Struct);
        assert_eq!(
            find(&symbols, "Runner").unwrap().kind,
            SymbolKind::Interface
        );
        assert_eq!(find(&symbols, "MaxItems").unwrap().kind, SymbolKind::Const);
        assert_eq!(
            find(&symbols, "FreeFunction").unwrap().kind,
            SymbolKind::Function
        );
    }

    #[test]
    fn method_carries_receiver_type_as_container() {
        let symbols = GoSymbols.outline(SAMPLE);
        let scan = find(&symbols, "Scan").expect("Scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        assert_eq!(scan.outline_label(), "method Scanner::Scan");
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
package main
func target() {}
// target in a comment
func caller() { target() }
";
        let refs = GoSymbols.references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 2);
        assert_eq!(refs[1].line, 4);
    }
}
