//! Shell (Bash) symbol extraction backed by `tree-sitter-bash`.
//!
//! Shell scripts only expose functions as named definitions, so the outline is
//! the set of `function_definition`s — useful for navigating large scripts.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// Bash / shell symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct BashSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_bash::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "word" | "variable_name")
}

impl LanguageSymbols for BashSymbols {
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
        if child.kind() == "function_definition"
            && let Some(name) = field_name(&child, source)
        {
            out.push(symbol_at(
                &child,
                SymbolKind::Function,
                name.to_string(),
                None,
            ));
        }
        collect(child, source, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_functions() {
        let source = "\
#!/bin/bash
greet() {
    echo hi
}

function deploy {
    greet
}
";
        let symbols = BashSymbols.outline(source);
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "greet" && s.kind == SymbolKind::Function),
            "{symbols:?}"
        );
        assert!(symbols.iter().any(|s| s.name == "deploy"), "{symbols:?}");
    }

    #[test]
    fn references_find_function_uses() {
        let source = "\
greet() { echo hi; }
greet
";
        let refs = BashSymbols.references(source, "greet");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 2);
    }
}
