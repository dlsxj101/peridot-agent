//! C# symbol extraction backed by `tree-sitter-c-sharp`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// C# symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct CSharpSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_c_sharp::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier")
}

/// Maps a C# type-declaration node kind to a [`SymbolKind`].
fn type_kind(node_kind: &str) -> Option<SymbolKind> {
    Some(match node_kind {
        "class_declaration" | "record_declaration" => SymbolKind::Class,
        "struct_declaration" => SymbolKind::Struct,
        "interface_declaration" => SymbolKind::Interface,
        "enum_declaration" => SymbolKind::Enum,
        "namespace_declaration" | "file_scoped_namespace_declaration" => SymbolKind::Module,
        _ => return None,
    })
}

impl LanguageSymbols for CSharpSymbols {
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
            "method_declaration" | "constructor_declaration" | "destructor_declaration" => {
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
                    // A namespace qualifies but does not own methods; types do.
                    let child_container = if kind == SymbolKind::Module {
                        container.clone()
                    } else {
                        name
                    };
                    collect(child, source, child_container, out);
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

    const SAMPLE: &str = r#"
namespace App {
    public interface IRunner { void Run(); }

    public enum Color { Red, Green }

    public class Scanner {
        private int count;
        public Scanner() { count = 0; }
        public int Scan() { return count; }
    }
}
"#;

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_types_and_namespace() {
        let symbols = CSharpSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "App").unwrap().kind, SymbolKind::Module);
        assert_eq!(
            find(&symbols, "IRunner").unwrap().kind,
            SymbolKind::Interface
        );
        assert_eq!(find(&symbols, "Color").unwrap().kind, SymbolKind::Enum);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Class);
    }

    #[test]
    fn methods_carry_class_container() {
        let symbols = CSharpSymbols.outline(SAMPLE);
        let scan = find(&symbols, "Scan").expect("Scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
class A {
    void Target() {}
    // Target in a comment
    void Caller() { Target(); }
}
";
        let refs = CSharpSymbols.references(source, "Target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 2);
        assert_eq!(refs[1].line, 4);
    }
}
