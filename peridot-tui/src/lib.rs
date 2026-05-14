//! Terminal UI state and rendering boundary.

use std::fmt::Write as FmtWrite;
use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        size as terminal_size,
    },
};
use peridot_common::{AskUserRequest, ExecutionMode, PermissionMode};
use peridot_core::{SlashCommand, parse_slash_command};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use serde::{Deserialize, Serialize};

/// TUI layout mode selected from terminal size.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    /// Header, main panel, side panel, and input.
    Full,
    /// Header, main panel, and input.
    Compact,
    /// Minimal transcript plus input.
    Minimal,
}

/// Header state shown at the top of the TUI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeaderState {
    /// Active execution mode.
    pub mode: ExecutionMode,
    /// Active permission mode.
    pub permission: PermissionMode,
    /// Active model name.
    pub model: String,
    /// Estimated cost in USD.
    pub cost_usd: f64,
}

impl HeaderState {
    /// Creates a new header state.
    pub fn new(mode: ExecutionMode, permission: PermissionMode, model: impl Into<String>) -> Self {
        Self {
            mode,
            permission,
            model: model.into(),
            cost_usd: 0.0,
        }
    }
}

/// One plan item shown in the side panel.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step label.
    pub label: String,
    /// Whether the step has completed.
    pub done: bool,
}

/// Session statistics shown in the side panel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionStats {
    /// Completed tool/model steps.
    pub steps: u32,
    /// Recoverable error count.
    pub errors: u32,
    /// Elapsed seconds.
    pub elapsed_seconds: u64,
}

/// Right-side panel state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SidePanelState {
    /// Current plan steps.
    pub plan: Vec<PlanStep>,
    /// Session statistics.
    pub stats: SessionStats,
}

/// Interactive ask-user prompt shown as a special TUI screen.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AskUserPanel {
    /// Question text.
    pub question: String,
    /// Selectable choices.
    pub choices: Vec<String>,
    /// Currently highlighted choice.
    pub selected_index: usize,
    /// Free-form fallback text.
    pub freeform: String,
}

impl AskUserPanel {
    /// Builds a panel from an ask-user request.
    pub fn from_request(request: AskUserRequest) -> Self {
        match request {
            AskUserRequest::SingleSelect {
                question,
                options,
                default_index,
            } => Self {
                question,
                choices: options,
                selected_index: default_index.unwrap_or(0),
                freeform: String::new(),
            },
            AskUserRequest::MultiSelect {
                question, options, ..
            } => Self {
                question,
                choices: options,
                selected_index: 0,
                freeform: String::new(),
            },
            AskUserRequest::FreeForm {
                question, default, ..
            } => Self {
                question,
                choices: Vec::new(),
                selected_index: 0,
                freeform: default.unwrap_or_default(),
            },
        }
    }

    fn selected_answer(&self) -> String {
        self.choices
            .get(self.selected_index)
            .cloned()
            .unwrap_or_else(|| self.freeform.clone())
    }
}

/// Main TUI state independent from the terminal backend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TuiState {
    /// Current layout mode.
    pub layout: LayoutMode,
    /// Header state.
    pub header: HeaderState,
    /// Transcript lines.
    pub transcript: Vec<String>,
    /// Side panel state.
    pub side_panel: SidePanelState,
    /// Current input buffer.
    pub input: String,
    /// Active ask-user panel, when the agent is waiting for user guidance.
    pub ask_user: Option<AskUserPanel>,
}

/// Result produced when an interactive TUI session exits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TuiExit {
    /// Final TUI state.
    pub state: TuiState,
    /// Submitted task, when the user pressed Enter on non-command input.
    pub submitted: Option<String>,
}

/// Outcome of handling one terminal input event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TuiEventOutcome {
    /// Keep rendering the current TUI session.
    Continue,
    /// Exit without submitting a task.
    Quit,
    /// Exit and submit the contained task text.
    Submit(String),
}

impl TuiState {
    /// Creates a new TUI state.
    pub fn new(header: HeaderState) -> Self {
        Self {
            layout: LayoutMode::Full,
            header,
            transcript: Vec::new(),
            side_panel: SidePanelState::default(),
            input: String::new(),
            ask_user: None,
        }
    }

    /// Selects a layout mode from terminal dimensions.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.layout = select_layout(width, height);
    }

    /// Appends a transcript line.
    pub fn push_transcript(&mut self, line: impl Into<String>) {
        self.transcript.push(line.into());
    }

    /// Parses the current input as a slash command when possible.
    pub fn current_slash_command(&self) -> Option<SlashCommand> {
        parse_slash_command(&self.input)
    }

    /// Opens an ask-user panel.
    pub fn open_ask_user(&mut self, request: AskUserRequest) {
        self.ask_user = Some(AskUserPanel::from_request(request));
    }
}

/// Runs the interactive terminal UI until the user quits or submits a task.
pub fn run_interactive(mut state: TuiState) -> io::Result<TuiExit> {
    let mut terminal = TerminalGuard::enter()?;
    let (width, height) = terminal_size()?;
    state.resize(width, height);
    let submitted = loop {
        terminal.terminal.draw(|frame| draw(frame, &state))?;
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => match handle_key_event(&mut state, key) {
                    TuiEventOutcome::Continue => {}
                    TuiEventOutcome::Quit => break None,
                    TuiEventOutcome::Submit(task) => break Some(task),
                },
                Event::Resize(width, height) => state.resize(width, height),
                _ => {}
            }
        }
    };
    Ok(TuiExit { state, submitted })
}

/// Applies a keyboard event to the TUI state.
pub fn handle_key_event(state: &mut TuiState, key: KeyEvent) -> TuiEventOutcome {
    if state.ask_user.is_some() {
        return handle_ask_user_key_event(state, key);
    }
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            TuiEventOutcome::Quit
        }
        KeyCode::Esc => TuiEventOutcome::Quit,
        KeyCode::Backspace => {
            state.input.pop();
            TuiEventOutcome::Continue
        }
        KeyCode::Enter => submit_input(state),
        KeyCode::Char(character) => {
            state.input.push(character);
            TuiEventOutcome::Continue
        }
        _ => TuiEventOutcome::Continue,
    }
}

fn handle_ask_user_key_event(state: &mut TuiState, key: KeyEvent) -> TuiEventOutcome {
    let Some(panel) = state.ask_user.as_mut() else {
        return TuiEventOutcome::Continue;
    };
    match key.code {
        KeyCode::Esc => {
            state.ask_user = None;
            TuiEventOutcome::Continue
        }
        KeyCode::Up => {
            panel.selected_index = panel.selected_index.saturating_sub(1);
            TuiEventOutcome::Continue
        }
        KeyCode::Down => {
            if !panel.choices.is_empty() {
                panel.selected_index = (panel.selected_index + 1).min(panel.choices.len() - 1);
            }
            TuiEventOutcome::Continue
        }
        KeyCode::Backspace if panel.choices.is_empty() => {
            panel.freeform.pop();
            TuiEventOutcome::Continue
        }
        KeyCode::Char(character) if panel.choices.is_empty() => {
            panel.freeform.push(character);
            TuiEventOutcome::Continue
        }
        KeyCode::Enter => {
            let question = panel.question.clone();
            let answer = panel.selected_answer();
            state.ask_user = None;
            state.push_transcript(format!("ask_user: {question} -> {answer}"));
            TuiEventOutcome::Continue
        }
        _ => TuiEventOutcome::Continue,
    }
}

fn submit_input(state: &mut TuiState) -> TuiEventOutcome {
    let input = state.input.trim().to_string();
    state.input.clear();
    if input.is_empty() {
        return TuiEventOutcome::Continue;
    }
    if input == "/quit" || input == "/exit" {
        return TuiEventOutcome::Quit;
    }
    state.push_transcript(format!("> {input}"));
    if let Some(command) = parse_slash_command(&input) {
        apply_slash_command(state, command);
        return TuiEventOutcome::Continue;
    }
    TuiEventOutcome::Submit(input)
}

fn apply_slash_command(state: &mut TuiState, command: SlashCommand) {
    match command {
        SlashCommand::Plan => {
            state.header.mode = ExecutionMode::Plan;
            state.push_transcript("mode: plan");
        }
        SlashCommand::Execute => {
            state.header.mode = ExecutionMode::Execute;
            state.push_transcript("mode: execute");
        }
        SlashCommand::GoalStart(goal) => {
            state.header.mode = ExecutionMode::Goal;
            state.side_panel.plan.push(PlanStep {
                label: goal.clone(),
                done: false,
            });
            state.push_transcript(format!("goal: {goal}"));
        }
        SlashCommand::GoalPause => state.push_transcript("goal: paused"),
        SlashCommand::GoalResume => state.push_transcript("goal: resumed"),
        SlashCommand::GoalClear => {
            state.side_panel.plan.clear();
            state.push_transcript("goal: cleared");
        }
        SlashCommand::GoalStatus => {
            let done = state
                .side_panel
                .plan
                .iter()
                .filter(|step| step.done)
                .count();
            state.push_transcript(format!(
                "goal: {done}/{} steps done",
                state.side_panel.plan.len()
            ));
        }
        SlashCommand::Safe => {
            state.header.permission = PermissionMode::Safe;
            state.push_transcript("permission: safe");
        }
        SlashCommand::Auto => {
            state.header.permission = PermissionMode::Auto;
            state.push_transcript("permission: auto");
        }
        SlashCommand::Yolo => {
            state.header.permission = PermissionMode::Yolo;
            state.push_transcript("permission: yolo");
        }
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

/// Renders a deterministic text snapshot for tests and headless previews.
pub fn render_text_snapshot(state: &TuiState) -> String {
    let mut output = String::new();
    let _ = writeln!(
        output,
        "PERIDOT | {}.{} | {} | ${:.4}",
        state.header.mode, state.header.permission, state.header.model, state.header.cost_usd
    );
    let _ = writeln!(output, "layout: {:?}", state.layout);
    let _ = writeln!(output);
    for line in state.transcript.iter().rev().take(20).rev() {
        let _ = writeln!(output, "{line}");
    }
    if state.layout == LayoutMode::Full {
        let done = state
            .side_panel
            .plan
            .iter()
            .filter(|step| step.done)
            .count();
        let _ = writeln!(output);
        let _ = writeln!(output, "Plan {done}/{}", state.side_panel.plan.len());
        for step in &state.side_panel.plan {
            let marker = if step.done { "[x]" } else { "[ ]" };
            let _ = writeln!(output, "{marker} {}", step.label);
        }
        let _ = writeln!(
            output,
            "Session steps={} errors={} elapsed={}s",
            state.side_panel.stats.steps,
            state.side_panel.stats.errors,
            state.side_panel.stats.elapsed_seconds
        );
    }
    let _ = write!(output, "> {}", state.input);
    output
}

/// Draws the TUI state with Ratatui.
pub fn draw(frame: &mut Frame<'_>, state: &TuiState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled("PERIDOT", Style::default().fg(Color::Green)),
        Span::raw(format!(
            " | {}.{} | {} | ${:.4}",
            state.header.mode, state.header.permission, state.header.model, state.header.cost_usd
        )),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, chunks[0]);

    let body_chunks = if state.layout == LayoutMode::Full {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(chunks[1])
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100)])
            .split(chunks[1])
    };
    let transcript = if let Some(panel) = &state.ask_user {
        render_ask_user_panel(panel)
    } else {
        state
            .transcript
            .iter()
            .rev()
            .take(body_chunks[0].height.saturating_sub(2) as usize)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    };
    frame.render_widget(
        Paragraph::new(transcript).block(
            Block::default()
                .title(body_title(state))
                .borders(Borders::ALL),
        ),
        body_chunks[0],
    );

    if state.layout == LayoutMode::Full && body_chunks.len() > 1 {
        let done = state
            .side_panel
            .plan
            .iter()
            .filter(|step| step.done)
            .count();
        let plan = state
            .side_panel
            .plan
            .iter()
            .map(|step| {
                let marker = if step.done { "[x]" } else { "[ ]" };
                format!("{marker} {}", step.label)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let side = format!(
            "Plan {done}/{}\n{}\n\nSession\nsteps: {}\nerrors: {}\nelapsed: {}s",
            state.side_panel.plan.len(),
            plan,
            state.side_panel.stats.steps,
            state.side_panel.stats.errors,
            state.side_panel.stats.elapsed_seconds
        );
        frame.render_widget(
            Paragraph::new(side).block(Block::default().title("Status").borders(Borders::ALL)),
            body_chunks[1],
        );
    }

    frame.render_widget(
        Paragraph::new(format!("> {}", state.input))
            .block(Block::default().title("Input").borders(Borders::ALL)),
        chunks[2],
    );
}

fn body_title(state: &TuiState) -> &'static str {
    if state.ask_user.is_some() {
        "Ask User"
    } else {
        "Transcript"
    }
}

fn render_ask_user_panel(panel: &AskUserPanel) -> String {
    if panel.choices.is_empty() {
        return format!("{}\n\n> {}", panel.question, panel.freeform);
    }
    let choices = panel
        .choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let marker = if index == panel.selected_index {
                ">"
            } else {
                " "
            };
            format!("{marker} {choice}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n\n{}", panel.question, choices)
}

/// Selects a layout mode from terminal dimensions.
pub fn select_layout(width: u16, height: u16) -> LayoutMode {
    if width >= 120 && height >= 32 {
        LayoutMode::Full
    } else if width >= 80 && height >= 20 {
        LayoutMode::Compact
    } else {
        LayoutMode::Minimal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_layout_from_terminal_size() {
        assert_eq!(select_layout(140, 40), LayoutMode::Full);
        assert_eq!(select_layout(90, 24), LayoutMode::Compact);
        assert_eq!(select_layout(60, 12), LayoutMode::Minimal);
    }

    #[test]
    fn parses_input_slash_command() {
        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));
        state.input = "/goal fix tests".to_string();

        assert_eq!(
            state.current_slash_command(),
            Some(SlashCommand::GoalStart("fix tests".to_string()))
        );
    }

    #[test]
    fn renders_text_snapshot() {
        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));
        state.push_transcript("tool file_write ok");
        state.side_panel.plan.push(PlanStep {
            label: "Implement hooks".to_string(),
            done: true,
        });

        let snapshot = render_text_snapshot(&state);

        assert!(snapshot.contains("PERIDOT | execute.auto | mock"));
        assert!(snapshot.contains("[x] Implement hooks"));
        assert!(snapshot.contains("tool file_write ok"));
    }

    #[test]
    fn draws_with_ratatui_backend() {
        use ratatui::{Terminal, backend::TestBackend};

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));
        state.resize(100, 30);
        state.push_transcript("hello tui");

        terminal.draw(|frame| draw(frame, &state)).unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = format!("{buffer:?}");
        assert!(rendered.contains("PERIDOT"));
        assert!(rendered.contains("hello tui"));
    }

    #[test]
    fn key_events_edit_and_submit_input() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));

        assert_eq!(
            handle_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)
            ),
            TuiEventOutcome::Continue
        );
        assert_eq!(
            handle_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE)
            ),
            TuiEventOutcome::Continue
        );
        assert_eq!(
            handle_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
            ),
            TuiEventOutcome::Continue
        );
        assert_eq!(state.input, "f");

        assert_eq!(
            handle_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            TuiEventOutcome::Submit("f".to_string())
        );
    }

    #[test]
    fn slash_commands_update_tui_state() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));
        for character in "/goal ship release".chars() {
            let outcome = handle_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            );
            assert_eq!(outcome, TuiEventOutcome::Continue);
        }

        let outcome = handle_key_event(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );

        assert_eq!(outcome, TuiEventOutcome::Continue);
        assert_eq!(state.header.mode, ExecutionMode::Goal);
        assert_eq!(state.side_panel.plan[0].label, "ship release");
    }

    #[test]
    fn ask_user_panel_renders_and_accepts_choice() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut state = TuiState::new(HeaderState::new(
            ExecutionMode::Execute,
            PermissionMode::Auto,
            "mock",
        ));
        state.open_ask_user(AskUserRequest::SingleSelect {
            question: "Proceed?".to_string(),
            options: vec!["yes".to_string(), "no".to_string()],
            default_index: Some(0),
        });

        assert!(render_ask_user_panel(state.ask_user.as_ref().unwrap()).contains("> yes"));
        assert_eq!(
            handle_key_event(&mut state, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            TuiEventOutcome::Continue
        );
        assert_eq!(
            handle_key_event(
                &mut state,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            TuiEventOutcome::Continue
        );

        assert!(state.ask_user.is_none());
        assert!(state.transcript[0].contains("Proceed? -> no"));
    }
}
