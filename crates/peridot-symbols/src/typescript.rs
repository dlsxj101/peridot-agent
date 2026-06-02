//! TypeScript / JavaScript symbol extraction backed by `tree-sitter-typescript`.
//!
//! The TSX grammar is a superset that also parses plain JavaScript and JSX, so
//! `.js` / `.jsx` / `.tsx` use it while `.ts` uses the stricter TypeScript
//! grammar (which rejects JSX `<tags>` as comparison operators).

use crate::{
    LanguageSymbols, Reference, Symbol, SymbolKind, collect_references_by_kind, field_name, parse,
    symbol_at,
};

/// TypeScript / JavaScript symbol extraction.
#[derive(Debug, Clone, Copy)]
pub struct TypeScriptSymbols {
    /// When true, use the TSX grammar (also parses JS/JSX); otherwise the
    /// stricter TypeScript grammar.
    tsx: bool,
}

impl TypeScriptSymbols {
    /// Extractor using the TypeScript grammar (`.ts` / `.mts` / `.cts`).
    pub fn typescript() -> Self {
        Self { tsx: false }
    }

    /// Extractor using the TSX grammar (`.tsx` / `.js` / `.jsx` / `.mjs` / `.cjs`).
    pub fn tsx() -> Self {
        Self { tsx: true }
    }

    fn language(&self) -> tree_sitter::Language {
        if self.tsx {
            tree_sitter_typescript::LANGUAGE_TSX.into()
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
        }
    }
}

/// Maps a tree-sitter TypeScript node kind to a [`SymbolKind`] for the simple
/// (named-by-`name`-field) declarations. Functions assigned to variables and
/// class methods are handled separately in [`collect`].
fn node_kind(node_kind: &str) -> Option<SymbolKind> {
    Some(match node_kind {
        "function_declaration" | "generator_function_declaration" => SymbolKind::Function,
        "class_declaration" | "abstract_class_declaration" => SymbolKind::Class,
        "interface_declaration" => SymbolKind::Interface,
        "type_alias_declaration" => SymbolKind::TypeAlias,
        "enum_declaration" => SymbolKind::Enum,
        "internal_module" | "module" => SymbolKind::Module,
        _ => return None,
    })
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier" | "type_identifier" | "property_identifier" | "shorthand_property_identifier"
    )
}

impl LanguageSymbols for TypeScriptSymbols {
    fn outline(&self, source: &str) -> Vec<Symbol> {
        let Some(tree) = parse(&self.language(), source) else {
            return Vec::new();
        };
        let mut symbols = Vec::new();
        collect(tree.root_node(), source, None, &mut symbols);
        symbols
    }

    fn references(&self, source: &str, name: &str) -> Vec<Reference> {
        let Some(tree) = parse(&self.language(), source) else {
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

/// Whether a declarator's initializer is a function value (arrow or function
/// expression), so `const f = () => {}` is recorded as a function.
fn is_function_value(kind: Option<&str>) -> bool {
    matches!(
        kind,
        Some("arrow_function" | "function" | "function_expression")
    )
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
            // Class methods inside a class body, with the class as container.
            "method_definition" => {
                if let Some(name) = field_name(&child, source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Method,
                        name.to_string(),
                        container.clone(),
                    ));
                }
            }
            // `const f = () => {}` / `let g = function() {}` at any depth.
            "lexical_declaration" | "variable_declaration" => {
                let mut decl_cursor = child.walk();
                for declarator in child.children(&mut decl_cursor) {
                    if declarator.kind() != "variable_declarator" {
                        continue;
                    }
                    let value_kind = declarator.child_by_field_name("value").map(|n| n.kind());
                    if !is_function_value(value_kind) {
                        continue;
                    }
                    if let Some(name) = field_name(&declarator, source) {
                        out.push(symbol_at(
                            &declarator,
                            SymbolKind::Function,
                            name.to_string(),
                            container.clone(),
                        ));
                    }
                }
            }
            other => {
                if let Some(kind) = node_kind(other) {
                    let name = field_name(&child, source).map(str::to_string);
                    // Class/module bodies thread their name down as container.
                    let child_container = if matches!(kind, SymbolKind::Class | SymbolKind::Module)
                    {
                        name.clone()
                    } else {
                        container.clone()
                    };
                    if let Some(name) = name {
                        out.push(symbol_at(&child, kind, name, container.clone()));
                    }
                    collect(child, source, child_container, out);
                } else {
                    // Wrapper nodes (export_statement, program, class_body, ...).
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
export function renderApp(): void {}

export const helper = (x: number) => x + 1;

export class AppShell {
    private count = 0;
    start(): void {}
    async stop(): Promise<void> {}
}

interface Options {
    debug: boolean;
}

type Id = string;

enum Color {
    Red,
    Green,
}
"#;

    fn ts() -> TypeScriptSymbols {
        TypeScriptSymbols::typescript()
    }

    fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_top_level_declarations() {
        let symbols = ts().outline(SAMPLE);
        assert_eq!(
            find(&symbols, "renderApp").unwrap().kind,
            SymbolKind::Function
        );
        assert_eq!(find(&symbols, "AppShell").unwrap().kind, SymbolKind::Class);
        assert_eq!(
            find(&symbols, "Options").unwrap().kind,
            SymbolKind::Interface
        );
        assert_eq!(find(&symbols, "Id").unwrap().kind, SymbolKind::TypeAlias);
        assert_eq!(find(&symbols, "Color").unwrap().kind, SymbolKind::Enum);
    }

    #[test]
    fn arrow_function_const_is_a_function() {
        let symbols = ts().outline(SAMPLE);
        assert_eq!(find(&symbols, "helper").unwrap().kind, SymbolKind::Function);
    }

    #[test]
    fn class_methods_carry_their_class_as_container() {
        let symbols = ts().outline(SAMPLE);
        let start = find(&symbols, "start").expect("start method");
        assert_eq!(start.kind, SymbolKind::Method);
        assert_eq!(start.container.as_deref(), Some("AppShell"));
        let stop = find(&symbols, "stop").expect("stop method");
        assert_eq!(stop.container.as_deref(), Some("AppShell"));
        assert_eq!(start.outline_label(), "method AppShell::start");
    }

    #[test]
    fn tsx_grammar_parses_jsx() {
        let source = "export function App() { return <div>hi</div>; }";
        let symbols = TypeScriptSymbols::tsx().outline(source);
        assert!(symbols.iter().any(|s| s.name == "App"));
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
function target() {}
// target in a comment
const s = \"target in a string\";
target();
";
        let refs = TypeScriptSymbols::typescript().references(source, "target");
        // definition (line 1) + call (line 4); comment and string excluded.
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 4);
    }
}
