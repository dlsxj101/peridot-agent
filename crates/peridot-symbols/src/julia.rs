//! Julia symbol extraction backed by `tree-sitter-julia`.
//!
//! Julia has `struct`/`function`/`module` definitions plus the short
//! assignment form of a function (`f(x) = ...`). Definitions inside a module
//! carry it as `container`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind,
    first_descendant_text, parse, symbol_at,
};

/// Julia symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct JuliaSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_julia::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier")
}

impl LanguageSymbols for JuliaSymbols {
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

/// The first named child of `node`, skipping anonymous tokens.
fn first_named_child<'a>(node: &tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).find(|child| child.is_named())
}

fn collect(node: tree_sitter::Node, source: &str, container: Option<&str>, out: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "struct_definition" => {
                if let Some(name) = first_descendant_text(child, "identifier", source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Struct,
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                }
            }
            "function_definition" => {
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
            // Short function form: `f(x) = ...` — an assignment whose left side
            // is a call expression.
            "assignment" => {
                if let Some(lhs) = first_named_child(&child)
                    && lhs.kind() == "call_expression"
                    && let Some(name) = first_descendant_text(lhs, "identifier", source)
                {
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
            "module_definition" => {
                if let Some(name) = first_descendant_text(child, "identifier", source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Module,
                        name.to_string(),
                        container.map(str::to_string),
                    ));
                    collect(child, source, Some(name), out);
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
struct Scanner
    count::Int
end

function scan(s)
    s.count
end

free(x) = x

module Helper
    inner(y) = y
end
";

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_structs_functions_and_modules() {
        let symbols = JuliaSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Struct);
        assert_eq!(find(&symbols, "scan").unwrap().kind, SymbolKind::Function);
        assert_eq!(find(&symbols, "free").unwrap().kind, SymbolKind::Function);
        assert_eq!(find(&symbols, "Helper").unwrap().kind, SymbolKind::Module);
    }

    #[test]
    fn module_member_carries_container() {
        let symbols = JuliaSymbols.outline(SAMPLE);
        let inner = find(&symbols, "inner").expect("module function");
        assert_eq!(inner.container.as_deref(), Some("Helper"));
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
function target()
    1
end
# target in a comment
caller() = target() + target()
";
        let refs = JuliaSymbols.references(source, "target");
        // definition + two usages = 3 occurrences.
        assert_eq!(refs.len(), 3, "{refs:?}");
    }
}
