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
//! * `MOSH_IMAGE_BUDGETS` states what one picture can spend in each protocol
//!   that the transport carries. The transport cuts a picture above that cap,
//!   or it drops the picture and draws nothing at all, so a tool that writes a
//!   picture has to know the cap before it writes one byte.
//!
//! The first two hold a comma-separated list of `kitty`, `sixel` and `iterm2`.
//! The third holds a comma-separated list of `NAME=NUMBER` pairs, with those
//! same names in that same order, so a reader joins the two lists by name.
//!
//! **The three numbers do not count one unit.** Each one counts the stretch of
//! the escape sequence that carries one whole picture in its own protocol, and
//! one unit is one byte on the wire in every case. A reader that takes the
//! three for one number answers for the wrong protocol on two runs out of
//! three. See <https://github.com/timmattison/mosh-rs/issues/78>,
//! <https://github.com/timmattison/mosh-rs/issues/94> and
//! <https://github.com/timmattison/tools/issues/480>.
//!
//! **A variable that crosses a multiplexer outlives the session that wrote
//! it.** A user exports it by hand, and a tmux server or a Zellij server that a
//! mosh session started hands the whole environment of that session to every
//! pane it opens after the mosh session ends. So this module states what a
//! transport carries, and it states nothing about which transport this session
//! has. A caller reads the process tree for that, and it reads the two answers
//! together.

use crate::detect::DisplayRoutine;
use crate::draw::{PayloadBudget, ProtocolBudgets};

/// The environment variable that names what the transport of the session
/// carries.
const TRANSPORT_VARIABLE: &str = "MOSH_IMAGES";

/// The environment variable that names what the terminal of the user draws.
const CLIENT_VARIABLE: &str = "MOSH_CLIENT_IMAGES";

/// The environment variable that states what one picture can spend in each
/// protocol that the transport carries.
const BUDGETS_VARIABLE: &str = "MOSH_IMAGE_BUDGETS";

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoshImages {
    transport: ProtocolSet,
    client: ProtocolSet,
    budgets: ProtocolBudgets,
}

impl Default for MoshImages {
    /// A session that states nothing at all.
    ///
    /// [`crate::Capabilities`] derives [`Default`] and holds one of these, and
    /// [`crate::Capabilities::new`] states the terminal and no session. So
    /// this answer stands for every run that reads no environment, and it
    /// agrees with `MoshImages::from_env(None, None, None)`: no protocol, and
    /// the careful budget of [`PayloadBudget::MOSH`] for each of the three.
    ///
    /// The budgets are stated by hand because neither answer of a derive is
    /// correct here. A budget of zero holds no payload and draws no picture,
    /// and a budget with no limit sends a picture that a mosh drops.
    fn default() -> Self {
        Self::from_env(None, None, None)
    }
}

impl MoshImages {
    /// Read the three variables from the environment of this process.
    #[must_use]
    pub fn detect() -> Self {
        Self::from_env(
            std::env::var(TRANSPORT_VARIABLE).ok().as_deref(),
            std::env::var(CLIENT_VARIABLE).ok().as_deref(),
            std::env::var(BUDGETS_VARIABLE).ok().as_deref(),
        )
    }

    /// Read the three variables from values that the caller already holds.
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
    /// * `budgets` - The value of `MOSH_IMAGE_BUDGETS`, or `None` where the
    ///   environment carries none. A session that states no cap keeps the
    ///   careful budget of [`PayloadBudget::MOSH`] for each protocol.
    #[must_use]
    pub fn from_env(transport: Option<&str>, client: Option<&str>, budgets: Option<&str>) -> Self {
        Self {
            transport: transport.map(ProtocolSet::parse).unwrap_or_default(),
            client: client.map(ProtocolSet::parse).unwrap_or_default(),
            budgets: budgets.map_or(ProtocolBudgets::uniform(PayloadBudget::MOSH), |_raw| {
                ProtocolBudgets::uniform(PayloadBudget::MOSH)
            }),
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

    /// What one picture can spend in each protocol of this session.
    ///
    /// The transport states a cap for each protocol it carries, and a picture
    /// above that cap draws cut or draws not at all. A caller that writes a
    /// picture reads the budget of the protocol it writes, and it holds the
    /// picture under that number.
    ///
    /// A protocol that this session states no cap for keeps
    /// [`PayloadBudget::MOSH`], which is the careful number that this crate
    /// held before mosh stated its caps. An upstream mosh states no cap at
    /// all, and so does every mosh built before
    /// <https://github.com/timmattison/mosh-rs/issues/94>. See
    /// <https://github.com/timmattison/tools/issues/480>.
    #[must_use]
    pub fn budgets(&self) -> ProtocolBudgets {
        self.budgets
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

    /// The cap that a mosh of today states for the Kitty graphics protocol.
    ///
    /// The three caps stand here as the value of one session, and no test
    /// reads one of them as a number this crate knows. A test asserts against
    /// [`PayloadBudget::under_command_cap`] of the cap it stated, so a mosh
    /// that moves a cap moves the answer with it and no test goes stale the
    /// way the copy did.
    const KITTY_CAP: usize = 1_638_400;

    /// The cap that a mosh of today states for the Sixel protocol.
    const SIXEL_CAP: usize = 1_048_576;

    /// The cap that a mosh of today states for the iTerm2 protocol.
    const ITERM2_CAP: usize = 1_048_576;

    /// A mosh that states its caps gives the budget of each protocol.
    ///
    /// The caps travel in the environment for the same reason the protocol
    /// names do: a query cannot cross a multiplexer. This crate held a copy of
    /// them before mosh wrote them, and the copy went stale, which is the
    /// defect that <https://github.com/timmattison/tools/issues/480> reports.
    ///
    /// Each cap counts the command of the protocol together with the payload,
    /// so the budget of a protocol is the cap less the room that this crate
    /// keeps for the command.
    #[test]
    fn a_mosh_that_states_its_caps_gives_the_budget_of_each_protocol() {
        let stated = MoshImages::from_env(
            Some("kitty,sixel,iterm2"),
            None,
            Some(&format!(
                "kitty={KITTY_CAP},sixel={SIXEL_CAP},iterm2={ITERM2_CAP}"
            )),
        );
        assert_eq!(
            stated.budgets(),
            ProtocolBudgets::uniform(PayloadBudget::MOSH)
                .with_kitty(PayloadBudget::under_command_cap(KITTY_CAP))
                .with_sixel(PayloadBudget::under_command_cap(SIXEL_CAP))
                .with_iterm2(PayloadBudget::under_command_cap(ITERM2_CAP)),
            "each protocol takes the cap that the session states for it, less the room of the command"
        );
    }

    /// A protocol that no cap names keeps the careful number.
    ///
    /// An upstream mosh writes no such variable, and so does every mosh built
    /// before <https://github.com/timmattison/mosh-rs/issues/94>. The careful
    /// number is what keeps a picture drawing there, so an absent name leaves
    /// that protocol alone.
    #[test]
    fn a_protocol_that_no_cap_names_keeps_the_careful_number() {
        let kitty_alone =
            MoshImages::from_env(Some("kitty"), None, Some(&format!(" KITTY = {KITTY_CAP} ")));
        assert_eq!(
            kitty_alone.budgets(),
            ProtocolBudgets::uniform(PayloadBudget::MOSH)
                .with_kitty(PayloadBudget::under_command_cap(KITTY_CAP)),
            "a name is read whatever its case and its space, and the two protocols that the value does not name keep the careful number"
        );

        assert_eq!(
            MoshImages::from_env(Some("kitty,sixel,iterm2"), None, None).budgets(),
            ProtocolBudgets::uniform(PayloadBudget::MOSH),
            "a session that states no cap at all keeps the careful number for all three protocols"
        );
        assert_eq!(
            MoshImages::default().budgets(),
            ProtocolBudgets::uniform(PayloadBudget::MOSH),
            "and a run that reads no environment states the same three numbers"
        );
    }

    /// A pair that this crate cannot read leaves that protocol alone.
    ///
    /// The variable comes from a transport that this crate does not build. A
    /// name that this crate does not draw belongs to a later protocol, and a
    /// pair that carries no cap carries nothing to read, so both drop and that
    /// protocol keeps the careful number. A pair that does read still states
    /// its budget, so one bad pair costs one protocol and no more.
    #[test]
    fn a_pair_that_this_crate_cannot_read_leaves_that_protocol_alone() {
        let mixed = MoshImages::from_env(
            Some("kitty,sixel,iterm2"),
            None,
            Some(&format!("kitty={KITTY_CAP},quicktime=99,sixel,iterm2=lots")),
        );
        assert_eq!(
            mixed.budgets(),
            ProtocolBudgets::uniform(PayloadBudget::MOSH)
                .with_kitty(PayloadBudget::under_command_cap(KITTY_CAP)),
            "an unknown name, a pair with no cap and a cap that is no number all drop, and the one pair that reads states its budget"
        );
    }

    /// A cap that stands under the room of the command gives a budget of zero.
    ///
    /// A small cap is still the cap of that session, and this keeps it. A mosh
    /// that lowers a cap is the failure that
    /// <https://github.com/timmattison/tools/issues/480> guards against: a
    /// number above the real cap sends a picture that the transport drops, and
    /// the user reads an empty screen.
    #[test]
    fn a_cap_under_the_room_of_the_command_gives_a_budget_of_zero() {
        const SMALL_CAP: usize = 100;

        let small = MoshImages::from_env(Some("kitty"), None, Some(&format!("kitty={SMALL_CAP}")));
        assert_eq!(
            small.budgets().of_routine(DisplayRoutine::Kitty),
            PayloadBudget::under_command_cap(SMALL_CAP),
            "a stated cap is honored however small it is"
        );
        assert_ne!(
            small.budgets().of_routine(DisplayRoutine::Kitty),
            PayloadBudget::MOSH,
            "a cap under the room of the command falls to zero, and it never falls back to the careful number"
        );
    }

    /// The environment of a mosh that draws images states what the session
    /// delivers from end to end.
    ///
    /// The transport carries a set and the terminal of the user draws a set,
    /// and a picture needs a protocol that stands in both. A session that
    /// names no terminal of the user narrows nothing, and a session that names
    /// no transport carries nothing at all.
    #[test]
    fn the_environment_of_a_mosh_states_what_the_session_delivers() {
        let both = MoshImages::from_env(Some("kitty,sixel,iterm2"), Some("sixel"), None);
        assert!(
            both.carries_images(),
            "a mosh that names the protocols it carries carries images"
        );
        assert_eq!(
            both.delivers(),
            ProtocolSet::parse("sixel"),
            "a picture draws with a protocol that the transport carries and the terminal of the user draws"
        );

        let no_client = MoshImages::from_env(Some("kitty, SIXEL "), None, None);
        assert_eq!(
            no_client.delivers(),
            ProtocolSet::parse("sixel,kitty"),
            "a session that names no terminal of the user narrows nothing, and a name is read whatever its case and its space"
        );

        let upstream = MoshImages::from_env(None, Some("kitty"), None);
        assert!(
            !upstream.carries_images(),
            "only a mosh that draws images names the protocols it carries, so a session that names none is an upstream mosh"
        );
        assert!(
            upstream.delivers().is_empty(),
            "and an upstream mosh delivers no protocol at all"
        );

        assert!(
            !MoshImages::from_env(Some(""), None, None).carries_images(),
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
