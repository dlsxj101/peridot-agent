//! Slash-command help / catalog RPC results for the daemon.
//!
//! Stateless helpers that turn the shared [`peridot_tui`] slash-command catalog
//! into the JSON the VS Code surface renders for `command.catalog` and
//! `command help` requests — optionally filtered to a named surface. Split out
//! of the daemon `mod.rs` so the catalog-shaping lives in one place; the
//! request dispatcher calls these.

use serde_json::Value;

/// The optional `surface` filter from request params (a non-empty string).
pub(crate) fn command_catalog_surface(params: Option<&Value>) -> Option<&str> {
    params
        .and_then(Value::as_object)
        .and_then(|object| object.get("surface"))
        .and_then(Value::as_str)
        .filter(|surface| !surface.trim().is_empty())
}

/// The full slash-command catalog as a `{ commands: [...] }` value, optionally
/// filtered to commands available on `surface`.
pub(crate) fn slash_command_catalog_result(surface: Option<&str>) -> Value {
    let commands: Vec<Value> = peridot_tui::slash_command_catalog()
        .iter()
        .filter(|spec| {
            surface
                .is_none_or(|surface| peridot_tui::slash_command_surfaces(spec).contains(&surface))
        })
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "description": spec.description,
                "arg_hint": spec.arg_hint,
                "category": spec.category,
                "surfaces": peridot_tui::slash_command_surfaces(spec),
                "arg_options": peridot_tui::slash_command_arg_options(spec),
            })
        })
        .collect();
    serde_json::json!({ "commands": commands })
}

/// A `help`-kind command result listing the available slash commands (filtered
/// to `surface` when given), echoing the raw command that triggered it.
pub(crate) fn handle_command_help(raw_command: &str, surface: Option<&str>) -> Value {
    let items = slash_help_items(surface);
    let total = items.len();
    let mut result = serde_json::json!({
        "kind": "help",
        "title": "Slash Commands",
        "message": format!("{total} slash command(s) available"),
        "severity": "info",
        "command": raw_command,
        "items": items,
        "total": total,
    });
    if let Some(surface) = surface {
        result["surface"] = Value::String(surface.to_string());
    }
    result
}

fn slash_help_items(surface: Option<&str>) -> Vec<Value> {
    peridot_tui::slash_command_catalog()
        .iter()
        .filter(|spec| {
            surface
                .is_none_or(|surface| peridot_tui::slash_command_surfaces(spec).contains(&surface))
        })
        .map(|spec| {
            let label = match spec.arg_hint {
                Some(hint) => format!("{} {}", spec.name, hint),
                None => spec.name.to_string(),
            };
            serde_json::json!({
                "label": label,
                "detail": spec.description,
                "source": spec.category,
            })
        })
        .collect()
}
