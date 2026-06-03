//! Dart symbol extraction backed by `tree-sitter-dart`.
//!
//! Dart classes/mixins/extensions wrap their members in a `class_body`;
//! methods are `method_declaration`, fields are `declaration`, and top-level
//! functions are `function_declaration`. Members carry their enclosing type as
//! `container`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, first_child_text,
    first_descendant_text, parse, symbol_at,
};

/// Dart symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct DartSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_dart::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier")
}

impl LanguageSymbols for DartSymbols {
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

fn collect(node: tree_sitter::Node, source: &str, container: Option<&str>, out: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "mixin_declaration" | "extension_declaration" => {
                if let Some(name) = first_child_text(&child, "identifier", source) {
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
            "enum_declaration" => {
                if let Some(name) = first_child_text(&child, "identifier", source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Enum,
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                }
            }
            "method_declaration" => {
                if let Some(name) = first_descendant_text(child, "identifier", source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Method,
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                }
            }
            "function_declaration" => {
                if let Some(name) = first_descendant_text(child, "identifier", source) {
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
            }
            // A field inside a class body.
            "declaration" if container.is_some() => {
                if let Some(name) = first_descendant_text(child, "identifier", source) {
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
class Scanner {
  int count = 0;
  int scan() => count;
}

enum Direction { north, south }

int free(int x) => x;
";

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_classes_enums_and_functions() {
        let symbols = DartSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Class);
        assert_eq!(find(&symbols, "Direction").unwrap().kind, SymbolKind::Enum);
        assert_eq!(find(&symbols, "free").unwrap().kind, SymbolKind::Function);
    }

    #[test]
    fn method_and_field_carry_container() {
        let symbols = DartSymbols.outline(SAMPLE);
        let scan = find(&symbols, "scan").expect("scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        assert_eq!(
            find(&symbols, "count").unwrap().container.as_deref(),
            Some("Scanner")
        );
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
void target() {}
// target in a comment
void caller() { target(); }
";
        let refs = DartSymbols.references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 3);
    }
}
