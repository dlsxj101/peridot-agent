//! Elm symbol extraction backed by `tree-sitter-elm`.
//!
//! Elm declares union types (`type`), type aliases (`type alias`), and values
//! / functions (`value_declaration`, named by the leading lower-case
//! identifier of its `function_declaration_left`). Type annotations are
//! skipped so a function with a separate signature line is listed once.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, first_child_text,
    first_descendant_text, parse, symbol_at,
};

/// Elm symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct ElmSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_elm::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "lower_case_identifier" | "upper_case_identifier")
}

impl LanguageSymbols for ElmSymbols {
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
}

fn collect(node: tree_sitter::Node, source: &str, out: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_declaration" => {
                if let Some(name) = first_child_text(&child, "upper_case_identifier", source) {
                    out.push(symbol_at(&child, SymbolKind::Enum, name.to_string(), None));
                }
            }
            "type_alias_declaration" => {
                if let Some(name) = first_child_text(&child, "upper_case_identifier", source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::TypeAlias,
                        name.to_string(),
                        None,
                    ));
                }
            }
            "value_declaration" => {
                if let Some(name) = first_descendant_text(child, "lower_case_identifier", source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Function,
                        name.to_string(),
                        None,
                    ));
                }
            }
            _ => collect(child, source, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
module M exposing (..)

type Color = Red | Blue

type alias Point = { x : Int, y : Int }

scan : Int -> Int
scan x =
    x + 1
";

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_types_aliases_and_functions() {
        let symbols = ElmSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "Color").unwrap().kind, SymbolKind::Enum);
        assert_eq!(find(&symbols, "Point").unwrap().kind, SymbolKind::TypeAlias);
        assert_eq!(find(&symbols, "scan").unwrap().kind, SymbolKind::Function);
    }

    #[test]
    fn function_with_signature_appears_once() {
        let symbols = ElmSymbols.outline(SAMPLE);
        assert_eq!(symbols.iter().filter(|s| s.name == "scan").count(), 1);
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
target : Int
target = 1
caller = target + target
";
        let refs = ElmSymbols.references(source, "target");
        // signature + definition + two usages = 4 occurrences.
        assert_eq!(refs.len(), 4, "{refs:?}");
    }
}
