//! Lua symbol extraction backed by `tree-sitter-lua`.
//!
//! Lua only names functions (free, table-field `M.foo`, and method `M:foo`
//! forms); the `name` field text is used verbatim, so dotted/colon names are
//! preserved as written.

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// Lua symbol extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct LuaSymbols;

fn language() -> tree_sitter::Language {
    tree_sitter_lua::LANGUAGE.into()
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier")
}

impl LanguageSymbols for LuaSymbols {
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
        if child.kind() == "function_declaration"
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
    fn extracts_function_declarations() {
        let source = "\
local function helper(x)
    return x + 1
end

function Scanner.scan(self)
    return self.count
end
";
        let symbols = LuaSymbols.outline(source);
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "helper" && s.kind == SymbolKind::Function),
            "{symbols:?}"
        );
        // Table-field functions keep their dotted name.
        assert!(
            symbols.iter().any(|s| s.name == "Scanner.scan"),
            "{symbols:?}"
        );
    }

    #[test]
    fn references_find_function_uses() {
        let source = "\
local function target() end
-- target in a comment
target()
";
        let refs = LuaSymbols.references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 3);
    }
}
