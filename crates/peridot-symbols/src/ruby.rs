//! Ruby symbol extraction backed by `tree-sitter-ruby`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// Ruby symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct RubySymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_ruby::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier" | "constant")
}

impl LanguageSymbols for RubySymbols {
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
            "method" | "singleton_method" => {
                // A `def` inside a class/module is a method; at the top level
                // it is a free function.
                let kind = if container.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(&child, kind, name.to_string(), container.clone()));
                }
                // Bodies rarely nest defs; keep the current container.
                collect(child, source, container.clone(), out);
            }
            "class" | "module" => {
                let kind = if child.kind() == "class" {
                    SymbolKind::Class
                } else {
                    SymbolKind::Module
                };
                let name = field_name(&child, source).map(str::to_string);
                if let Some(name) = name.clone() {
                    out.push(symbol_at(&child, kind, name, container.clone()));
                }
                collect(child, source, name, out);
            }
            _ => collect(child, source, container.clone(), out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
def free_function(x)
  x + 1
end

module Helpers
  def helper
    1
  end
end

class Scanner
  def initialize
    @count = 0
  end

  def scan
    @count
  end
end
"#;

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_classes_modules_and_methods() {
        let symbols = RubySymbols.outline(SAMPLE);
        assert_eq!(
            find(&symbols, "free_function").unwrap().kind,
            SymbolKind::Function
        );
        assert_eq!(find(&symbols, "Helpers").unwrap().kind, SymbolKind::Module);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Class);
    }

    #[test]
    fn methods_carry_their_class_container() {
        let symbols = RubySymbols.outline(SAMPLE);
        let scan = find(&symbols, "scan").expect("scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        let helper = find(&symbols, "helper").expect("helper method");
        assert_eq!(helper.container.as_deref(), Some("Helpers"));
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
def target
end
# target in a comment
target()
";
        let refs = RubySymbols.references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 4);
    }
}
