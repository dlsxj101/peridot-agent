//! C and C++ symbol extraction backed by `tree-sitter-c` / `tree-sitter-cpp`.
//!
//! C and C++ name their functions through nested *declarators* rather than a
//! simple `name` field, so [`declarator_name`] unwraps pointer/reference/
//! parenthesized declarators (following only the `declarator` field, never
//! `parameters`) to reach the identifier. C++ adds classes, namespaces, and
//! methods (named with `field_identifier`), handled via [`CFamilySymbols::cpp`].

use crate::{
    LanguageSymbols, LocalBinding, Reference, Symbol, SymbolKind, collect_references_by_kind,
    field_name, first_descendant_node, local_binding, nearest_ancestor, parse, symbol_at,
    walk_nodes,
};

/// C / C++ symbol extraction.
#[derive(Debug, Clone, Copy)]
pub struct CFamilySymbols {
    /// When true, use the C++ grammar (classes, namespaces, methods);
    /// otherwise the C grammar.
    cpp: bool,
}

impl CFamilySymbols {
    /// Extractor using the C grammar (`.c` / `.h`).
    pub fn c() -> Self {
        Self { cpp: false }
    }

    /// Extractor using the C++ grammar (`.cpp` / `.cc` / `.cxx` / `.hpp` / `.hh`).
    pub fn cpp() -> Self {
        Self { cpp: true }
    }

    fn language(&self) -> tree_sitter::Language {
        if self.cpp {
            tree_sitter_cpp::LANGUAGE.into()
        } else {
            tree_sitter_c::LANGUAGE.into()
        }
    }
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier" | "field_identifier" | "type_identifier" | "namespace_identifier"
    )
}

/// Resolves the name out of a (possibly nested) declarator, following only the
/// `declarator` field so function parameters are never mistaken for the name.
fn declarator_name<'a>(node: tree_sitter::Node, source: &'a str) -> Option<&'a str> {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" | "destructor_name"
        | "operator_name" => node.utf8_text(source.as_bytes()).ok(),
        // `Foo::bar` — the unqualified method name is the `name` field.
        "qualified_identifier" => node
            .child_by_field_name("name")
            .and_then(|n| declarator_name(n, source)),
        _ => node
            .child_by_field_name("declarator")
            .and_then(|d| declarator_name(d, source)),
    }
}

/// The name of a `function_definition`, dug out of its declarator.
fn function_name<'a>(func: &tree_sitter::Node, source: &'a str) -> Option<&'a str> {
    func.child_by_field_name("declarator")
        .and_then(|d| declarator_name(d, source))
}

impl LanguageSymbols for CFamilySymbols {
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
            // Function parameters: the binding name is the first identifier of
            // the declarator (skipping the type), governing the function body.
            "parameter_declaration" => {
                if let Some(token) = declarator_binding(&node, name, source)
                    && let Some(scope) = nearest_ancestor(node, &["function_definition"])
                {
                    bindings.push(local_binding(&token, &scope));
                }
            }
            // `int bar = …;` / `int bar;` inside a block governs that block.
            // File-scope declarations have no enclosing block and stay module
            // symbols.
            "declaration" => {
                let Some(scope) = nearest_ancestor(node, &["compound_statement"]) else {
                    return;
                };
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    let declarator = match child.kind() {
                        "init_declarator" => child.child_by_field_name("declarator"),
                        "identifier"
                        | "pointer_declarator"
                        | "array_declarator"
                        | "reference_declarator" => Some(child),
                        _ => None,
                    };
                    if let Some(declarator) = declarator
                        && let Some(token) = first_descendant_node(declarator, "identifier")
                        && token.utf8_text(source.as_bytes()) == Ok(name)
                    {
                        bindings.push(local_binding(&token, &scope));
                    }
                }
            }
            _ => {}
        });
        bindings
    }
}

/// The parameter's binding identifier (the first identifier reached through its
/// `declarator` field, skipping the type), if its name is `name`.
fn declarator_binding<'a>(
    parameter: &tree_sitter::Node<'a>,
    name: &str,
    source: &str,
) -> Option<tree_sitter::Node<'a>> {
    let declarator = parameter.child_by_field_name("declarator")?;
    let token = first_descendant_node(declarator, "identifier")?;
    (token.utf8_text(source.as_bytes()) == Ok(name)).then_some(token)
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
                if let Some(name) = function_name(&child, source) {
                    // A method body inside a class is a Method; elsewhere a function.
                    let kind = if container.is_some() {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    out.push(symbol_at(&child, kind, name.to_string(), container.clone()));
                }
            }
            "struct_specifier" | "union_specifier" => {
                if let Some(name) = type_name(&child, source) {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::Struct,
                        name.to_string(),
                        None,
                    ));
                    collect(child, source, Some(name.to_string()), out);
                }
            }
            "enum_specifier" => {
                if let Some(name) = type_name(&child, source) {
                    out.push(symbol_at(&child, SymbolKind::Enum, name.to_string(), None));
                }
            }
            "class_specifier" => {
                if let Some(name) = type_name(&child, source) {
                    out.push(symbol_at(&child, SymbolKind::Class, name.to_string(), None));
                    collect(child, source, Some(name.to_string()), out);
                }
            }
            "namespace_definition" => {
                let name = field_name(&child, source).map(str::to_string);
                if let Some(name) = name.clone() {
                    out.push(symbol_at(&child, SymbolKind::Module, name, None));
                }
                collect(child, source, container.clone(), out);
            }
            "type_definition" => {
                // typedef — the trailing declarator carries the alias name.
                if let Some(name) = child
                    .child_by_field_name("declarator")
                    .and_then(|d| declarator_name(d, source))
                {
                    out.push(symbol_at(
                        &child,
                        SymbolKind::TypeAlias,
                        name.to_string(),
                        None,
                    ));
                }
            }
            // declaration_list, field_declaration_list, translation_unit, ... — descend.
            _ => collect(child, source, container.clone(), out),
        }
    }
}

/// The `name` (type_identifier) of a struct/union/enum/class specifier.
fn type_name<'a>(node: &tree_sitter::Node, source: &'a str) -> Option<&'a str> {
    node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_extracts_functions_structs_enums_typedefs() {
        let source = "\
struct Point { int x; int y; };
enum Color { RED, GREEN };
typedef int MyInt;
int add(int a, int b) { return a + b; }
char *make_name(void) { return 0; }
";
        let symbols = CFamilySymbols::c().outline(source);
        let find = |n: &str| symbols.iter().find(|s| s.name == n);
        assert_eq!(find("Point").unwrap().kind, SymbolKind::Struct);
        assert_eq!(find("Color").unwrap().kind, SymbolKind::Enum);
        assert_eq!(find("MyInt").unwrap().kind, SymbolKind::TypeAlias);
        assert_eq!(find("add").unwrap().kind, SymbolKind::Function);
        // Pointer-returning function: declarator unwrapping still finds the name.
        assert_eq!(find("make_name").unwrap().kind, SymbolKind::Function);
    }

    #[test]
    fn cpp_extracts_classes_namespaces_and_methods() {
        let source = "\
namespace app {
class Scanner {
public:
    int scan() { return count; }
private:
    int count;
};
}
void freeFn() {}
";
        let symbols = CFamilySymbols::cpp().outline(source);
        let find = |n: &str| symbols.iter().find(|s| s.name == n);
        assert_eq!(find("app").unwrap().kind, SymbolKind::Module);
        assert_eq!(find("Scanner").unwrap().kind, SymbolKind::Class);
        let scan = find("scan").expect("scan method");
        assert_eq!(scan.kind, SymbolKind::Method);
        assert_eq!(scan.container.as_deref(), Some("Scanner"));
        assert_eq!(find("freeFn").unwrap().kind, SymbolKind::Function);
    }

    #[test]
    fn references_are_ast_aware() {
        let source = "\
int target(void) { return 0; }
/* target in a comment */
int caller(void) { return target(); }
";
        let refs = CFamilySymbols::c().references(source, "target");
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].line, 3);
    }
}
