//! Python symbol extraction backed by `tree-sitter-python`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// Python symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct PythonSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_python::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    kind == "identifier"
}

impl LanguageSymbols for PythonSymbols {
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

/// Depth-first walk. A `function_definition` directly inside a class body is a
/// method (carrying the class as `container`); elsewhere it is a function.
/// Nested functions inside a method are plain functions, not methods of the
/// class, so the class container is *not* threaded through function bodies.
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
                let kind = if container.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(&child, kind, name.to_string(), container.clone()));
                }
                // Nested defs are plain functions: drop the class container.
                collect(child, source, None, out);
            }
            "class_definition" => {
                let name = field_name(&child, source).map(str::to_string);
                if let Some(name) = name.clone() {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Class,
                        name,
                        container.clone(),
                    ));
                }
                // Methods inside the class body get the class as container.
                collect(child, source, name, out);
            }
            // `decorated_definition`, `module`, `block`, ... — descend.
            _ => collect(child, source, container.clone(), out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
def free_function(x):
    return x + 1

class Scanner:
    def __init__(self):
        self.count = 0

    def scan(self):
        def nested_helper():
            return 1
        return nested_helper()

@decorator
def decorated():
    pass
"#;

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_functions_and_classes() {
        let symbols = PythonSymbols.outline(SAMPLE);
        assert_eq!(
            find(&symbols, "free_function").unwrap().kind,
            SymbolKind::Function
        );
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Class);
        assert_eq!(
            find(&symbols, "decorated").unwrap().kind,
            SymbolKind::Function
        );
    }

    #[test]
    fn methods_carry_class_container() {
        let symbols = PythonSymbols.outline(SAMPLE);
        let scan = find(&symbols, "scan").expect("scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        let init = find(&symbols, "__init__").expect("init method");
        assert_eq!(init.container.as_deref(), Some("Scanner"));
        assert_eq!(scan.outline_label(), "method Scanner::scan");
    }

    #[test]
    fn nested_function_is_not_a_method() {
        let symbols = PythonSymbols.outline(SAMPLE);
        let nested = find(&symbols, "nested_helper").expect("nested fn");
        assert_eq!(nested.kind, SymbolKind::Function);
        assert!(nested.container.is_none());
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
def target():
    pass
# target in a comment
s = \"target in a string\"
target()
";
        let refs = PythonSymbols.references(source, "target");
        // definition (line 1) + call (line 5); comment and string excluded.
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 5);
    }
}
