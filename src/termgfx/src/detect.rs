//! The reader of the environment that names the terminal a run draws into.
//!
//! A terminal that draws an image draws it from an escape sequence, and three
//! such sequences are in service. Kitty carries the pixels in a protocol of
//! its own. iTerm2 carries a whole image file in one sequence. Sixel carries a
//! palette and then a band of pixels at a time. No terminal reads all three,
//! and a terminal answers no question about the ones it reads. A tool that
//! wants to draw an image therefore reads the environment variables that the
//! terminal set, names the terminal from what it finds there, and then picks
//! the one sequence that terminal reads.
//!
//! A caller asks [`Capabilities::detect`] once and gets three facts: which
//! terminal it draws into, whether that terminal draws an image at all, and
//! whether that terminal takes the raw mode that a key press needs.
//! [`display_routine_for`] turns the first of those facts into the one routine
//! that draws.
//!
//! # What the detection has to get right
//!
//! A terminal panel that runs a program of its own leaks the environment of
//! the terminal that started it. A muxiavelli panel draws through the
//! `@xterm/addon-image` of xterm.js, and the process in that panel still reads
//! `KITTY_WINDOW_ID` from the Kitty window that started the muxiavelli server.
//! A detector that reads the host signal first therefore names the panel a
//! Kitty window and sends it a sequence that xterm.js does not read. The panel
//! then prints the escape sequence as text. So the panel's own signal wins
//! over every host signal, and [`classify_terminal_type`] states that order.
//!
//! The same trap holds for the width of the answer. A terminal that leaks
//! `TERM=screen` from a backing session still draws an image when the panel
//! draws one, so the terminal type decides first and the `TERM` string only
//! decides the terminals that carry no better signal.

use std::io;

/// An inline-image protocol that a caller emits into a muxiavelli panel.
///
/// muxiavelli panels render through the xterm.js of ttyd with
/// `@xterm/addon-image`, which supports **Sixel and iTerm2 IIP only**. The
/// Kitty graphics protocol is deliberately absent from this enum: it is
/// structurally impossible to pick Kitty for a muxiavelli panel, which is how
/// the shared contract's "Never Kitty" guarantee is enforced at compile time
/// rather than by convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    /// A palette and then a band of pixels at a time.
    Sixel,
    /// A whole image file in one escape sequence.
    Iterm2,
}

impl ImageProtocol {
    /// Parse a single `MUXIAVELLI_IMAGE_PROTOCOLS` token (case- and
    /// whitespace-insensitive). Returns `None` for anything a caller cannot
    /// emit into a muxiavelli panel (including `kitty`), so unsupported tokens
    /// are simply skipped when selecting from the advertised list.
    fn parse_token(token: &str) -> Option<Self> {
        let token = token.trim();
        if token.eq_ignore_ascii_case("sixel") {
            Some(Self::Sixel)
        } else if token.eq_ignore_ascii_case("iterm2") {
            Some(Self::Iterm2)
        } else {
            None
        }
    }
}

/// The protocol a caller falls back to for a muxiavelli panel when the
/// advertised preference list is absent, empty, or names nothing supported.
const DEFAULT_MUXIAVELLI_PROTOCOL: ImageProtocol = ImageProtocol::Sixel;

/// Resolve the ordered `MUXIAVELLI_IMAGE_PROTOCOLS` preference list to the
/// first protocol a caller can emit, honoring the advertised order rather than
/// hardcoding a choice. Falls back to Sixel (never Kitty) when the list is
/// absent, empty, or names nothing supported.
fn select_muxiavelli_protocol(raw: Option<&str>) -> ImageProtocol {
    raw.unwrap_or_default()
        .split(',')
        .filter_map(ImageProtocol::parse_token)
        .next()
        .unwrap_or(DEFAULT_MUXIAVELLI_PROTOCOL)
}

/// The terminal that a run draws into, as far as the environment says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalType {
    /// The Kitty terminal.
    Kitty,
    /// The Ghostty terminal.
    Ghostty,
    /// The iTerm2 terminal.
    ITerm2,
    /// The WezTerm terminal.
    WezTerm,
    /// The Alacritty terminal, which draws no image.
    Alacritty,
    /// A pane of the Zellij multiplexer.
    Zellij,
    /// A muxiavelli panel (ttyd/xterm.js). Carries the resolved inline-image
    /// protocol so the host terminal's leaked env vars cannot override it.
    Muxiavelli(ImageProtocol),
    /// A terminal that set no signal this crate reads.
    Unknown,
}

/// What one terminal does, as three facts a caller acts on.
///
/// The three facts stand behind methods, and the fields stay private, because
/// the only honest way to make them is to read the environment of the process.
/// A caller that held the fields could build a set of capabilities that no
/// terminal ever has, and the tool would then draw into a terminal that reads
/// nothing it sends.
#[derive(Debug, Clone)]
pub struct Capabilities {
    terminal_type: TerminalType,
    draws_images: bool,
    raw_mode: bool,
}

impl Capabilities {
    /// Read the environment of this process and name what the terminal does.
    ///
    /// This is the entrance a tool uses. It reads the environment variables
    /// once, names the terminal from them, and asks the operating system
    /// whether standard output is a terminal at all.
    #[must_use]
    pub fn detect() -> Self {
        Self::from_env(&TerminalEnv::from_process(), stdout_is_a_terminal())
    }

    /// Build a set of capabilities from facts that the caller already holds.
    ///
    /// [`Capabilities::detect`] is the entrance for a run, and this
    /// constructor is the entrance for a test: a test that covers what a tool
    /// prints for one terminal names that terminal here, where a test that
    /// called `detect` would answer with the terminal of whoever started the
    /// test run.
    #[must_use]
    pub fn new(terminal_type: TerminalType, draws_images: bool, raw_mode: bool) -> Self {
        Self {
            terminal_type,
            draws_images,
            raw_mode,
        }
    }

    /// Name the terminal that this run draws into.
    #[must_use]
    pub fn terminal_type(&self) -> &TerminalType {
        &self.terminal_type
    }

    /// Whether this terminal draws an inline image at all.
    #[must_use]
    pub fn draws_images(&self) -> bool {
        self.draws_images
    }

    /// Whether this terminal draws an inline image and named itself as well.
    ///
    /// A terminal that set none of the signals this crate reads is a
    /// [`TerminalType::Unknown`], and [`display_routine_for`] sends such a
    /// terminal the sequence of iTerm2. That is a guess at the protocol, and it
    /// is the only thing left to do: no terminal answers a question about the
    /// sequences it reads. So this method answers no for a terminal of no name,
    /// and it answers [`Capabilities::draws_images`] for every terminal that
    /// the crate does name.
    ///
    /// This is not [`Capabilities::draws_images`]. That method takes the guess,
    /// and the guess is right for a tool with no second way to show the
    /// picture, because an image at a guessed protocol beats no image. It is
    /// wrong for a tool that has one, because an escape sequence that the
    /// terminal does not read lands on the screen as text. `ic` is the first
    /// kind of tool and `krt` is the second, and [`crate::cell_pixels`] splits
    /// the same two audiences over the size of a character cell.
    #[must_use]
    pub fn draws_images_by_name(&self) -> bool {
        self.draws_images
    }

    /// Whether this terminal takes the raw mode that a key press needs.
    #[must_use]
    pub fn raw_mode(&self) -> bool {
        self.raw_mode
    }

    /// Name what the terminal does from a captured environment.
    ///
    /// The capture stands apart from the reading of it, so that a test names
    /// the environment it covers and the answer holds no state of the machine
    /// that runs the test.
    fn from_env(env: &TerminalEnv, raw_mode: bool) -> Self {
        let terminal_type = classify_terminal_type(env);
        let draws_images = terminal_supports_graphics(&terminal_type, &env.term);
        Self {
            terminal_type,
            draws_images,
            raw_mode,
        }
    }
}

/// Whether standard output is a terminal, which is what raw mode needs.
fn stdout_is_a_terminal() -> bool {
    use std::os::unix::io::AsRawFd;
    // SAFETY: isatty() is a read-only check that only examines whether the file
    // descriptor refers to a terminal. It has no side effects and cannot cause
    // memory unsafety. The file descriptor from stdout is always valid.
    unsafe { libc::isatty(io::stdout().as_raw_fd()) == 1 }
}

/// Snapshot of the environment variables that influence terminal detection.
///
/// Captured once so that [`classify_terminal_type`] is a pure function of its
/// inputs — testable without mutating process-global env vars (which is `unsafe`
/// in the 2024 edition and not parallel-safe for the test suite).
#[derive(Debug, Clone, Default)]
struct TerminalEnv {
    term: String,
    term_program: String,
    zellij: bool,
    muxiavelli: bool,
    muxiavelli_protocols: Option<String>,
    kitty_window_id: bool,
    ghostty_resources_dir: bool,
    iterm_session_id: bool,
    alacritty_socket: bool,
}

impl TerminalEnv {
    /// Capture the detection-relevant environment variables from the process.
    fn from_process() -> Self {
        let is_set = |key: &str| std::env::var(key).is_ok();
        TerminalEnv {
            term: std::env::var("TERM").unwrap_or_default(),
            term_program: std::env::var("TERM_PROGRAM").unwrap_or_default(),
            zellij: is_set("ZELLIJ"),
            // The contract pins MUXIAVELLI to the literal "1"; treat any other
            // value (including "0") as "not a muxiavelli panel".
            muxiavelli: matches!(std::env::var("MUXIAVELLI").as_deref(), Ok("1")),
            muxiavelli_protocols: std::env::var("MUXIAVELLI_IMAGE_PROTOCOLS").ok(),
            kitty_window_id: is_set("KITTY_WINDOW_ID"),
            ghostty_resources_dir: is_set("GHOSTTY_RESOURCES_DIR"),
            iterm_session_id: is_set("ITERM_SESSION_ID"),
            alacritty_socket: is_set("ALACRITTY_SOCKET"),
        }
    }
}

/// Classify the terminal from a captured [`TerminalEnv`].
///
/// `MUXIAVELLI` is checked **first** — before Zellij and every host-terminal
/// signal — because a muxiavelli panel's PTY inherits (leaks) the env vars of
/// whatever terminal launched the muxiavelli server. Honoring the panel's own
/// advertised capability is the only correct signal; the leaked host vars must
/// not win. (Same reasoning as checking Zellij before the host terminals.)
fn classify_terminal_type(env: &TerminalEnv) -> TerminalType {
    if env.muxiavelli {
        TerminalType::Muxiavelli(select_muxiavelli_protocol(
            env.muxiavelli_protocols.as_deref(),
        ))
    } else if env.zellij {
        TerminalType::Zellij
    } else if env.kitty_window_id || env.term.contains("kitty") {
        TerminalType::Kitty
    } else if env.term_program == "ghostty"
        || env.ghostty_resources_dir
        || env.term.contains("ghostty")
    {
        TerminalType::Ghostty
    } else if env.term_program.contains("iTerm") || env.iterm_session_id {
        TerminalType::ITerm2
    } else if env.term_program.contains("WezTerm") {
        TerminalType::WezTerm
    } else if env.alacritty_socket || env.term.contains("alacritty") {
        TerminalType::Alacritty
    } else {
        TerminalType::Unknown
    }
}

/// Whether a resolved terminal type can render inline graphics at all.
///
/// muxiavelli panels always can (xterm.js `@xterm/addon-image`), regardless of
/// any `TERM` leaked from a backing session.
fn terminal_supports_graphics(terminal_type: &TerminalType, term: &str) -> bool {
    match terminal_type {
        TerminalType::Muxiavelli(_) => true,
        TerminalType::Alacritty => false,
        _ => {
            !term.contains("linux")   // Linux console doesn't support graphics
                && !term.contains("screen") // Screen doesn't support graphics
                && !term.starts_with("vt") // VT terminals don't support graphics
        }
    }
}

/// The concrete inline-image routine a resolved terminal type dispatches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayRoutine {
    /// A palette and then a band of pixels at a time.
    Sixel,
    /// The graphics protocol of Kitty.
    Kitty,
    /// A whole image file in one escape sequence.
    Iterm2,
}

/// Map a resolved [`TerminalType`] to the display routine a caller uses.
///
/// Deliberately exhaustive (no wildcard arm): adding a future `TerminalType`
/// forces a conscious routing decision here instead of silently falling through
/// to iTerm2 — the exact failure mode that sent the Kitty protocol into
/// muxiavelli panels.
#[must_use]
pub(crate) fn display_routine_for(terminal_type: &TerminalType) -> DisplayRoutine {
    match terminal_type {
        // Zellij and muxiavelli-Sixel both go through the Sixel path.
        TerminalType::Zellij | TerminalType::Muxiavelli(ImageProtocol::Sixel) => {
            DisplayRoutine::Sixel
        }
        TerminalType::Muxiavelli(ImageProtocol::Iterm2) => DisplayRoutine::Iterm2,
        TerminalType::Kitty | TerminalType::Ghostty | TerminalType::WezTerm => {
            DisplayRoutine::Kitty
        }
        TerminalType::ITerm2 | TerminalType::Alacritty | TerminalType::Unknown => {
            DisplayRoutine::Iterm2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_muxiavelli_sixel_panel_draws_with_sixel_and_a_kitty_window_draws_with_kitty() {
        let panel = Capabilities::from_env(
            &TerminalEnv {
                muxiavelli: true,
                muxiavelli_protocols: Some("sixel".to_string()),
                ..TerminalEnv::default()
            },
            false,
        );
        assert_eq!(
            display_routine_for(panel.terminal_type()),
            DisplayRoutine::Sixel
        );

        let window = Capabilities::from_env(
            &TerminalEnv {
                kitty_window_id: true,
                ..TerminalEnv::default()
            },
            false,
        );
        assert_eq!(
            display_routine_for(window.terminal_type()),
            DisplayRoutine::Kitty
        );
    }

    // =========================================================================
    // Tests for terminal type detection (basic sanity checks)
    // =========================================================================

    #[test]
    fn terminal_type_enum_is_exhaustive() {
        // Ensure we can construct all terminal types (compile-time check)
        let _kitty = TerminalType::Kitty;
        let _ghostty = TerminalType::Ghostty;
        let _iterm2 = TerminalType::ITerm2;
        let _wezterm = TerminalType::WezTerm;
        let _alacritty = TerminalType::Alacritty;
        let _zellij = TerminalType::Zellij;
        let _muxiavelli = TerminalType::Muxiavelli(ImageProtocol::Sixel);
        let _unknown = TerminalType::Unknown;
    }

    // =========================================================================
    // Tests for muxiavelli image-protocol selection (select_muxiavelli_protocol)
    // =========================================================================

    #[test]
    fn muxiavelli_protocol_absent_falls_back_to_sixel() {
        assert_eq!(select_muxiavelli_protocol(None), ImageProtocol::Sixel);
    }

    #[test]
    fn muxiavelli_protocol_empty_falls_back_to_sixel() {
        assert_eq!(select_muxiavelli_protocol(Some("")), ImageProtocol::Sixel);
    }

    #[test]
    fn muxiavelli_protocol_unrecognized_falls_back_to_sixel() {
        // Kitty (and any other token `ic` cannot emit) must never be selected.
        assert_eq!(
            select_muxiavelli_protocol(Some("kitty")),
            ImageProtocol::Sixel
        );
        assert_eq!(
            select_muxiavelli_protocol(Some("kitty,png,webp")),
            ImageProtocol::Sixel
        );
    }

    #[test]
    fn muxiavelli_protocol_honors_advertised_order() {
        // Proves the list is honored in order, not hardcoded to Sixel.
        assert_eq!(
            select_muxiavelli_protocol(Some("sixel,iterm2")),
            ImageProtocol::Sixel
        );
        assert_eq!(
            select_muxiavelli_protocol(Some("iterm2,sixel")),
            ImageProtocol::Iterm2
        );
    }

    #[test]
    fn muxiavelli_protocol_skips_unsupported_then_picks_supported() {
        // An unsupported leading token is skipped, not treated as a fallback.
        assert_eq!(
            select_muxiavelli_protocol(Some("kitty,iterm2")),
            ImageProtocol::Iterm2
        );
    }

    #[test]
    fn muxiavelli_protocol_tolerates_whitespace_and_case() {
        assert_eq!(
            select_muxiavelli_protocol(Some("  ITERM2 , sixel ")),
            ImageProtocol::Iterm2
        );
    }

    // =========================================================================
    // Tests for classify_terminal_type (precedence + existing detection)
    // =========================================================================

    /// A muxiavelli panel whose PTY leaked the host terminal's env vars: Kitty
    /// **and** Ghostty signals are present simultaneously. MUXIAVELLI must still
    /// win over both. This is the exact mis-detection the issue describes.
    fn leaked_muxiavelli_env(protocols: Option<&str>) -> TerminalEnv {
        TerminalEnv {
            term: "xterm-ghostty".to_string(),
            term_program: "ghostty".to_string(),
            zellij: false,
            muxiavelli: true,
            muxiavelli_protocols: protocols.map(str::to_string),
            kitty_window_id: true,
            ghostty_resources_dir: true,
            iterm_session_id: false,
            alacritty_socket: false,
        }
    }

    #[test]
    fn muxiavelli_wins_over_leaked_host_signals_and_selects_sixel() {
        let env = leaked_muxiavelli_env(Some("sixel,iterm2"));
        assert_eq!(
            classify_terminal_type(&env),
            TerminalType::Muxiavelli(ImageProtocol::Sixel)
        );
    }

    #[test]
    fn muxiavelli_iterm2_first_selects_iterm2_despite_leak() {
        let env = leaked_muxiavelli_env(Some("iterm2,sixel"));
        assert_eq!(
            classify_terminal_type(&env),
            TerminalType::Muxiavelli(ImageProtocol::Iterm2)
        );
    }

    #[test]
    fn muxiavelli_absent_protocols_defaults_to_sixel() {
        let env = leaked_muxiavelli_env(None);
        assert_eq!(
            classify_terminal_type(&env),
            TerminalType::Muxiavelli(ImageProtocol::Sixel)
        );
    }

    #[test]
    fn muxiavelli_wins_over_zellij_backend() {
        // muxiavelli may run a zellij backing session, so ZELLIJ can also be set
        // inside a panel; MUXIAVELLI must still win since it is the precise signal.
        let mut env = leaked_muxiavelli_env(None);
        env.zellij = true;
        assert_eq!(
            classify_terminal_type(&env),
            TerminalType::Muxiavelli(ImageProtocol::Sixel)
        );
    }

    #[test]
    fn classify_zellij_when_not_muxiavelli() {
        let env = TerminalEnv {
            zellij: true,
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::Zellij);
    }

    #[test]
    fn classify_kitty_from_window_id() {
        let env = TerminalEnv {
            kitty_window_id: true,
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::Kitty);
    }

    #[test]
    fn classify_kitty_from_term() {
        let env = TerminalEnv {
            term: "xterm-kitty".to_string(),
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::Kitty);
    }

    #[test]
    fn classify_ghostty_from_term_program() {
        let env = TerminalEnv {
            term_program: "ghostty".to_string(),
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::Ghostty);
    }

    #[test]
    fn classify_iterm2_from_session_id() {
        let env = TerminalEnv {
            iterm_session_id: true,
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::ITerm2);
    }

    #[test]
    fn classify_iterm2_from_term_program() {
        let env = TerminalEnv {
            term_program: "iTerm.app".to_string(),
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::ITerm2);
    }

    #[test]
    fn classify_wezterm_from_term_program() {
        let env = TerminalEnv {
            term_program: "WezTerm".to_string(),
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::WezTerm);
    }

    #[test]
    fn classify_alacritty_from_socket() {
        let env = TerminalEnv {
            alacritty_socket: true,
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::Alacritty);
    }

    #[test]
    fn classify_unknown_when_no_signals() {
        assert_eq!(
            classify_terminal_type(&TerminalEnv::default()),
            TerminalType::Unknown
        );
    }

    // =========================================================================
    // Tests for terminal_supports_graphics
    // =========================================================================

    #[test]
    fn muxiavelli_supports_graphics_regardless_of_term() {
        // Even if a backing session leaks TERM=screen/vt, muxiavelli renders images.
        assert!(terminal_supports_graphics(
            &TerminalType::Muxiavelli(ImageProtocol::Sixel),
            "screen.xterm-256color"
        ));
        assert!(terminal_supports_graphics(
            &TerminalType::Muxiavelli(ImageProtocol::Iterm2),
            "vt100"
        ));
    }

    #[test]
    fn alacritty_never_supports_graphics() {
        assert!(!terminal_supports_graphics(
            &TerminalType::Alacritty,
            "xterm-256color"
        ));
    }

    #[test]
    fn kitty_supports_graphics_on_normal_term() {
        assert!(terminal_supports_graphics(
            &TerminalType::Kitty,
            "xterm-kitty"
        ));
    }

    #[test]
    fn linux_console_does_not_support_graphics() {
        assert!(!terminal_supports_graphics(&TerminalType::Unknown, "linux"));
    }

    // =========================================================================
    // Tests for draws_images_by_name
    // =========================================================================

    #[test]
    fn a_terminal_that_this_crate_cannot_name_draws_no_image_by_name() {
        // A terminal that set none of the signals this crate reads carries no
        // name, so the routine it takes is a guess. `display_routine_for` sends
        // it the sequence of iTerm2, and xterm, GNOME Terminal and Konsole each
        // read none of that sequence and print it as text.
        let no_name = Capabilities::from_env(
            &TerminalEnv {
                term: "xterm-256color".to_string(),
                ..TerminalEnv::default()
            },
            false,
        );
        assert_eq!(no_name.terminal_type(), &TerminalType::Unknown);
        assert!(
            no_name.draws_images(),
            "the crate takes such a terminal at its word for a caller with no second way to draw"
        );
        assert!(
            !no_name.draws_images_by_name(),
            "and it draws no image by name, because nothing said which sequence this terminal reads"
        );

        // A Kitty window named itself, so the routine it takes is the routine
        // that this terminal reads.
        let window = Capabilities::from_env(
            &TerminalEnv {
                kitty_window_id: true,
                ..TerminalEnv::default()
            },
            false,
        );
        assert!(
            window.draws_images_by_name(),
            "a terminal that named itself draws the image of the sequence it reads"
        );

        // An Alacritty window named itself and it draws no image at all, so the
        // name settles nothing that the protocol left open.
        let alacritty = Capabilities::from_env(
            &TerminalEnv {
                alacritty_socket: true,
                ..TerminalEnv::default()
            },
            false,
        );
        assert!(
            !alacritty.draws_images_by_name(),
            "a terminal that draws no image draws none under its own name either"
        );
    }

    // =========================================================================
    // Tests for display_routine_for (dispatch routing)
    // =========================================================================

    #[test]
    fn dispatch_muxiavelli_sixel_routes_to_sixel() {
        assert_eq!(
            display_routine_for(&TerminalType::Muxiavelli(ImageProtocol::Sixel)),
            DisplayRoutine::Sixel
        );
    }

    #[test]
    fn dispatch_muxiavelli_iterm2_routes_to_iterm2() {
        assert_eq!(
            display_routine_for(&TerminalType::Muxiavelli(ImageProtocol::Iterm2)),
            DisplayRoutine::Iterm2
        );
    }

    #[test]
    fn dispatch_zellij_routes_to_sixel() {
        assert_eq!(
            display_routine_for(&TerminalType::Zellij),
            DisplayRoutine::Sixel
        );
    }

    #[test]
    fn dispatch_kitty_family_routes_to_kitty() {
        for terminal_type in [
            TerminalType::Kitty,
            TerminalType::Ghostty,
            TerminalType::WezTerm,
        ] {
            assert_eq!(display_routine_for(&terminal_type), DisplayRoutine::Kitty);
        }
    }

    #[test]
    fn dispatch_other_terminals_route_to_iterm2() {
        for terminal_type in [
            TerminalType::ITerm2,
            TerminalType::Alacritty,
            TerminalType::Unknown,
        ] {
            assert_eq!(display_routine_for(&terminal_type), DisplayRoutine::Iterm2);
        }
    }
}
