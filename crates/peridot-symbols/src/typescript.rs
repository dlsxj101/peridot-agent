//! TypeScript / JavaScript symbol extraction backed by `tree-sitter-typescript`.
//!
//! The TSX grammar is a superset that also parses plain JavaScript and JSX, so
//! `.js` / `.jsx` / `.tsx` use it while `.ts` uses the stricter TypeScript
//! grammar (which rejects JSX `<tags>` as comparison operators).

use crate::{
    LanguageSymbols, LocalBinding, Reference, Symbol, SymbolKind, collect_references_by_kind,
    field_name, local_binding, nearest_ancestor, parse, symbol_at, walk_nodes,
};

/// Node kinds that introduce a value scope a parameter governs.
const TS_FUNCTION_SCOPES: &[&str] = &[
    "function_declaration",
    "generator_function_declaration",
    "function_expression",
    "generator_function",
    "arrow_function",
    "method_definition",
    "function_signature",
];

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
        "identifier"
            | "type_identifier"
            | "property_identifier"
            | "shorthand_property_identifier"
            // Shorthand binding in object destructuring patterns (`{ x }`), so
            // a destructured binding site shows up as an occurrence.
            | "shorthand_property_identifier_pattern"
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

    fn local_bindings(&self, source: &str, name: &str) -> Vec<LocalBinding> {
        let Some(tree) = parse(&self.language(), source) else {
            return Vec::new();
        };
        let mut bindings = Vec::new();
        walk_nodes(tree.root_node(), &mut |node| match node.kind() {
            // `function f(foo)`, method params, incl. destructuring like
            // `({a, b})` / `([c])`: a parameter binds for its whole function.
            "required_parameter" | "optional_parameter" => {
                if let Some(pattern) = node.child_by_field_name("pattern")
                    && let Some(scope) = nearest_ancestor(node, TS_FUNCTION_SCOPES)
                {
                    push_pattern(pattern, name, source, &scope, &mut bindings);
                }
            }
            // Bare single arrow param `foo => …`.
            "arrow_function" => {
                if let Some(param) = node.child_by_field_name("parameter")
                    && param.kind() == "identifier"
                    && param.utf8_text(source.as_bytes()) == Ok(name)
                {
                    bindings.push(local_binding(&param, &node));
                }
            }
            // `let`/`const`/`var foo = …`, incl. `const {x} = …` / `const [y]
            // = …`, governs the rest of its block; module-level declarations
            // (no block) are module symbols.
            "variable_declarator" => {
                if let Some(target) = node.child_by_field_name("name")
                    && let Some(scope) = nearest_ancestor(node, &["statement_block"])
                {
                    push_pattern(target, name, source, &scope, &mut bindings);
                }
            }
            // `for (const d of …)` / `for (const k in …)` bind for the loop body.
            "for_in_statement" => {
                if let Some(target) = node.child_by_field_name("left") {
                    push_pattern(target, name, source, &node, &mut bindings);
                }
            }
            // `catch (e)` binds for the catch body.
            "catch_clause" => {
                if let Some(param) = node.child_by_field_name("parameter") {
                    push_pattern(param, name, source, &node, &mut bindings);
                }
            }
            _ => {}
        });
        bindings
    }
}

/// Binding-leaf node kinds inside a TypeScript / JavaScript binding pattern.
const TS_PATTERN_BINDINGS: &[&str] = &["identifier", "shorthand_property_identifier_pattern"];

/// Collects every binding of `name` in `pattern` (handling object / array
/// destructuring, skipping type annotations and default-value expressions) and
/// pushes a [`LocalBinding`] governed by `scope` for each.
fn push_pattern(
    pattern: tree_sitter::Node,
    name: &str,
    source: &str,
    scope: &tree_sitter::Node,
    out: &mut Vec<LocalBinding>,
) {
    let mut tokens = Vec::new();
    crate::collect_pattern_idents(
        pattern,
        name,
        source,
        TS_PATTERN_BINDINGS,
        // Skip type annotations and `= default` values inside patterns.
        &["type", "value", "right"],
        &mut tokens,
    );
    for token in tokens {
        out.push(local_binding(&token, scope));
    }
}

/// Whether a declarator's initializer is a function value (arrow or function
/// expression), so `const f = () => {}` is recorded as a function.
fn is_function_value(kind: Option<&str>) -> bool {
    matches!(
        kind,
        Some("arrow_function" | "function" | "function_expression" | "generator_function")
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
            // Class fields holding a function value (`handler = () => {}` /
            // `run = function() {}`) are methods of the enclosing class.
            "public_field_definition" | "field_definition" => {
                let value_kind = child.child_by_field_name("value").map(|n| n.kind());
                if is_function_value(value_kind)
                    && let Some(name) = field_name(&child, source)
                {
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
    fn generator_function_expression_const_is_a_function() {
        let symbols = ts().outline("const g = function*(){};");
        assert_eq!(find(&symbols, "g").unwrap().kind, SymbolKind::Function);
    }

    #[test]
    fn arrow_class_field_is_a_method() {
        let source = "\
class Server {
    handler = () => {};
}
";
        let symbols = ts().outline(source);
        let handler = find(&symbols, "handler").expect("handler field");
        assert_eq!(handler.kind, SymbolKind::Method);
        assert_eq!(handler.container.as_deref(), Some("Server"));
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
