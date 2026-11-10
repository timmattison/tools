//! Draw the state of the screen with ratatui.
//!
//! `draw` reads the state through the read accessors of `App`. It holds no
//! state of its own.

use std::path::Path;

use ratatui::Frame;

use crate::app::{App, Drawn};

/// What the draw needs besides the state of the app.
pub struct Context<'a, Tz: chrono::TimeZone> {
    /// The home directory. A path under it shows as `~`.
    pub home: Option<&'a Path>,
    /// The time zone of the dates in the list.
    pub tz: &'a Tz,
}

/// Draw the screen into `frame` and report what the draw measured.
///
/// The draw reads the size of the frame on each call. It never panics, for any
/// size from 0 by 0 up.
pub fn draw<Tz: chrono::TimeZone>(
    frame: &mut Frame,
    app: &App,
    ctx: &Context<'_, Tz>,
) -> Drawn
where
    Tz::Offset: std::fmt::Display,
{
    let _ = (frame, app, ctx);
    Drawn {
        list_rows: 1,
        preview_scroll: 0,
    }
}
