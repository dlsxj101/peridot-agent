//! Python symbol extraction backed by `tree-sitter-python`.

use crate::{
    LanguageSymbols, LocalBinding, Reference, Symbol, SymbolKind, collect_references_by_kind,
    field_name, first_descendant_node, local_binding, nearest_ancestor, parse, symbol_at,
    walk_nodes,
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

    fn local_bindings(&self, source: &str, name: &str) -> Vec<LocalBinding> {
        let Some(tree) = parse(&language(), source) else {
            return Vec::new();
        };
        let mut bindings = Vec::new();
        walk_nodes(tree.root_node(), &mut |node| match node.kind() {
            // Parameters bind for the whole function body.
            "function_definition" => {
                let Some(params) = node.child_by_field_name("parameters") else {
                    return;
                };
                let mut cursor = params.walk();
                for param in params.children(&mut cursor) {
                    if let Some(token) = param_name(param, name, source) {
                        bindings.push(local_binding(&token, &node));
                    }
                }
            }
            // An assignment, a `for` target, and a walrus (`:=`) all make the
            // name function-local for the entire enclosing function;
            // module-level binders are treated as module symbols, not locals.
            "assignment" => {
                if let Some(target) = node.child_by_field_name("left") {
                    push_function_local(target, name, source, &mut bindings);
                }
            }
            "for_statement" => {
                if let Some(target) = node.child_by_field_name("left") {
                    push_function_local(target, name, source, &mut bindings);
                }
            }
            "named_expression" => {
                if let Some(target) = node.child_by_field_name("name") {
                    push_function_local(target, name, source, &mut bindings);
                }
            }
            // `with … as w` / `except … as e`: the alias binds for the closest
            // `except` handler, else for the enclosing function.
            "as_pattern_target" => {
                if let Some(token) = first_descendant_node(node, "identifier")
                    && token.utf8_text(source.as_bytes()) == Ok(name)
                {
                    if let Some(handler) = nearest_ancestor(node, &["except_clause"]) {
                        bindings.push(local_binding(&token, &handler));
                    } else {
                        push_function_local(token, name, source, &mut bindings);
                    }
                }
            }
            // Comprehension variables are scoped to the comprehension itself.
            "for_in_clause" => {
                if let Some(target) = node.child_by_field_name("left")
                    && let Some(scope) = nearest_ancestor(
                        node,
                        &[
                            "list_comprehension",
                            "set_comprehension",
                            "dictionary_comprehension",
                            "generator_expression",
                        ],
                    )
                {
                    let mut tokens = Vec::new();
                    collect_target_idents(target, name, source, &mut tokens);
                    for token in tokens {
                        bindings.push(local_binding(&token, &scope));
                    }
                }
            }
            _ => {}
        });
        bindings
    }
}

/// Collects the `identifier` binding tokens named `name` in an assignment / for
/// / comprehension target, recursing through tuple and list targets but never
/// into `attribute` (`obj.x`) or `subscript` (`a[i]`) targets, which rebind an
/// existing object rather than introduce a new name.
fn collect_target_idents<'a>(
    node: tree_sitter::Node<'a>,
    name: &str,
    source: &str,
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    match node.kind() {
        "attribute" | "subscript" | "call" => return,
        "identifier" => {
            if node.utf8_text(source.as_bytes()) == Ok(name) {
                out.push(node);
            }
            return;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_target_idents(child, name, source, out);
    }
}

/// Pushes a function-local binding for every `identifier` named `name` in
/// `target` (handling tuple targets like `for a, b in …`). The local governs
/// the whole enclosing function — Python names are function-scoped — so any use
/// textually before the binder still resolves to it. Module-level binders (no
/// enclosing function) are left as module symbols.
fn push_function_local(
    target: tree_sitter::Node,
    name: &str,
    source: &str,
    out: &mut Vec<LocalBinding>,
) {
    let Some(scope) = nearest_ancestor(target, &["function_definition"]) else {
        return;
    };
    let mut tokens = Vec::new();
    collect_target_idents(target, name, source, &mut tokens);
    for token in tokens {
        let mut binding = local_binding(&token, &scope);
        binding.scope_start_line = scope.start_position().row + 1;
        out.push(binding);
    }
}

/// The binding identifier of a parameter node whose name is `name`, covering
/// simple, typed, and default parameters. Returns the name token so its
/// declaration site can be matched precisely.
fn param_name<'a>(
    param: tree_sitter::Node<'a>,
    name: &str,
    source: &str,
) -> Option<tree_sitter::Node<'a>> {
    let token = if param.kind() == "identifier" {
        param
    } else {
        // typed / default / splat parameters carry the name as their first
        // identifier descendant (the name field precedes any default value).
        first_descendant_node(param, "identifier")?
    };
    (token.utf8_text(source.as_bytes()) == Ok(name)).then_some(token)
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
