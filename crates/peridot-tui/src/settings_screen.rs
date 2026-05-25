//! Reusable interactive settings screen.
//!
//! The operator invokes `peridot setting` and lands on a single-screen
//! list of toggleable / cycleable / numeric options. The screen owns
//! no opinion about *what* the settings are — peridot-cli builds the
//! [`SettingItem`] list from `PeridotConfig`, hands it to
//! [`run_settings_screen`], then reads the mutated list back to write
//! the new values to disk.
//!
//! Keys:
//! - `↑` / `↓`              navigate between items
//! - `Space`, `Enter`       toggle bool / cycle choice forward / step number up
//! - `←` / `→`              step number / cycle choice backward / forward
//! - `s`                    save and exit (returns [`SettingsOutcome::Save`])
//! - `q` / `Esc`            cancel and exit (returns [`SettingsOutcome::Cancel`])

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use serde::{Deserialize, Serialize};

use crate::terminal::TerminalGuard;

/// One row in the settings screen.
///
/// The struct is `Serialize`/`Deserialize` so the daemon can ship the same
/// item list across JSON-RPC to non-TUI clients (VS Code webview, future
/// REST shim, …) without duplicating the schema. The TUI renders these
/// rows directly; the wire format and the in-memory form are intentionally
/// the same type to keep them in lock-step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingItem {
    /// Stable identifier (e.g. `"defaults.auto_verify_after_mutation"`).
    /// Echoed verbatim back to the caller after save so the caller can
    /// route mutations to the right config field.
    pub id: String,
    /// Group header rendered above the first item that carries it.
    /// Items sharing a group cluster under that header.
    pub group: String,
    /// Short human label shown on the row.
    pub label: String,
    /// Optional one-line help text rendered in the bottom panel when
    /// the row is focused.
    pub help: Option<String>,
    /// Current value.
    pub value: SettingValue,
    /// Which surfaces should display this item. Lets the VS Code
    /// webview filter out TUI-only knobs (`show_mascot`, `show_thinking`,
    /// …) that would do nothing if toggled there. Defaults to
    /// `["tui", "vscode"]` via [`SettingItem::default_surfaces`] when
    /// the producer doesn't care. Unknown surfaces are ignored by
    /// renderers — adding a new surface (e.g. `"web"`) is additive.
    #[serde(default = "SettingItem::default_surfaces")]
    pub surfaces: Vec<String>,
}

impl SettingItem {
    /// Default surface list — the item is rendered everywhere.
    /// Centralised so the registry doesn't litter literal vec
    /// constructions and so the serde `default` callback has a
    /// stable target.
    pub fn default_surfaces() -> Vec<String> {
        vec!["tui".to_string(), "vscode".to_string()]
    }
}

/// Value held by a [`SettingItem`].
///
/// JSON shape uses adjacently-tagged enum representation
/// (`{"kind": "Bool", "data": true}`) for an unambiguous mapping that
/// TypeScript / Python clients can pattern-match on. The TUI never reads
/// the JSON form — the same `enum` is the in-memory state — but keeping
/// one definition prevents wire-format drift between TUI and webview.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum SettingValue {
    /// On/off toggle.
    Bool(bool),
    /// One of a fixed set of options.
    Choice {
        /// Display labels for each option, in stable order.
        options: Vec<String>,
        /// Index into `options`.
        selected: usize,
    },
    /// 32-bit unsigned integer with bounds and step.
    U32 {
        /// Current value.
        value: u32,
        /// Inclusive lower bound.
        min: u32,
        /// Inclusive upper bound.
        max: u32,
        /// Step size per left/right press.
        step: u32,
    },
    /// 64-bit float with bounds and step.
    F64 {
        /// Current value.
        value: f64,
        /// Inclusive lower bound.
        min: f64,
        /// Inclusive upper bound.
        max: f64,
        /// Step size per left/right press.
        step: f64,
    },
    /// Pointer-sized unsigned with bounds and step.
    Usize {
        /// Current value.
        value: usize,
        /// Inclusive lower bound.
        min: usize,
        /// Inclusive upper bound.
        max: usize,
        /// Step size per left/right press.
        step: usize,
    },
}

impl SettingValue {
    /// Renders the value as the bracketed right-hand text on a row.
    pub fn display(&self) -> String {
        match self {
            SettingValue::Bool(true) => "  on  ".to_string(),
            SettingValue::Bool(false) => " off  ".to_string(),
            SettingValue::Choice { options, selected } => {
                options.get(*selected).cloned().unwrap_or_default()
            }
            SettingValue::U32 { value, .. } => format!("{value}"),
            SettingValue::F64 { value, .. } => format!("{value:.2}"),
            SettingValue::Usize { value, .. } => format!("{value}"),
        }
    }

    fn step_up(&mut self) {
        match self {
            SettingValue::Bool(v) => *v = !*v,
            SettingValue::Choice { options, selected } => {
                if !options.is_empty() {
                    *selected = (*selected + 1) % options.len();
                }
            }
            SettingValue::U32 {
                value, max, step, ..
            } => {
                let next = value.saturating_add(*step);
                *value = next.min(*max);
            }
            SettingValue::F64 {
                value, max, step, ..
            } => {
                *value = (*value + *step).min(*max);
            }
            SettingValue::Usize {
                value, max, step, ..
            } => {
                let next = value.saturating_add(*step);
                *value = next.min(*max);
            }
        }
    }

    fn step_down(&mut self) {
        match self {
            SettingValue::Bool(v) => *v = !*v,
            SettingValue::Choice { options, selected } => {
                if !options.is_empty() {
                    *selected = if *selected == 0 {
                        options.len() - 1
                    } else {
                        *selected - 1
                    };
                }
            }
            SettingValue::U32 {
                value, min, step, ..
            } => {
                let next = value.saturating_sub(*step);
                *value = next.max(*min);
            }
            SettingValue::F64 {
                value, min, step, ..
            } => {
                *value = (*value - *step).max(*min);
            }
            SettingValue::Usize {
                value, min, step, ..
            } => {
                let next = value.saturating_sub(*step);
                *value = next.max(*min);
            }
        }
    }
}

/// Outcome of the settings screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsOutcome {
    /// Operator pressed `s`. Caller should persist the mutated items.
    Save,
    /// Operator pressed `q` / `Esc`. Caller should discard mutations.
    Cancel,
}

/// Runs the interactive settings screen until the operator saves or
/// cancels. The `items` slice is mutated in place: on `Save` the
/// caller reads the updated values back; on `Cancel` the in-memory
/// mutations should be discarded (the items reflect the last
/// keystroke, not what the operator committed to save).
pub fn run_settings_screen(items: &mut [SettingItem]) -> io::Result<SettingsOutcome> {
    let mut terminal_guard = TerminalGuard::enter()?;
    let outcome = run_loop(&mut terminal_guard.terminal, items);
    drop(terminal_guard);
    outcome
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    items: &mut [SettingItem],
) -> io::Result<SettingsOutcome> {
    let mut focused = 0usize;
    loop {
        terminal.draw(|frame| draw_settings(frame, items, focused))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
        {
            match handle_key(key, items, &mut focused) {
                KeyOutcome::Continue => {}
                KeyOutcome::Save => return Ok(SettingsOutcome::Save),
                KeyOutcome::Cancel => return Ok(SettingsOutcome::Cancel),
            }
        }
    }
}

enum KeyOutcome {
    Continue,
    Save,
    Cancel,
}

fn handle_key(key: KeyEvent, items: &mut [SettingItem], focused: &mut usize) -> KeyOutcome {
    if items.is_empty() {
        return match key.code {
            KeyCode::Char('q') | KeyCode::Esc => KeyOutcome::Cancel,
            _ => KeyOutcome::Continue,
        };
    }
    match key.code {
        KeyCode::Up => {
            *focused = if *focused == 0 {
                items.len() - 1
            } else {
                *focused - 1
            };
        }
        KeyCode::Down => {
            *focused = (*focused + 1) % items.len();
        }
        KeyCode::Right | KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Char('l') => {
            if let Some(item) = items.get_mut(*focused) {
                item.value.step_up();
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if let Some(item) = items.get_mut(*focused) {
                item.value.step_down();
            }
        }
        KeyCode::Char('s') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            return KeyOutcome::Save;
        }
        KeyCode::Char('q') | KeyCode::Esc => return KeyOutcome::Cancel,
        _ => {}
    }
    KeyOutcome::Continue
}

fn draw_settings(frame: &mut ratatui::Frame<'_>, items: &[SettingItem], focused: usize) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(4),
        ])
        .split(area);
    draw_header(frame, chunks[0]);
    draw_list(frame, chunks[1], items, focused);
    draw_footer(frame, chunks[2], items, focused);
}

fn draw_header(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" peridot settings ");
    let body = Paragraph::new(Line::from(vec![Span::styled(
        "Toggle and cycle Peridot's runtime options. Saves to .peridot/config.toml.",
        Style::default().fg(Color::Gray),
    )]))
    .block(block);
    frame.render_widget(body, area);
}

fn draw_list(frame: &mut ratatui::Frame<'_>, area: Rect, items: &[SettingItem], focused: usize) {
    let value_col_width: usize = 14;
    let mut last_group: Option<&str> = None;
    let mut list_items: Vec<ListItem> = Vec::with_capacity(items.len() * 2);
    for (idx, item) in items.iter().enumerate() {
        let group = item.group.as_str();
        if Some(group) != last_group {
            list_items.push(ListItem::new(Line::from(vec![Span::styled(
                format!(" {group} "),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )])));
            last_group = Some(group);
        }
        let prefix = if idx == focused { "▶ " } else { "  " };
        let label_style = if idx == focused {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let value_text = item.value.display();
        let padded_value = format!("{value_text:>width$}", width = value_col_width);
        let value_style = match &item.value {
            SettingValue::Bool(true) => Style::default().fg(Color::Green),
            SettingValue::Bool(false) => Style::default().fg(Color::DarkGray),
            _ => Style::default().fg(Color::Cyan),
        };
        list_items.push(ListItem::new(Line::from(vec![
            Span::styled(prefix.to_string(), label_style),
            Span::styled(item.label.clone(), label_style),
            Span::raw("  "),
            Span::styled(format!("[{padded_value}]"), value_style),
        ])));
    }
    let block = Block::default().borders(Borders::ALL);
    let list = List::new(list_items).block(block);
    frame.render_widget(list, area);
}

fn draw_footer(frame: &mut ratatui::Frame<'_>, area: Rect, items: &[SettingItem], focused: usize) {
    let help_line = items
        .get(focused)
        .and_then(|item| item.help.clone())
        .unwrap_or_else(|| "Use ↑/↓ to navigate.".to_string());
    let key_hint = "  ↑/↓ navigate  •  ←/→ / Space change  •  s save  •  q quit";
    let body = Paragraph::new(vec![
        Line::from(Span::styled(help_line, Style::default().fg(Color::Gray))),
        Line::from(Span::styled(
            key_hint.to_string(),
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .wrap(Wrap { trim: true })
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(body, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bool_item(id: &str, value: bool) -> SettingItem {
        SettingItem {
            id: id.to_string(),
            group: "Group".to_string(),
            label: id.to_string(),
            help: None,
            value: SettingValue::Bool(value),
            surfaces: SettingItem::default_surfaces(),
        }
    }

    #[test]
    fn bool_step_up_toggles() {
        let mut v = SettingValue::Bool(false);
        v.step_up();
        assert_eq!(v, SettingValue::Bool(true));
        v.step_up();
        assert_eq!(v, SettingValue::Bool(false));
    }

    #[test]
    fn choice_step_up_wraps() {
        let mut v = SettingValue::Choice {
            options: vec!["a".into(), "b".into(), "c".into()],
            selected: 2,
        };
        v.step_up();
        if let SettingValue::Choice { selected, .. } = v {
            assert_eq!(selected, 0, "wrap to first after last");
        } else {
            panic!("kind changed unexpectedly");
        }
    }

    #[test]
    fn choice_step_down_wraps_from_zero() {
        let mut v = SettingValue::Choice {
            options: vec!["a".into(), "b".into(), "c".into()],
            selected: 0,
        };
        v.step_down();
        if let SettingValue::Choice { selected, .. } = v {
            assert_eq!(selected, 2);
        } else {
            panic!("kind changed unexpectedly");
        }
    }

    #[test]
    fn u32_clamps_to_bounds() {
        let mut v = SettingValue::U32 {
            value: 99,
            min: 1,
            max: 100,
            step: 10,
        };
        v.step_up();
        if let SettingValue::U32 { value, .. } = v {
            assert_eq!(value, 100, "clamp to max");
        } else {
            panic!();
        }
        let mut v = SettingValue::U32 {
            value: 5,
            min: 1,
            max: 100,
            step: 10,
        };
        v.step_down();
        if let SettingValue::U32 { value, .. } = v {
            assert_eq!(value, 1, "clamp to min");
        } else {
            panic!();
        }
    }

    #[test]
    fn handle_key_navigates_and_toggles() {
        let mut items = vec![bool_item("a", false), bool_item("b", true)];
        let mut focused = 0usize;
        // Down → focused = 1
        let _ = handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut items,
            &mut focused,
        );
        assert_eq!(focused, 1);
        // Space toggles b: true → false
        let _ = handle_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut items,
            &mut focused,
        );
        assert_eq!(items[1].value, SettingValue::Bool(false));
        // Up wraps to 0
        let _ = handle_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &mut items,
            &mut focused,
        );
        assert_eq!(focused, 0);
    }

    #[test]
    fn save_and_cancel_keys_return_outcomes() {
        let mut items = vec![bool_item("a", false)];
        let mut focused = 0usize;
        match handle_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            &mut items,
            &mut focused,
        ) {
            KeyOutcome::Save => {}
            _ => panic!("'s' should save"),
        }
        match handle_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &mut items,
            &mut focused,
        ) {
            KeyOutcome::Cancel => {}
            _ => panic!("'q' should cancel"),
        }
        match handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut items,
            &mut focused,
        ) {
            KeyOutcome::Cancel => {}
            _ => panic!("Esc should cancel"),
        }
    }
}
