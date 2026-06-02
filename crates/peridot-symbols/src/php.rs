//! PHP symbol extraction backed by `tree-sitter-php`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// PHP symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct PhpSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_php::LANGUAGE_PHP.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "name")
}

/// Maps a PHP type-declaration node kind to a [`SymbolKind`].
fn type_kind(node_kind: &str) -> Option<SymbolKind> {
    Some(match node_kind {
        "class_declaration" => SymbolKind::Class,
        "interface_declaration" => SymbolKind::Interface,
        "trait_declaration" => SymbolKind::Trait,
        "enum_declaration" => SymbolKind::Enum,
        _ => return None,
    })
}

impl LanguageSymbols for PhpSymbols {
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

fn collect(
    node: tree_sitter::Node,
    source: &str,
    container: Option<String>,
    out: &mut Vec<Symbol>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Function,
                        name.to_string(),
                        None,
                    ));
                }
            }
            "method_declaration" => {
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Method,
                        name.to_string(),
                        container.clone(),
                    ));
                }
            }
            other => {
                if let Some(kind) = type_kind(other) {
                    let name = field_name(&child, source).map(str::to_string);
                    if let Some(name) = name.clone() {
                        out.push(symbol_at(&child, kind, name, container.clone()));
                    }
                    collect(child, source, name, out);
                } else {
                    collect(child, source, container.clone(), out);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?php
function freeFunction($x) {
    return $x + 1;
}

interface Runner {
    public function run();
}

class Scanner {
    private $count = 0;

    public function scan() {
        return $this->count;
    }
}
"#;

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_functions_classes_interfaces() {
        let symbols = PhpSymbols.outline(SAMPLE);
        assert_eq!(
            find(&symbols, "freeFunction").unwrap().kind,
            SymbolKind::Function
        );
        assert_eq!(
            find(&symbols, "Runner").unwrap().kind,
            SymbolKind::Interface
        );
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Class);
    }

    #[test]
    fn methods_carry_class_container() {
        let symbols = PhpSymbols.outline(SAMPLE);
        let scan = find(&symbols, "scan").expect("scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "<?php
function target() {}
// target in a comment
target();
";
        let refs = PhpSymbols.references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 2);
        assert_eq!(refs[1].line, 4);
    }
}
