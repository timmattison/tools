//! What a mosh session says about the images it carries.
//!
//! A query cannot answer this question. The process writes the query to the
//! pseudo terminal of its pane, and inside a multiplexer the multiplexer owns
//! that pseudo terminal and answers every query itself. So the answer states
//! what the multiplexer draws, and it states nothing about the transport that
//! carries the bytes to the terminal of the user.
//!
//! That matters because two transports carry the same name. Upstream mosh
//! strips every escape sequence that carries an image, and a port of mosh that
//! draws images carries all three. A round trip cannot tell them apart, and a
//! tool that cannot tell them apart has to refuse both.
//!
//! **The environment is the one channel that crosses a multiplexer.** A
//! multiplexer hands the environment of the session to every pane it starts,
//! unchanged. So a mosh that draws images states it there, and this module
//! reads what it stated:
//!
//! * `MOSH_IMAGES` names the protocols that the **transport** carries. Only a
//!   mosh that draws images writes it, so a mosh session that carries no such
//!   variable is an upstream mosh and a tool still refuses it.
//! * `MOSH_CLIENT_IMAGES` names the protocols that the **terminal of the user**
//!   draws. An upstream server started by the wrapper of such a mosh carries
//!   this one and not the first, which is why the two are separate names.
//!
//! Both hold a comma-separated list of `kitty`, `sixel` and `iterm2`. See
//! <https://github.com/timmattison/mosh-rs/issues/78>.
//!
//! **A variable that crosses a multiplexer outlives the session that wrote
//! it.** A user exports it by hand, and a tmux server or a Zellij server that a
//! mosh session started hands the whole environment of that session to every
//! pane it opens after the mosh session ends. So this module states what a
//! transport carries, and it states nothing about which transport this session
//! has. A caller reads the process tree for that, and it reads the two answers
//! together.

use crate::detect::DisplayRoutine;

/// The environment variable that names what the transport of the session
/// carries.
const TRANSPORT_VARIABLE: &str = "MOSH_IMAGES";

/// The environment variable that names what the terminal of the user draws.
const CLIENT_VARIABLE: &str = "MOSH_CLIENT_IMAGES";

/// The name that both variables give the kitty graphics protocol.
const KITTY_NAME: &str = "kitty";

/// The name that both variables give the Sixel protocol.
const SIXEL_NAME: &str = "sixel";

/// The name that both variables give the inline image protocol of iTerm2.
const ITERM2_NAME: &str = "iterm2";

/// What a message calls a set that names no protocol at all.
const EMPTY_SET_NAME: &str = "none";

/// A set of the inline-image protocols that one party carries or draws.
///
/// The three protocols stand as three flags rather than as a list, because the
/// one question a caller asks of two such sets is which protocols stand in
/// both of them. A list would answer that question with a search of one list
/// for each member of the other, and it would answer "kitty, kitty" for a
/// value that names one protocol twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProtocolSet {
    kitty: bool,
    sixel: bool,
    iterm2: bool,
}

impl ProtocolSet {
    /// Read a comma-separated list of protocol names.
    ///
    /// A name is read whatever its case, and the space around it is dropped. A
    /// token that names no protocol this crate draws is dropped as well, so a
    /// transport that grows a fourth protocol does not turn this set into a
    /// promise that this crate cannot keep.
    ///
    /// # Arguments
    /// * `raw` - The value of the variable.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let mut set = Self::default();
        for token in raw.split(',') {
            let token = token.trim();
            if token.eq_ignore_ascii_case(KITTY_NAME) {
                set.kitty = true;
            } else if token.eq_ignore_ascii_case(SIXEL_NAME) {
                set.sixel = true;
            } else if token.eq_ignore_ascii_case(ITERM2_NAME) {
                set.iterm2 = true;
            }
        }
        set
    }

    /// Whether this set names no protocol at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.kitty && !self.sixel && !self.iterm2
    }

    /// The protocols that stand in this set and in `other` as well.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Self {
        Self {
            kitty: self.kitty && other.kitty,
            sixel: self.sixel && other.sixel,
            iterm2: self.iterm2 && other.iterm2,
        }
    }

    /// The protocols of this set, as a reader of an error message reads them.
    ///
    /// The names come out in one order whatever order the variable stated, so
    /// two messages about the same set read the same. An empty set gives
    /// `none`, because a message that named an empty list would read as a
    /// message that lost its list.
    #[must_use]
    pub fn names(&self) -> String {
        let names: Vec<&str> = self.in_order().map(|(name, _)| name).collect();
        if names.is_empty() {
            EMPTY_SET_NAME.to_string()
        } else {
            names.join(", ")
        }
    }

    /// The one routine that this set prefers, or `None` for an empty set.
    ///
    /// Kitty stands first because it is the one protocol that reports a
    /// failure, so a picture that does not arrive says why. Sixel stands next
    /// because a terminal answers a query about it. iTerm2 stands last because
    /// it carries no query and answers nothing at all.
    pub(crate) fn preferred_routine(&self) -> Option<DisplayRoutine> {
        self.in_order().map(|(_, routine)| routine).next()
    }

    /// The set that holds one routine alone.
    pub(crate) fn of_routine(routine: DisplayRoutine) -> Self {
        Self {
            kitty: routine == DisplayRoutine::Kitty,
            sixel: routine == DisplayRoutine::Sixel,
            iterm2: routine == DisplayRoutine::Iterm2,
        }
    }

    /// Whether this set holds the protocol that `routine` writes.
    pub(crate) fn holds(&self, routine: DisplayRoutine) -> bool {
        match routine {
            DisplayRoutine::Kitty => self.kitty,
            DisplayRoutine::Sixel => self.sixel,
            DisplayRoutine::Iterm2 => self.iterm2,
        }
    }

    /// The protocols of this set, in the one order that this module states.
    ///
    /// [`ProtocolSet::names`] and [`ProtocolSet::preferred_routine`] both read
    /// this order, so a message and a choice never disagree about which
    /// protocol stands first.
    fn in_order(&self) -> impl Iterator<Item = (&'static str, DisplayRoutine)> + '_ {
        [
            (self.kitty, KITTY_NAME, DisplayRoutine::Kitty),
            (self.sixel, SIXEL_NAME, DisplayRoutine::Sixel),
            (self.iterm2, ITERM2_NAME, DisplayRoutine::Iterm2),
        ]
        .into_iter()
        .filter_map(|(held, name, routine)| held.then_some((name, routine)))
    }
}

/// What the environment of a mosh session says about the images it carries.
///
/// [`crate::Capabilities`] holds one of these for the run, and the gate of a
/// tool reads it from there with [`crate::Capabilities::session`] to decide
/// whether a picture can draw at all. A run fills it with
/// [`MoshImages::detect`], through [`crate::Capabilities::detect`]. A test
/// states it with [`MoshImages::from_env`], through
/// [`crate::Capabilities::in_session`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoshImages {
    transport: ProtocolSet,
    client: ProtocolSet,
}

impl MoshImages {
    /// Read the two variables from the environment of this process.
    #[must_use]
    pub fn detect() -> Self {
        Self::from_env(
            std::env::var(TRANSPORT_VARIABLE).ok().as_deref(),
            std::env::var(CLIENT_VARIABLE).ok().as_deref(),
        )
    }

    /// Read the two variables from values that the caller already holds.
    ///
    /// The capture stands apart from the reading of it, so that a test names
    /// the session it covers and holds no state of the machine that runs the
    /// test.
    ///
    /// # Arguments
    /// * `transport` - The value of `MOSH_IMAGES`, or `None` where the
    ///   environment carries none.
    /// * `client` - The value of `MOSH_CLIENT_IMAGES`, or `None` where the
    ///   environment carries none.
    #[must_use]
    pub fn from_env(transport: Option<&str>, client: Option<&str>) -> Self {
        Self {
            transport: transport.map(ProtocolSet::parse).unwrap_or_default(),
            client: client.map(ProtocolSet::parse).unwrap_or_default(),
        }
    }

    /// Whether the transport of this session carries an image at all.
    ///
    /// Only a mosh that draws images writes `MOSH_IMAGES`, so this answers no
    /// for an upstream mosh. The variable outlives the session that wrote it,
    /// so a yes here states what a transport carries and states nothing about
    /// which transport this session has. A caller reads the process tree for
    /// that.
    #[must_use]
    pub fn carries_images(&self) -> bool {
        !self.transport.is_empty()
    }

    /// The protocols that the transport of this session carries.
    #[must_use]
    pub fn transport(&self) -> ProtocolSet {
        self.transport
    }

    /// The protocols that the terminal of the user draws.
    #[must_use]
    pub fn client(&self) -> ProtocolSet {
        self.client
    }

    /// The protocols that this session delivers from end to end.
    ///
    /// A picture travels through the transport and then draws on the terminal
    /// of the user, so a protocol that only one of the two reads delivers
    /// nothing. A session that names no terminal of the user narrows nothing,
    /// because an absent name is no name of an empty set: the wrapper writes
    /// `MOSH_CLIENT_IMAGES` where the terminal answered, and a terminal that
    /// answered nothing still draws whatever its own name says it draws.
    #[must_use]
    pub fn delivers(&self) -> ProtocolSet {
        if self.client.is_empty() {
            self.transport
        } else {
            self.transport.intersect(&self.client)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The environment of a mosh that draws images states what the session
    /// delivers from end to end.
    ///
    /// The transport carries a set and the terminal of the user draws a set,
    /// and a picture needs a protocol that stands in both. A session that
    /// names no terminal of the user narrows nothing, and a session that names
    /// no transport carries nothing at all.
    #[test]
    fn the_environment_of_a_mosh_states_what_the_session_delivers() {
        let both = MoshImages::from_env(Some("kitty,sixel,iterm2"), Some("sixel"));
        assert!(
            both.carries_images(),
            "a mosh that names the protocols it carries carries images"
        );
        assert_eq!(
            both.delivers(),
            ProtocolSet::parse("sixel"),
            "a picture draws with a protocol that the transport carries and the terminal of the user draws"
        );

        let no_client = MoshImages::from_env(Some("kitty, SIXEL "), None);
        assert_eq!(
            no_client.delivers(),
            ProtocolSet::parse("sixel,kitty"),
            "a session that names no terminal of the user narrows nothing, and a name is read whatever its case and its space"
        );

        let upstream = MoshImages::from_env(None, Some("kitty"));
        assert!(
            !upstream.carries_images(),
            "only a mosh that draws images names the protocols it carries, so a session that names none is an upstream mosh"
        );
        assert!(
            upstream.delivers().is_empty(),
            "and an upstream mosh delivers no protocol at all"
        );

        assert!(
            !MoshImages::from_env(Some(""), None).carries_images(),
            "an empty value names an empty set, and an empty set is no promise that this session carries an image"
        );
    }

    /// A token that names no protocol this crate draws is dropped.
    ///
    /// The variable comes from a transport that this crate does not build, so
    /// a value it does not read has to leave the set alone rather than turn it
    /// into a promise this crate cannot keep.
    #[test]
    fn a_name_that_this_crate_does_not_draw_leaves_the_set_alone() {
        assert_eq!(
            ProtocolSet::parse("kitty,quicktime,,sixel"),
            ProtocolSet::parse("kitty,sixel"),
            "an unread name and an empty part both drop out of the set"
        );
        assert!(
            ProtocolSet::parse("quicktime").is_empty(),
            "a value that names nothing this crate draws names an empty set"
        );
        assert!(
            ProtocolSet::parse("").is_empty(),
            "and so does an empty value"
        );
    }

    /// A message about a set names the protocols of it in one order.
    #[test]
    fn a_message_names_the_protocols_of_a_set_in_one_order() {
        assert_eq!(ProtocolSet::parse("iterm2,kitty").names(), "kitty, iterm2");
        assert_eq!(ProtocolSet::parse("sixel").names(), "sixel");
        assert_eq!(
            ProtocolSet::default().names(),
            "none",
            "a message that named an empty list would read as a message that lost its list"
        );
    }

    /// The set states the one routine it prefers.
    #[test]
    fn the_set_states_the_one_routine_it_prefers() {
        assert_eq!(
            ProtocolSet::parse("iterm2,sixel,kitty").preferred_routine(),
            Some(DisplayRoutine::Kitty),
            "kitty reports a failure, so a picture that does not arrive says why"
        );
        assert_eq!(
            ProtocolSet::parse("iterm2,sixel").preferred_routine(),
            Some(DisplayRoutine::Sixel),
            "a terminal answers a query about sixel, where it answers nothing about iterm2"
        );
        assert_eq!(
            ProtocolSet::parse("iterm2").preferred_routine(),
            Some(DisplayRoutine::Iterm2)
        );
        assert_eq!(ProtocolSet::default().preferred_routine(), None);
    }

    /// One routine names one protocol, and the set of it holds that one alone.
    #[test]
    fn the_set_of_one_routine_holds_that_one_protocol() {
        assert_eq!(
            ProtocolSet::of_routine(DisplayRoutine::Kitty),
            ProtocolSet::parse("kitty")
        );
        assert_eq!(
            ProtocolSet::of_routine(DisplayRoutine::Sixel),
            ProtocolSet::parse("sixel")
        );
        assert_eq!(
            ProtocolSet::of_routine(DisplayRoutine::Iterm2),
            ProtocolSet::parse("iterm2")
        );
    }
}
