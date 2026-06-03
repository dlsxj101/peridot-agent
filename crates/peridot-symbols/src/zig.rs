//! Zig symbol extraction backed by `tree-sitter-zig`.
//!
//! Zig declares types as `const`-bound `struct` / `enum` / `union`
//! expressions, so a `variable_declaration` is a type when its value is one of
//! those and a constant/variable otherwise. Functions and fields inside a type
//! body carry it as `container`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, first_child_text,
    parse, symbol_at,
};

/// Zig symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct ZigSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_zig::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier")
}

impl LanguageSymbols for ZigSymbols {
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
}

/// The type-declaration node a `const`/`var` is bound to, if any.
fn type_value<'a>(decl: &tree_sitter::Node<'a>) -> Option<(tree_sitter::Node<'a>, SymbolKind)> {
    let mut cursor = decl.walk();
    decl.children(&mut cursor)
        .find_map(|child| match child.kind() {
            "struct_declaration" => Some((child, SymbolKind::Struct)),
            "enum_declaration" => Some((child, SymbolKind::Enum)),
            "union_declaration" | "opaque_declaration" => Some((child, SymbolKind::Struct)),
            _ => None,
        })
}

fn has_token(node: &tree_sitter::Node, token: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == token)
}

fn collect(node: tree_sitter::Node, source: &str, container: Option<&str>, out: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "variable_declaration" => {
                let Some(name) = first_child_text(&child, "identifier", source) else {
                    collect(child, source, container, out);
                    continue;
                };
                if let Some((type_node, kind)) = type_value(&child) {
                    out.push(symbol_at(
                        &child,
                        kind,
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                    collect(type_node, source, Some(name), out);
                } else {
                    let kind = if has_token(&child, "var") {
                        SymbolKind::Variable
                    } else {
                        SymbolKind::Const
                    };
                    out.push(symbol_at(
                        &child,
                        kind,
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                }
            }
            "function_declaration" => {
                if let Some(name) = first_child_text(&child, "identifier", source) {
                    let kind = if container.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    out.push(symbol_at(
                        &child,
                        kind,
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                }
                // Skip function bodies.
            }
            "container_field" => {
                if let Some(name) = first_child_text(&child, "identifier", source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Variable,
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                }
            }
            _ => collect(child, source, container, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
const Scanner = struct {
    count: u32,
    pub fn scan(self: *Scanner) u32 {
        return self.count;
    }
};

fn free(x: u32) u32 {
    return x;
}

const TOP = 9;
var counter = 0;
";

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_structs_functions_and_constants() {
        let symbols = ZigSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Struct);
        assert_eq!(find(&symbols, "free").unwrap().kind, SymbolKind::Function);
        assert_eq!(find(&symbols, "TOP").unwrap().kind, SymbolKind::Const);
        assert_eq!(
            find(&symbols, "counter").unwrap().kind,
            SymbolKind::Variable
        );
    }

    #[test]
    fn method_and_field_carry_container() {
        let symbols = ZigSymbols.outline(SAMPLE);
        let scan = find(&symbols, "scan").expect("scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        let count = find(&symbols, "count").expect("count field");
        assert_eq!(count.container.as_deref(), Some("Scanner"));
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
fn target() void {}
// target in a comment
fn caller() void { target(); }
";
        let refs = ZigSymbols.references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 3);
    }
}
