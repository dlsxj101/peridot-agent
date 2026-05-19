use super::*;

pub(super) struct TerminalGuard {
    pub(super) terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    pub(super) fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Mouse capture is intentionally NOT enabled: it would intercept drag
        // gestures and prevent the operator from selecting transcript text to
        // copy. Wheel scrolling is handled by PageUp/PageDown and Shift+↑/↓
        // keybindings instead, which work in every terminal regardless of how
        // the host handles wheel events.
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
