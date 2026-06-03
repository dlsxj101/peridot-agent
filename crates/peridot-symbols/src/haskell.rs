//! Haskell symbol extraction backed by `tree-sitter-haskell`.
//!
//! Haskell definitions live directly under the top-level `declarations` node:
//! `data` / `newtype` / `type` declarations, type `class`es (mapped to traits,
//! their method signatures carried as methods), and function equations.
//! Functions defined by several pattern-matching equations appear once each in
//! the tree; they are de-duplicated by name so the outline lists a function a
//! single time.

use std::collections::HashSet;

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// Haskell symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct HaskellSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_haskell::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "variable" | "name" | "constructor")
}

impl LanguageSymbols for HaskellSymbols {
    fn outline(&self, source: &str) -> Vec<Symbol> {
        let Some(tree) = parse(&language(), source) else {
            return Vec::new();
        };
        let mut symbols = Vec::new();
        let mut seen_functions = HashSet::new();
        collect(tree.root_node(), source, &mut seen_functions, &mut symbols);
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

/// Emits the method signatures declared inside a type `class` body, each
/// carrying the class as its container.
fn collect_class_methods(
    class: &tree_sitter::Node,
    source: &str,
    owner: &str,
    out: &mut Vec<Symbol>,
) {
    let Some(declarations) = class.child_by_field_name("declarations") else {
        return;
    };
    let mut cursor = declarations.walk();
    for decl in declarations.children(&mut cursor) {
        if decl.kind() == "signature"
            && let Some(name) = field_name(&decl, source)
        {
            out.push(symbol_at(
                &decl,
                SymbolKind::Method,
                name.to_string(),
                Some(owner.to_string()),
            ));
        }
    }
}

/// The bound name of a simple `bind` (the leading `variable`). Pattern binds
/// such as `(a, b) = ...` have no single name and yield `None`.
fn bind_name<'a>(bind: &tree_sitter::Node, source: &'a str) -> Option<&'a str> {
    let mut cursor = bind.walk();
    bind.children(&mut cursor)
        .find(|child| child.kind() == "variable")
        .and_then(|child| child.utf8_text(source.as_bytes()).ok())
}

fn collect(
    node: tree_sitter::Node,
    source: &str,
    seen_functions: &mut HashSet<String>,
    out: &mut Vec<Symbol>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "data_type" => {
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(&child, SymbolKind::Enum, name.to_string(), None));
                }
            }
            "newtype" => {
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Struct,
                        name.to_string(),
                        None,
                    ));
                }
            }
            "type_synomym" => {
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::TypeAlias,
                        name.to_string(),
                        None,
                    ));
                }
            }
            "class" => {
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(&child, SymbolKind::Trait, name.to_string(), None));
                    collect_class_methods(&child, source, name, out);
                }
            }
            "function" => {
                if let Some(name) = field_name(&child, source)
                    && seen_functions.insert(name.to_string())
                {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Function,
                        name.to_string(),
                        None,
                    ));
                }
            }
            // A zero-argument top-level binding (`main = ...`, a constant) is a
            // `bind` rather than a `function`; its name is the leading variable.
            "bind" => {
                if let Some(name) = bind_name(&child, source)
                    && seen_functions.insert(name.to_string())
                {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Function,
                        name.to_string(),
                        None,
                    ));
                }
            }
            // `haskell` root and `declarations` wrapper.
            _ => collect(child, source, seen_functions, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
module M where

data Color = Red | Blue

newtype Wrap = Wrap Int

type Name = String

class Show a where
  display :: a -> String

scan :: Int -> Int
scan 0 = 0
scan x = x + 1
";

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_types_classes_and_functions() {
        let symbols = HaskellSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "Color").unwrap().kind, SymbolKind::Enum);
        assert_eq!(find(&symbols, "Wrap").unwrap().kind, SymbolKind::Struct);
        assert_eq!(find(&symbols, "Name").unwrap().kind, SymbolKind::TypeAlias);
        assert_eq!(find(&symbols, "Show").unwrap().kind, SymbolKind::Trait);
        assert_eq!(find(&symbols, "scan").unwrap().kind, SymbolKind::Function);
    }

    #[test]
    fn class_method_carries_container() {
        let symbols = HaskellSymbols.outline(SAMPLE);
        let display = find(&symbols, "display").expect("class method");
        assert_eq!(display.kind, SymbolKind::Method);
        assert_eq!(display.container.as_deref(), Some("Show"));
    }

    #[test]
    fn multi_equation_function_appears_once() {
        let symbols = HaskellSymbols.outline(SAMPLE);
        assert_eq!(
            symbols.iter().filter(|s| s.name == "scan").count(),
            1,
            "{symbols:?}"
        );
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
target :: Int
target = 1
-- target in a comment
caller = target + target
";
        let refs = HaskellSymbols.references(source, "target");
        // signature + definition + two usages = 4 occurrences.
        assert_eq!(refs.len(), 4, "{refs:?}");
    }
}
