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
