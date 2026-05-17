use super::*;

pub(super) struct TerminalGuard {
    pub(super) terminal: Terminal<CrosstermBackend<Stdout>>,
    // Whether kitty-keyboard-protocol enhancement flags were successfully
    // pushed during `enter()`. Drop only pops them when we actually pushed,
    // otherwise we'd emit a CSI-u sequence into terminals that don't grok it.
    pushed_keyboard_flags: bool,
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
        execute!(stdout, EnterAlternateScreen)?;
        // Negotiate kitty-keyboard-protocol so we can distinguish
        // `Shift+Enter` (newline in the prompt) from a bare `Enter` (submit).
        // Without this, most terminals collapse both into the same `\r` and
        // the SHIFT modifier never reaches the handler in `input.rs`.
        // Terminals that don't support CSI-u (Windows conpty, some legacy
        // emulators) silently ignore the sequence — we fall back to
        // `Alt+Enter` for those in the key handler.
        let pushed_keyboard_flags = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            pushed_keyboard_flags,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.pushed_keyboard_flags {
            let _ = execute!(self.terminal.backend_mut(), PopKeyboardEnhancementFlags);
        }
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}
