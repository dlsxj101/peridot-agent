//! Elixir symbol extraction backed by `tree-sitter-elixir`.
//!
//! Elixir has no dedicated declaration nodes: `defmodule` / `def` / `defp` /
//! `defmacro` are ordinary `call` nodes whose target identifier is the macro
//! name. We walk every call, recognize those forms, and recover the defined
//! name from the call arguments. Functions and macros carry their enclosing
//! module as `container`.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind,
    first_descendant_text, parse, symbol_at,
};

/// Elixir symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct ElixirSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_elixir::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier" | "alias")
}

impl LanguageSymbols for ElixirSymbols {
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

/// The target identifier of a `call` node (`def`, `defmodule`, `scan`, ...).
fn call_head<'a>(call: &tree_sitter::Node, source: &'a str) -> Option<&'a str> {
    call.child_by_field_name("target")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
}

/// The `arguments` child of a `call` node, if any.
fn call_arguments<'a>(call: &'a tree_sitter::Node) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = call.walk();
    call.children(&mut cursor)
        .find(|child| child.kind() == "arguments")
}

/// The name a `def`/`defp`/`defmacro` introduces, read from its first argument
/// (the function head `scan(x)` or a bare `scan`).
fn definition_name<'a>(call: &tree_sitter::Node, source: &'a str) -> Option<&'a str> {
    let arguments = call_arguments(call)?;
    first_descendant_text(arguments, "identifier", source)
}

fn collect(node: tree_sitter::Node, source: &str, container: Option<&str>, out: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call" {
            match call_head(&child, source) {
                Some("defmodule") => {
                    if let Some(name) = call_arguments(&child)
                        .and_then(|args| first_descendant_text(args, "alias", source))
                    {
                        out.push(symbol_at(
                            &child,
                            SymbolKind::Module,
                            name.to_string(),
                            container.map(str::to_string),
                        ));
                        collect(child, source, Some(name), out);
                        continue;
                    }
                }
                Some("def") | Some("defp") => {
                    if let Some(name) = definition_name(&child, source) {
                        out.push(symbol_at(
                            &child,
                            SymbolKind::Function,
                            name.to_string(),
                            container.map(str::to_string),
                        ));
                    }
                }
                Some("defmacro") | Some("defmacrop") => {
                    if let Some(name) = definition_name(&child, source) {
                        out.push(symbol_at(
                            &child,
                            SymbolKind::Macro,
                            name.to_string(),
                            container.map(str::to_string),
                        ));
                    }
                }
                _ => {}
            }
        }
        collect(child, source, container, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
defmodule Scanner do
  def scan(x), do: x

  defp helper(y) do
    y + 1
  end

  defmacro traced(z), do: z
end
";

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_module_functions_and_macros() {
        let symbols = ElixirSymbols.outline(SAMPLE);
        assert_eq!(find(&symbols, "Scanner").unwrap().kind, SymbolKind::Module);
        assert_eq!(find(&symbols, "scan").unwrap().kind, SymbolKind::Function);
        assert_eq!(find(&symbols, "helper").unwrap().kind, SymbolKind::Function);
        assert_eq!(find(&symbols, "traced").unwrap().kind, SymbolKind::Macro);
    }

    #[test]
    fn function_carries_module_container() {
        let symbols = ElixirSymbols.outline(SAMPLE);
        let scan = find(&symbols, "scan").expect("scan function");
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        assert_eq!(scan.outline_label(), "fn Scanner::scan");
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
defmodule M do
  def target, do: 1
  # target in a comment
  def caller, do: target() + target()
end
";
        let refs = ElixirSymbols.references(source, "target");
        // definition head + two usages = 3 occurrences.
        assert_eq!(refs.len(), 3, "{refs:?}");
    }
}
