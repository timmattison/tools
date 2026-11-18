//! A session that also gives a ratatui `Terminal`.

use std::io::{self, Stdout};
use std::sync::Arc;

use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;

use crate::{Options, Port, Session};

/// A session that also gives a ratatui `Terminal`.
///
/// The terminal is made after the erase, so its empty back buffer agrees with
/// the blank screen. The first frame then writes each cell that differs from a
/// blank cell, and no old shell text shows through. The session never asks the
/// terminal for the cursor position.
///
/// The terminal is restored when the session drops.
///
/// # Example
///
/// ```no_run
/// # fn main() -> std::io::Result<()> {
/// let mut session = altscreen::RatatuiSession::enter(altscreen::Options::default())?;
/// session.terminal().draw(|frame| {
///     frame.render_widget("hello", frame.area());
/// })?;
/// # Ok(())
/// # }
/// ```
#[must_use = "the terminal is restored when the session drops"]
pub struct RatatuiSession<B: Backend = CrosstermBackend<Stdout>> {
    /// This field comes first, so it drops before the session restores.
    terminal: Terminal<B>,
    session: Session,
}

impl RatatuiSession {
    /// Enters the alternate screen on the real terminal.
    ///
    /// The real terminal is standard output, with crossterm raw mode.
    ///
    /// # Errors
    ///
    /// Returns the error of raw mode, of the entry write, or of the terminal.
    /// A failed entry restores the terminal.
    pub fn enter(options: Options) -> io::Result<Self> {
        Self::enter_on(
            Arc::new(crate::StdoutPort),
            CrosstermBackend::new(io::stdout()),
            options,
        )
    }
}

impl<B: Backend> RatatuiSession<B>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    /// Enters the alternate screen on `port` and draws through `backend`.
    ///
    /// # Errors
    ///
    /// Returns the error of raw mode, of the entry write, or of the backend.
    #[doc(hidden)]
    pub fn enter_on(port: Arc<dyn Port>, backend: B, options: Options) -> io::Result<Self> {
        let session = Session::enter_on(port, options)?;
        let terminal = Terminal::new(backend).map_err(io::Error::other)?;
        Ok(Self { terminal, session })
    }
}

impl<B: Backend> RatatuiSession<B> {
    /// The terminal to draw on.
    pub fn terminal(&mut self) -> &mut Terminal<B> {
        &mut self.terminal
    }
}

#[cfg(test)]
mod tests {
    use std::sync::PoisonError;

    use ratatui::backend::{ClearType, TestBackend, WindowSize};
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Position, Size};
    use ratatui::widgets::Paragraph;

    use super::*;
    use crate::test_support::{fake, PANIC_HOOK_LOCK};

    /// The text that the shell wrote on the screen before the tool started.
    const SHELL_TEXT: &str = "old-shell-text";

    const WIDTH: u16 = 40;
    const HEIGHT: u16 = 6;

    /// A screen that the shell filled before the tool started. Each cell holds
    /// a glyph, and no cell is blank.
    fn screen_with_shell_text(width: u16, height: u16) -> TestBackend {
        TestBackend::with_lines((0..height).map(|row| {
            format!("{SHELL_TEXT}-{row}|")
                .chars()
                .cycle()
                .take(usize::from(width))
                .collect::<String>()
        }))
    }

    /// Each row of `buffer` as one string.
    fn lines(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area;
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    /// A terminal that does not answer the query for the cursor position.
    /// A recording under `script` does not answer it, and a slow link answers
    /// too late. Each other call goes to the `TestBackend` inside.
    struct NoCursorReport(TestBackend);

    impl Backend for NoCursorReport {
        type Error = io::Error;

        fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            let Ok(()) = self.0.draw(content);
            Ok(())
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            let Ok(()) = self.0.hide_cursor();
            Ok(())
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            let Ok(()) = self.0.show_cursor();
            Ok(())
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Err(io::Error::other(
                "the terminal does not report the cursor position",
            ))
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            let Ok(()) = self.0.set_cursor_position(position);
            Ok(())
        }

        fn clear(&mut self) -> io::Result<()> {
            let Ok(()) = self.0.clear();
            Ok(())
        }

        fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
            let Ok(()) = self.0.clear_region(clear_type);
            Ok(())
        }

        fn size(&self) -> io::Result<Size> {
            let Ok(size) = self.0.size();
            Ok(size)
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            let Ok(size) = self.0.window_size();
            Ok(size)
        }

        fn flush(&mut self) -> io::Result<()> {
            let Ok(()) = self.0.flush();
            Ok(())
        }
    }

    #[test]
    fn the_view_starts_on_a_terminal_that_does_not_report_the_cursor() {
        let _lock = PANIC_HOOK_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (port, _events) = fake(|_| {});
        let backend = NoCursorReport(screen_with_shell_text(WIDTH, HEIGHT));
        let mut session = RatatuiSession::enter_on(port, backend, Options::default())
            .expect("the session must start");
        let drawn = session
            .terminal()
            .draw(|frame| frame.render_widget(Paragraph::new("hello"), frame.area()));
        assert!(drawn.is_ok(), "{drawn:?}");
        let frame = lines(session.terminal().backend().0.buffer());
        assert!(!frame.iter().any(|l| l.contains(SHELL_TEXT)), "{frame:#?}");
    }

    #[test]
    fn the_first_frame_hides_the_text_that_was_on_the_screen_before() {
        let draw_on = |backend: TestBackend| {
            let _lock = PANIC_HOOK_LOCK
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let (port, _events) = fake(|_| {});
            let mut session = RatatuiSession::enter_on(port, backend, Options::default())
                .expect("the session must start");
            session
                .terminal()
                .draw(|frame| frame.render_widget(Paragraph::new("hello"), frame.area()))
                .expect("the first frame must draw");
            lines(session.terminal().backend().buffer())
        };
        let expected = draw_on(TestBackend::new(WIDTH, HEIGHT));
        let frame = draw_on(screen_with_shell_text(WIDTH, HEIGHT));
        assert!(!frame.iter().any(|l| l.contains(SHELL_TEXT)), "{frame:#?}");
        assert_eq!(frame, expected);
    }
}
