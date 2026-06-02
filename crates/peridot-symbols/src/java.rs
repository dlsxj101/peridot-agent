//! Java symbol extraction backed by `tree-sitter-java`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// Java symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct JavaSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_java::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier" | "type_identifier")
}

/// Maps a Java type-declaration node kind to a [`SymbolKind`].
fn type_kind(node_kind: &str) -> Option<SymbolKind> {
    Some(match node_kind {
        "class_declaration" | "record_declaration" => SymbolKind::Class,
        "interface_declaration" | "annotation_type_declaration" => SymbolKind::Interface,
        "enum_declaration" => SymbolKind::Enum,
        _ => return None,
    })
}

impl LanguageSymbols for JavaSymbols {
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
            "method_declaration" | "constructor_declaration" => {
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
                    // Members of this type get it as their container.
                    collect(child, source, name, out);
                } else {
                    // program, class_body, ... — descend, keep container.
                    collect(child, source, container.clone(), out);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
package com.example;

public interface Runner {
    void run();
}

public enum Color { RED, GREEN }

public class Scanner {
    private int count;

    public Scanner() {
        this.count = 0;
    }

    public int scan() {
        return count;
    }
}
"#;

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_types() {
        let symbols = JavaSymbols.outline(SAMPLE);
        assert_eq!(
            find(&symbols, "Runner").unwrap().kind,
            SymbolKind::Interface
        );
        assert_eq!(find(&symbols, "Color").unwrap().kind, SymbolKind::Enum);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Class);
    }

    #[test]
    fn methods_and_constructors_carry_class_container() {
        let symbols = JavaSymbols.outline(SAMPLE);
        let scan = find(&symbols, "scan").expect("scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        // The constructor shares the class name; ensure it is attributed too.
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "Scanner" && s.kind == SymbolKind::Method)
        );
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
class A {
    void target() {}
    // target in a comment
    void caller() { target(); }
}
";
        let refs = JavaSymbols.references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 2);
        assert_eq!(refs[1].line, 4);
    }
}
