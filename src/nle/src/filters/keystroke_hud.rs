//! The keystroke HUD filter: it shows the keys that the user presses in a box
//! on the screen. Issue #161.

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the painter of the HUD uses the parser in a later commit of issue #161"
    )
)]
mod keys;

mod options;
pub use options::{
    HudOptions, Position, Rgb, ACCENT_OPTION, BACKGROUND_OPTION, BORDER_OPTION, DURATION_OPTION,
    FADE_OPTION, POSITION_OPTION, TEXT_OPTION, THEME_OPTION,
};

use crate::types::{AppliedFilter, FilterKind, FilterRegion};

/// The kind of the keystroke HUD filter.
pub const KIND: &str = "keystroke-hud";

/// The kind of the keystroke HUD filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// A keystroke HUD filter for `region` with `options`.
#[must_use]
pub fn applied(region: FilterRegion, options: &HudOptions) -> AppliedFilter {
    AppliedFilter {
        kind: kind(),
        region,
        options: options.to_options(),
    }
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the painter of the HUD uses the timeline in a later commit of issue #161"
    )
)]
mod timeline;
