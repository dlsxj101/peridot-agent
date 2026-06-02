//! Scala symbol extraction backed by `tree-sitter-scala`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// Scala symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct ScalaSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_scala::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier" | "type_identifier")
}

/// Maps a Scala type-definition node kind to a [`SymbolKind`].
fn type_kind(node_kind: &str) -> Option<SymbolKind> {
    Some(match node_kind {
        "class_definition" => SymbolKind::Class,
        "trait_definition" => SymbolKind::Trait,
        // A Scala `object` is a singleton namespace.
        "object_definition" => SymbolKind::Module,
        "enum_definition" => SymbolKind::Enum,
        _ => return None,
    })
}

impl LanguageSymbols for ScalaSymbols {
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
            "function_definition" | "function_declaration" => {
                if let Some(name) = field_name(&child, source) {
                    let kind = if container.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    out.push(symbol_at(&child, kind, name.to_string(), container.clone()));
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

    const SAMPLE: &str = r#"
trait Runner {
  def run(): Unit
}

object Helpers {
  def helper(): Int = 1
}

class Scanner {
  def scan(): Int = 0
}

def freeFunction(x: Int): Int = x + 1
"#;

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_traits_objects_classes() {
        let symbols = ScalaSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "Runner").unwrap().kind, SymbolKind::Trait);
        assert_eq!(find(&symbols, "Helpers").unwrap().kind, SymbolKind::Module);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Class);
    }

    #[test]
    fn methods_carry_their_owner_as_container() {
        let symbols = ScalaSymbols.outline(SAMPLE);
        let scan = find(&symbols, "scan").expect("scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        let helper = find(&symbols, "helper").expect("helper method");
        assert_eq!(helper.container.as_deref(), Some("Helpers"));
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
def target(): Int = 0
// target in a comment
def caller(): Int = target()
";
        let refs = ScalaSymbols.references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 3);
    }
}
