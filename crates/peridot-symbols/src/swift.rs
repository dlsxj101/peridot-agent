//! Swift symbol extraction backed by `tree-sitter-swift`.
//!
//! Swift folds class/struct/enum/actor/extension into one
//! `class_declaration` node with a `declaration_kind` field; protocols get
//! their own node. Functions and properties inside a type body carry that
//! type as `container`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// Swift symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct SwiftSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_swift::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "simple_identifier" | "type_identifier")
}

impl LanguageSymbols for SwiftSymbols {
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

/// Maps the `declaration_kind` field of a `class_declaration` (class / struct /
/// enum / actor / extension) to a [`SymbolKind`].
fn class_declaration_kind(node: &tree_sitter::Node, source: &str) -> SymbolKind {
    match node
        .child_by_field_name("declaration_kind")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
    {
        Some("struct") => SymbolKind::Struct,
        Some("enum") => SymbolKind::Enum,
        Some("extension") => SymbolKind::Impl,
        // class / actor / anything else.
        _ => SymbolKind::Class,
    }
}

fn collect(node: tree_sitter::Node, source: &str, container: Option<&str>, out: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration" => {
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(
                        &child,
                        class_declaration_kind(&child, source),
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                    collect(child, source, Some(name), out);
                } else {
                    collect(child, source, container, out);
                }
            }
            "protocol_declaration" => {
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Interface,
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                    collect(child, source, Some(name), out);
                } else {
                    collect(child, source, container, out);
                }
            }
            "function_declaration" | "protocol_function_declaration" => {
                if let Some(name) = field_name(&child, source) {
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
                // Skip function bodies (local declarations).
            }
            "property_declaration" => {
                if let Some(name) = field_name(&child, source) {
                    let kind = if has_descendant_token(&child, "var") {
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
            _ => collect(child, source, container, out),
        }
    }
}

/// Whether the subtree rooted at `node` contains a token of the given kind. A
/// Swift property's `let`/`var` keyword sits inside a `value_binding_pattern`
/// child rather than directly on the declaration.
fn has_descendant_token(node: &tree_sitter::Node, token: &str) -> bool {
    if node.kind() == token {
        return true;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| has_descendant_token(&child, token))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
class Scanner {
    func scan() -> Int { return 0 }
    var size = 3
}

struct Point { let x: Int }

enum Direction { case north }

protocol Runner { func run() }

func free(x: Int) -> Int { return x }

let TOP = 9
";

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_types_functions_and_properties() {
        let symbols = SwiftSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Class);
        assert_eq!(find(&symbols, "Point").unwrap().kind, SymbolKind::Struct);
        assert_eq!(find(&symbols, "Direction").unwrap().kind, SymbolKind::Enum);
        assert_eq!(
            find(&symbols, "Runner").unwrap().kind,
            SymbolKind::Interface
        );
        assert_eq!(find(&symbols, "free").unwrap().kind, SymbolKind::Function);
        assert_eq!(find(&symbols, "TOP").unwrap().kind, SymbolKind::Const);
    }

    #[test]
    fn method_and_property_carry_container() {
        let symbols = SwiftSymbols.outline(SAMPLE);
        let scan = find(&symbols, "scan").expect("scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        assert_eq!(scan.outline_label(), "method Scanner::scan");
        let size = find(&symbols, "size").expect("size property");
        assert_eq!(size.kind, SymbolKind::Variable);
        assert_eq!(size.container.as_deref(), Some("Scanner"));
    }

    #[test]
    fn extension_is_an_impl() {
        let symbols = SwiftSymbols.outline("extension Scanner { func extra() {} }\n");
        let ext = find(&symbols, "Scanner").expect("extension");
        assert_eq!(ext.kind, SymbolKind::Impl);
        assert_eq!(
            find(&symbols, "extra").unwrap().container.as_deref(),
            Some("Scanner")
        );
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
func target() {}
// target in a comment
func caller() { target() }
";
        let refs = SwiftSymbols.references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 3);
    }
}
