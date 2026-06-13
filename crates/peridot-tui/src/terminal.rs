use super::*;

pub(super) struct TerminalGuard {
    pub(super) terminal: Terminal<CrosstermBackend<Stdout>>,
    mouse_captured: bool,
}

impl TerminalGuard {
    pub(super) fn enter(mouse_capture: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Mouse capture is opt-in (`tui.mouse_capture`, default on). When on,
        // the wheel scrolls the transcript (Claude-Code feel) but the app then
        // receives click/drag too, so native drag-to-select-copy becomes
        // `Shift`+drag (`Option`+drag on macOS) — honoured by virtually every
        // modern terminal. When off we leave the mouse alone so plain drag
        // selects text, and scrolling falls back to PageUp/PageDown / Shift+↑↓.
        //
        // PushKeyboardEnhancementFlags (CSI-u / kitty keyboard protocol) is
        // intentionally NOT pushed here. It would let us distinguish
        // `Shift+Enter` from bare `Enter`, but on terminals that don't fully
        // honour the negotiation (Windows Terminal under WSL conpty was the
        // case that bit us) the request silently corrupts unrelated key
        // mappings — `Ctrl+]` stopped firing the side-panel toggle. Operators
        // use `Alt+Enter` or `Ctrl+J` for newlines instead — both bypass the
        // CSI-u negotiation entirely and work on every terminal we've tried.
        execute!(stdout, EnterAlternateScreen)?;
        if mouse_capture {
            execute!(stdout, EnableMouseCapture)?;
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            mouse_captured: mouse_capture,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.mouse_captured {
            let _ = execute!(self.terminal.backend_mut(), DisableMouseCapture);
        }
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}
