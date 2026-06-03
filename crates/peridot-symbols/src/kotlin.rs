//! Kotlin symbol extraction backed by `tree-sitter-kotlin-ng`.
//!
//! Kotlin packs classes, interfaces, and enums into a single
//! `class_declaration` node distinguished by a keyword token (`interface`) or
//! an `enum_class_body` child; objects get their own `object_declaration`.
//! Functions and properties carry their enclosing type as `container`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name,
    first_descendant_text, parse, symbol_at,
};

/// Kotlin symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct KotlinSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_kotlin_ng::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier")
}

impl LanguageSymbols for KotlinSymbols {
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

/// Whether `node` has a direct child token of the given kind.
fn has_token(node: &tree_sitter::Node, token: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == token)
}

/// Maps a `class_declaration` node to its kind: an `interface` keyword makes it
/// an interface, an `enum_class_body` makes it an enum, otherwise a class.
fn class_declaration_kind(node: &tree_sitter::Node) -> SymbolKind {
    if has_token(node, "interface") {
        SymbolKind::Interface
    } else if has_token(node, "enum_class_body") {
        SymbolKind::Enum
    } else {
        SymbolKind::Class
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
                        class_declaration_kind(&child),
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                    collect(child, source, Some(name), out);
                } else {
                    collect(child, source, container, out);
                }
            }
            "object_declaration" => {
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Class,
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                    collect(child, source, Some(name), out);
                } else {
                    collect(child, source, container, out);
                }
            }
            "function_declaration" => {
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
                // Do not descend into function bodies: local declarations
                // would otherwise be attributed to the enclosing type.
            }
            "property_declaration" => {
                if let Some(name) = first_descendant_text(child, "identifier", source) {
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
            _ => collect(child, source, container, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
package p

class Scanner(val n: Int) {
    fun scan(): Int { return n }
    val size = 3
    var mutableCount = 0
}

object Registry {
    fun get() {}
}

interface Runner {
    fun run()
}

fun free(x: Int): Int = x

val TOP = 9
";

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_types_functions_and_properties() {
        let symbols = KotlinSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Class);
        assert_eq!(find(&symbols, "Registry").unwrap().kind, SymbolKind::Class);
        assert_eq!(
            find(&symbols, "Runner").unwrap().kind,
            SymbolKind::Interface
        );
        assert_eq!(find(&symbols, "free").unwrap().kind, SymbolKind::Function);
        assert_eq!(find(&symbols, "TOP").unwrap().kind, SymbolKind::Const);
    }

    #[test]
    fn method_and_property_carry_container() {
        let symbols = KotlinSymbols.outline(SAMPLE);
        let scan = find(&symbols, "scan").expect("scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        assert_eq!(scan.outline_label(), "method Scanner::scan");
        assert_eq!(find(&symbols, "size").unwrap().kind, SymbolKind::Const);
        assert_eq!(
            find(&symbols, "mutableCount").unwrap().kind,
            SymbolKind::Variable
        );
    }

    #[test]
    fn enum_class_is_an_enum() {
        let symbols = KotlinSymbols.outline("enum class Color { RED, BLUE }\n");
        assert_eq!(find(&symbols, "Color").unwrap().kind, SymbolKind::Enum);
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
fun target() {}
// target in a comment
fun caller() { target() }
";
        let refs = KotlinSymbols.references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 3);
    }
}
