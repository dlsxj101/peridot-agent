//! OCaml symbol extraction backed by `tree-sitter-ocaml` (the `.ml` grammar).
//!
//! OCaml names values with `let` (a function when the binding takes
//! parameters, a value otherwise), types with `type` (a variant or record
//! becomes an enum/struct, otherwise a type alias), and modules with `module`.
//! Bindings inside a module structure carry the module as `container`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, first_child_text,
    parse, symbol_at,
};

/// OCaml symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct OCamlSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_ocaml::LANGUAGE_OCAML.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "value_name" | "type_constructor" | "constructor_name" | "module_name"
    )
}

impl LanguageSymbols for OCamlSymbols {
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

fn has_child(node: &tree_sitter::Node, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| child.kind() == kind)
}

fn type_binding_kind(binding: &tree_sitter::Node) -> SymbolKind {
    if has_child(binding, "variant_declaration") {
        SymbolKind::Enum
    } else if has_child(binding, "record_declaration") {
        SymbolKind::Struct
    } else {
        SymbolKind::TypeAlias
    }
}

fn collect(node: tree_sitter::Node, source: &str, container: Option<&str>, out: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_definition" => {
                let mut bindings = child.walk();
                for binding in child.children(&mut bindings) {
                    if binding.kind() == "type_binding"
                        && let Some(name) = first_child_text(&binding, "type_constructor", source)
                    {
                        out.push(symbol_at(
                            &binding,
                            type_binding_kind(&binding),
                            name.to_string(),
                            container.map(str::to_string),
                        ));
                    }
                }
            }
            "value_definition" => {
                let mut bindings = child.walk();
                for binding in child.children(&mut bindings) {
                    if binding.kind() == "let_binding"
                        && let Some(name) = first_child_text(&binding, "value_name", source)
                    {
                        let kind = if has_child(&binding, "parameter") {
                            SymbolKind::Function
                        } else {
                            SymbolKind::Const
                        };
                        out.push(symbol_at(
                            &binding,
                            kind,
                            name.to_string(),
                            container.map(str::to_string),
                        ));
                    }
                }
            }
            "module_definition" => {
                let mut bindings = child.walk();
                for binding in child.children(&mut bindings) {
                    if binding.kind() == "module_binding"
                        && let Some(name) = first_child_text(&binding, "module_name", source)
                    {
                        out.push(symbol_at(
                            &binding,
                            SymbolKind::Module,
                            name.to_string(),
                            container.map(str::to_string),
                        ));
                        collect(binding, source, Some(name), out);
                    }
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
type color = Red | Blue

type point = { x : int; y : int }

type name = string

let scan x = x + 1

let top = 9

module Helper = struct
  let inner y = y
end
";

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_types_values_and_modules() {
        let symbols = OCamlSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "color").unwrap().kind, SymbolKind::Enum);
        assert_eq!(find(&symbols, "point").unwrap().kind, SymbolKind::Struct);
        assert_eq!(find(&symbols, "name").unwrap().kind, SymbolKind::TypeAlias);
        assert_eq!(find(&symbols, "scan").unwrap().kind, SymbolKind::Function);
        assert_eq!(find(&symbols, "top").unwrap().kind, SymbolKind::Const);
        assert_eq!(find(&symbols, "Helper").unwrap().kind, SymbolKind::Module);
    }

    #[test]
    fn module_member_carries_container() {
        let symbols = OCamlSymbols.outline(SAMPLE);
        let inner = find(&symbols, "inner").expect("module value");
        assert_eq!(inner.container.as_deref(), Some("Helper"));
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
let target x = x
(* target in a comment *)
let caller = target 1
";
        let refs = OCamlSymbols.references(source, "target");
        // definition + one usage = 2 occurrences.
        assert_eq!(refs.len(), 2, "{refs:?}");
    }
}
