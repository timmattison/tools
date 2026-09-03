//! Reading a plan drawn as a picture.
//!
//! A chain says one order, and a table of streams says one order for each
//! stream. A picture says the one thing that neither of them says: two streams
//! that join.
//!
//! ```text
//! #242 ──→ #247 ──┐
//!                 ├──→ #249  (gallery)
//! #246 ──→ #248 ──┘
//! ```
//!
//! The meaning of a picture is its geometry. So this module builds a grid of
//! the text and it walks the grid. A reader of tokens and a reader of lines
//! both miss the one thing the picture says, because the corner of the first
//! line and the corner of the third line reach the same bus and no line holds
//! both of them.
//!
//! # The four rules
//!
//! 1. A connector character is a wire, and it names the sides it touches. `─`
//!    touches left and right, `│` touches up and down, and `┐` touches left
//!    and down. A rightward arrowhead touches left and right, so a head keeps
//!    a wire connected rather than ends it.
//! 2. Two wires that face each other are one net. A net is thus a group of
//!    cells, and one flood fill finds it.
//! 3. A net has ports on its left and ports on its right. A port is the text
//!    that stands beyond a free end of the net, and the text of a port is one
//!    step.
//! 4. Every left port comes before every right port. The direction is left to
//!    right, always, and an arrowhead confirms the direction rather than sets
//!    it.
//!
//! # The grid counts display columns
//!
//! A picture is drawn by eye. A Japanese word inside a node takes two columns
//! for each of its characters, so a wire that stands under such a node stands
//! two columns further right for each one. The grid counts display columns for
//! that reason, and it never counts characters.
//!
//! # An ASCII wire needs a neighbor, and a word ends it
//!
//! A reader draws the same picture with `-`, `|`, `+`, and `>`, and prose
//! holds all four of those characters. So an ASCII spelling is a wire only
//! when a neighbor on a side it touches is a connector character as well. `a
//! 30-line window` holds no wire, because a digit and a letter stand beside
//! its hyphen.
//!
//! A neighbor alone is too little, because one character of prose stands
//! beside another: the two hyphens of `--hidden` answer for each other, and
//! every plan names a flag. So this reader reads the run and never the one
//! character. It walks the wire characters to each end of the run, and the run
//! draws no wire when a letter stands at that end. `(pass --hidden)` is a flag
//! and `well-known` is a word, and neither of them holds a wire. `#1-->#2`
//! holds one, because a digit and a `#` are no letters.
//!
//! A box-drawing character never stands inside a word, so it reads neither
//! test.
//!
//! A stroke that runs corner to corner needs a neighbor as well, and for the
//! same reason. A line of the picture carries prose in the text of its ports,
//! and `#249  (src/gallery)` is one port. So a stroke draws a diagonal only
//! when a neighbor on one of its four sides draws a wire, and this reader
//! refuses that stroke alone. Every spelling of the stroke reads this test,
//! the box-drawing ones as well, because a reader who draws with `╲` writes
//! prose in the same picture.
//!
//! # What this module claims
//!
//! A net claims the text when it names two steps or more and its cells stand
//! on more than one line. `#1 ──→ #2` on one line is a chain, and the chain
//! reader still answers it. The border of a box-drawn table touches no step at
//! all, so that table keeps its own reader.
//!
//! A picture of two streams that never join holds no such net, because every
//! wire of it stands on one line:
//!
//! ```text
//! #242 ──→ #247
//! #246 ──→ #248
//! ```
//!
//! It says the same thing the paste says, that two people start now, and a
//! chain reader answers it as one line of work. So the text is claimed as well
//! when two nets or more each join a step on their left to a step on their
//! right, those nets do not all stand on one line, one of them holds a
//! character of the box-drawing block, and no net of the text reaches a step
//! on one side and nothing on the other.
//!
//! Each of the three tests beside the count keeps a text the chain reader
//! answers today. The lines keep `#1 ──→ #2 ──→ #3` a chain, which holds two
//! such nets on the one line the reader wrote. The box-drawing character keeps
//! a page of prose out, because no word holds one and `→` stands in a sentence
//! as often as in a picture. The net that reaches nothing on one side keeps a
//! chain somebody wrapped out, because the `──→` at the end of the first line
//! says the order runs on.
//!
//! The price is a chain wrapped after a box-drawn wire. `#1 ──→ #2,` on one
//! line and `#3 ──→ #4` on the next reads as two streams, because nothing in
//! it says the second row continues the first.
//!
//! # A step with no wire beside it
//!
//! A line that holds no wire, and whose whole text reads as exactly one step,
//! is a node with no edge. `#6` alone is a stream of one step, and the answer
//! names it. Every other line with no wire is prose: `See #123 for the details`
//! writes no step, because `See` is no number.
//!
//! Such a node never claims the text. It is read after a net claims it, so a
//! number in the prose of a page that draws no picture still reaches the chain
//! reader.
//!
//! # What it refuses
//!
//! A claimed picture that this reader cannot follow is a refusal and never a
//! guess, because a guess sends somebody to the wrong issue. It refuses a
//! leftward arrowhead, a diagonal wire, a wire that reaches text which is not a
//! step, a net that reaches a step on one side and nothing on the other, and a
//! cycle. See [`GraphError`].
//!
//! A net with no port at all is dropped without a word. The border of a box
//! table touches no step, so it says nothing about an order.
//!
//! Every refusal stands after the claim. A text this reader does not claim
//! gives `None` and no message, whatever it is drawn with, because the chain
//! reader answers such a text next.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};
use std::ops::RangeInclusive;

use thiserror::Error;
use unicode_width::UnicodeWidthChar;

use crate::chain::{list, IssueNumber, Snippet};
use crate::plan::{one_step, Plan, Step};

/// The list a position outside the graph gives.
///
/// A named constant, because [`Graph::before`] gives a slice and an empty
/// `Vec` of its own would live no longer than the call.
const NO_POSITIONS: &[usize] = &[];

/// The number of step ports a net needs before it claims the text.
///
/// A net that names one step joins that step to nothing. Two is what makes an
/// edge, and an edge is the one thing this form carries that no other form
/// does.
const CLAIMING_PORTS: usize = 2;

/// The number of nets that join two steps a picture of streams needs.
///
/// One such net is a chain: `#1 ──→ #2` names the one order a chain names, and
/// the chain reader answers it. Two of them are two streams, and two streams
/// are the shape no chain writes.
const CLAIMING_NETS: usize = 2;

/// The block of characters that draw a box.
///
/// It holds every stroke, corner, tee, cross, and diagonal a reader draws a
/// picture with. No word holds one of them, so a character of this block is
/// the mark that somebody drew something rather than wrote a sentence.
const BOX_DRAWING: RangeInclusive<char> = '\u{2500}'..='\u{257f}';

/// The arrowheads that point right.
///
/// A head is a wire and not an end, so `──→` is one net that runs from the
/// first stroke to the point of the arrow. Each of these is one column wide,
/// as every wire character is.
const RIGHTWARD_HEADS: &[char] = &[
    '\u{2192}', // → RIGHTWARDS ARROW
    '\u{27f6}', // ⟶ LONG RIGHTWARDS ARROW
    '\u{21d2}', // ⇒ RIGHTWARDS DOUBLE ARROW
    '\u{279c}', // ➜ HEAVY ROUND-TIPPED RIGHTWARDS ARROW
    '\u{2794}', // ➔ HEAVY WIDE-HEADED RIGHTWARDS ARROW
    '>',        // the head of the ASCII arrow `-->`
];

/// The arrowheads that point left.
///
/// Not one of them is a wire. A picture drawn from right to left says the
/// opposite order, and this reader refuses such a picture rather than guessing
/// which order the reader means.
const LEFTWARD_HEADS: &[char] = &[
    '\u{2190}', // ← LEFTWARDS ARROW
    '\u{27f5}', // ⟵ LONG LEFTWARDS ARROW
    '\u{21d0}', // ⇐ LEFTWARDS DOUBLE ARROW
    '\u{25c0}', // ◀ BLACK LEFT-POINTING TRIANGLE
];

/// The strokes that run corner to corner.
///
/// A diagonal touches no side of a cell, so the rule that joins two wires
/// cannot read one. Prose holds both of the ASCII spellings — a path holds a
/// slash and an escape holds a backslash — and the text of a port carries such
/// prose. So a diagonal is a refusal when it stands beside a wire, and nothing
/// at all everywhere else.
const DIAGONALS: &[char] = &[
    '/', '\\',       // the ASCII spellings, which stand in a path as well
    '\u{2571}', // ╱ BOX DRAWINGS LIGHT DIAGONAL UPPER RIGHT TO LOWER LEFT
    '\u{2572}', // ╲ BOX DRAWINGS LIGHT DIAGONAL UPPER LEFT TO LOWER RIGHT
    '\u{2573}', // ╳ BOX DRAWINGS LIGHT DIAGONAL CROSS
];

/// The characters that draw a wire and stand inside prose as well.
///
/// Prose holds all four of them: a hyphen inside a word, a bar between two
/// words, a plus in a sum, and the `>` of a quotation. So each of them is a
/// wire only when a neighbor on a side it touches draws a wire as well, and it
/// draws no wire when the run it stands in ends at a letter: the two hyphens
/// of `--hidden` stand beside each other, and a flag is no wire.
/// [`Grid::sides_at`] holds both rules. `>` stands here and in
/// [`RIGHTWARD_HEADS`], because it is the head of the ASCII arrow and it is
/// one character of prose.
const ASCII_SPELLINGS: &[char] = &['-', '|', '+', '>'];

/// One of the four sides of a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// The side toward column zero.
    Left,
    /// The side away from column zero.
    Right,
    /// The side toward the first line.
    Up,
    /// The side away from the first line.
    Down,
}

impl Side {
    /// The four sides, to ask a wire about each of them.
    const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Up, Self::Down];

    /// The bit that stands for this side in a [`Sides`].
    const fn bit(self) -> u8 {
        match self {
            Self::Left => 1,
            Self::Right => 2,
            Self::Up => 4,
            Self::Down => 8,
        }
    }

    /// The side of the neighbor that faces this side.
    ///
    /// Two wires are one net when each of them touches the side that faces the
    /// other, so every join asks this question.
    const fn facing(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }

    /// The place next to `at` on this side, or `None` when that place is off
    /// the top or off the left of the grid.
    const fn from(self, at: Place) -> Option<Place> {
        let place = match self {
            Self::Left => Place {
                row: at.row,
                column: match at.column.checked_sub(1) {
                    Some(column) => column,
                    None => return None,
                },
            },
            Self::Right => Place {
                row: at.row,
                column: at.column + 1,
            },
            Self::Up => Place {
                row: match at.row.checked_sub(1) {
                    Some(row) => row,
                    None => return None,
                },
                column: at.column,
            },
            Self::Down => Place {
                row: at.row + 1,
                column: at.column,
            },
        };
        Some(place)
    }
}

/// The sides one wire touches.
///
/// A newtype over the four bits rather than four booleans, so a wire table
/// states one value for each character and a join is one test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sides(u8);

impl Sides {
    /// The sides of `─`.
    const HORIZONTAL: Self = Self(Side::Left.bit() | Side::Right.bit());
    /// The sides of `│`.
    const VERTICAL: Self = Self(Side::Up.bit() | Side::Down.bit());
    /// The sides of `┼`, which joins every neighbor it has.
    const ALL: Self = Self::HORIZONTAL.with(Self::VERTICAL);

    /// These sides and the sides of `other`.
    const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The sides of one named side.
    const fn of(side: Side) -> Self {
        Self(side.bit())
    }

    /// Does the wire touch `side`?
    const fn touches(self, side: Side) -> bool {
        self.0 & side.bit() != 0
    }
}

/// Where one cell of the picture stands.
///
/// The order is the order of the text: the row first, the column second. A
/// node takes the earliest place of the ports that name it, so this order is
/// what puts the nodes in the order a reader wrote them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Place {
    /// The line, counted from the first one.
    row: usize,
    /// The display column, counted from the left edge.
    column: usize,
}

/// The sides `c` touches, or `None` when `c` draws no wire.
///
/// This is the whole table, and it is read for the character alone. The rules
/// that an ASCII spelling needs a neighbor and ends at no letter stand in
/// [`Grid::sides_at`], because those rules read the grid and this table does
/// not.
fn sides_of(c: char) -> Option<Sides> {
    let sides = match c {
        // The whole box-drawing block, minus the three diagonals. Every
        // weight of one shape stands in one arm, because the light stroke and
        // the heavy stroke join the same neighbors, and so do the solid one,
        // the dashed ones, and the double one.
        //
        // The strokes.
        '\u{2500}' | '\u{2501}' | '\u{2504}' | '\u{2505}' | '\u{2508}' | '\u{2509}'
        | '\u{254c}' | '\u{254d}' | '\u{2550}' | '\u{257c}' | '\u{257e}' => Sides::HORIZONTAL, // ─ ━ ═
        '\u{2502}' | '\u{2503}' | '\u{2506}' | '\u{2507}' | '\u{250a}' | '\u{250b}'
        | '\u{254e}' | '\u{254f}' | '\u{2551}' | '\u{257d}' | '\u{257f}' => Sides::VERTICAL, // │ ┃ ║
        // The corners. The rounded corner of the light set stands beside the
        // square one it draws.
        '\u{250c}'..='\u{250f}' | '\u{2552}'..='\u{2554}' | '\u{256d}' => {
            Sides::of(Side::Right).with(Sides::of(Side::Down)) // ┌ ┏ ╔ ╭
        }
        '\u{2510}'..='\u{2513}' | '\u{2555}'..='\u{2557}' | '\u{256e}' => {
            Sides::of(Side::Left).with(Sides::of(Side::Down)) // ┐ ┓ ╗ ╮
        }
        '\u{2514}'..='\u{2517}' | '\u{2558}'..='\u{255a}' | '\u{2570}' => {
            Sides::of(Side::Up).with(Sides::of(Side::Right)) // └ ┗ ╚ ╰
        }
        '\u{2518}'..='\u{251b}' | '\u{255b}'..='\u{255d}' | '\u{256f}' => {
            Sides::of(Side::Up).with(Sides::of(Side::Left)) // ┘ ┛ ╝ ╯
        }
        // The tees.
        '\u{251c}'..='\u{2523}' | '\u{255e}'..='\u{2560}' => {
            Sides::VERTICAL.with(Sides::of(Side::Right)) // ├ ┣ ╠
        }
        '\u{2524}'..='\u{252b}' | '\u{2561}'..='\u{2563}' => {
            Sides::VERTICAL.with(Sides::of(Side::Left)) // ┤ ┫ ╣
        }
        '\u{252c}'..='\u{2533}' | '\u{2564}'..='\u{2566}' => {
            Sides::HORIZONTAL.with(Sides::of(Side::Down)) // ┬ ┳ ╦
        }
        '\u{2534}'..='\u{253b}' | '\u{2567}'..='\u{2569}' => {
            Sides::HORIZONTAL.with(Sides::of(Side::Up)) // ┴ ┻ ╩
        }
        // The crosses. A cross joins, so a picture that needs one wire to
        // cross another without a join cannot be drawn in this form.
        '\u{253c}'..='\u{254b}' | '\u{256a}'..='\u{256c}' => Sides::ALL, // ┼ ╋ ╬
        // The half lines, which draw the end of a stroke.
        '\u{2574}' | '\u{2578}' => Sides::of(Side::Left), // ╴ ╸
        '\u{2575}' | '\u{2579}' => Sides::of(Side::Up),   // ╵ ╹
        '\u{2576}' | '\u{257a}' => Sides::of(Side::Right), // ╶ ╺
        '\u{2577}' | '\u{257b}' => Sides::of(Side::Down), // ╷ ╻
        // U+2571 to U+2573 are the diagonals, and they draw no wire. A
        // diagonal has no side to face, so rule 2 cannot read it. The slice
        // that refuses a picture refuses one that holds a diagonal beside a
        // wire.
        head if RIGHTWARD_HEADS.contains(&head) => Sides::HORIZONTAL,
        // The ASCII spellings. Each of them is a wire only when a neighbor
        // says so and no letter ends its run, and [`Grid::sides_at`] holds
        // both rules.
        '-' => Sides::HORIZONTAL,
        '|' => Sides::VERTICAL,
        '+' => Sides::ALL,
        _ => return None,
    };
    Some(sides)
}

/// One display column of one line of the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    /// The character that starts in this column.
    Start(char),
    /// The second column of a character that takes two.
    ///
    /// It carries no character of its own. It is no space, so the text of a
    /// port that ends in a wide character ends there, and it draws no wire, so
    /// no net runs through the middle of a word.
    Tail,
}

/// One line of the picture, one cell for each display column.
type Row = Vec<Cell>;

/// The cells of `line`, one for each display column.
///
/// A character of width zero, and a character of no known width, takes one
/// column. Such a character can shift text and never a wire, because every
/// wire character takes one column.
fn row_of(line: &str) -> Row {
    let mut cells: Row = Vec::new();
    for c in line.chars() {
        cells.push(Cell::Start(c));
        for _ in 1..UnicodeWidthChar::width(c).unwrap_or(1).max(1) {
            cells.push(Cell::Tail);
        }
    }
    cells
}

/// The picture, as a grid of cells and the wire each of them draws.
struct Grid {
    /// The cells, indexed by line and then by display column.
    cells: Vec<Row>,
    /// The sides each cell touches, or `None` where the cell draws no wire.
    wires: Vec<Vec<Option<Sides>>>,
}

impl Grid {
    /// The grid `text` draws.
    fn new(text: &str) -> Self {
        let cells: Vec<Row> = text.lines().map(row_of).collect();
        let wires = cells
            .iter()
            .enumerate()
            .map(|(row, line)| {
                (0..line.len())
                    .map(|column| Self::sides_at(&cells, Place { row, column }))
                    .collect()
            })
            .collect();
        Self { cells, wires }
    }

    /// The character that starts at `at`, or `None` for the second column of a
    /// wide character and for a place off the grid.
    fn glyph(cells: &[Row], at: Place) -> Option<char> {
        match cells.get(at.row)?.get(at.column)? {
            Cell::Start(c) => Some(*c),
            Cell::Tail => None,
        }
    }

    /// The cell at `at`, or `None` when `at` is off the grid.
    fn cell(&self, at: Place) -> Option<Cell> {
        self.cells.get(at.row)?.get(at.column).copied()
    }

    /// The sides the cell at `at` touches, with the rules for an ASCII
    /// spelling applied.
    ///
    /// An ASCII spelling is a wire only when a neighbor on a side it touches
    /// draws a wire as well, and it draws no wire at all when the run it
    /// stands in ends at a letter. The neighbor alone is too little, because a
    /// hyphen stands beside a hyphen inside `--hidden` as much as inside
    /// `-->`. The run is what parts the two.
    ///
    /// The first test reads the neighbor out of the table alone, and the
    /// second walks that same table, so neither of them asks this question of
    /// another cell and neither recurses.
    fn sides_at(cells: &[Row], at: Place) -> Option<Sides> {
        let glyph = Self::glyph(cells, at)?;
        let sides = sides_of(glyph)?;
        if !ASCII_SPELLINGS.contains(&glyph) {
            return Some(sides);
        }
        let touched_sides = || Side::ALL.into_iter().filter(|&side| sides.touches(side));
        let beside_a_wire = touched_sides().any(|side| {
            side.from(at)
                .and_then(|next| Self::glyph(cells, next))
                .is_some_and(|neighbor| sides_of(neighbor).is_some())
        });
        let inside_a_word = touched_sides().any(|side| Self::run_ends_at_a_letter(cells, at, side));
        (beside_a_wire && !inside_a_word).then_some(sides)
    }

    /// Does the run of `at` along `side` end at a letter?
    ///
    /// A run of ASCII wire characters that touches a word is a word: `--hidden`
    /// is a flag and `30-line` is a width. So the walk steps over every cell
    /// the table of [`sides_of`] draws a wire for, and it reads the first cell
    /// that draws none. A box-drawing character never stands inside a word, so
    /// a run that holds one still ends where the wire ends.
    ///
    /// The edge of the grid holds no letter, and neither does the second
    /// column of a wide character. A run that ends at either of them is the
    /// wire a reader drew.
    fn run_ends_at_a_letter(cells: &[Row], at: Place, side: Side) -> bool {
        let mut place = at;
        while let Some(next) = side.from(place) {
            let Some(glyph) = Self::glyph(cells, next) else {
                return false;
            };
            if sides_of(glyph).is_none() {
                return glyph.is_alphabetic();
            }
            place = next;
        }
        false
    }

    /// The sides the cell at `at` touches, or `None` for a cell that draws no
    /// wire and for a place off the grid.
    fn sides(&self, at: Place) -> Option<Sides> {
        *self.wires.get(at.row)?.get(at.column)?
    }

    /// Is the cell at `at` a space?
    ///
    /// A place off the grid is no space. A walk that reaches one has left the
    /// line, and a walk that leaves the line finds no port.
    fn is_space(&self, at: Place) -> bool {
        Self::glyph(&self.cells, at).is_some_and(char::is_whitespace)
    }

    /// The text of the line at `row`, as the reader wrote it.
    ///
    /// Built back out of the cells, because a message about a line names the
    /// line the grid holds and no other text.
    fn line(&self, row: usize) -> String {
        self.cells
            .get(row)
            .into_iter()
            .flatten()
            .filter_map(|cell| match cell {
                Cell::Start(glyph) => Some(*glyph),
                Cell::Tail => None,
            })
            .collect()
    }

    /// Does the line at `row` hold a wire?
    ///
    /// A line with no wire on it is prose, and prose holds a slash inside a
    /// path and an arrow inside a phrase. So the refusals of the drawing read
    /// the lines of the picture and leave every other line alone.
    ///
    /// A line of the picture carries prose as well, in the text of each port
    /// it reaches. So the refusal of a stroke asks about a place inside such a
    /// line and never about the whole of it.
    fn is_drawn(&self, row: usize) -> bool {
        self.wires
            .get(row)
            .is_some_and(|line| line.iter().any(Option::is_some))
    }

    /// Does the cell at `at` draw a diagonal that belongs to the picture?
    ///
    /// A diagonal inside the prose of a port is a path or an escape, and it
    /// draws nothing. A diagonal beside a wire is a stroke the reader meant,
    /// and the rule that makes two wires one net cannot read it.
    fn is_drawn_diagonal(&self, at: Place) -> bool {
        let Some(glyph) = Self::glyph(&self.cells, at) else {
            return false;
        };
        DIAGONALS.contains(&glyph)
            && Side::ALL
                .into_iter()
                .any(|side| side.from(at).is_some_and(|next| self.sides(next).is_some()))
    }

    /// The refusal the drawing of the picture earns, or `Ok` when every line of
    /// it runs from left to right.
    ///
    /// # Errors
    ///
    /// Gives [`GraphError::Leftward`] for a line of the picture that holds an
    /// arrowhead which points left, and [`GraphError::Diagonal`] for a stroke
    /// which runs corner to corner beside a wire. A line with no wire on it
    /// draws nothing, so this reads neither question to it.
    ///
    /// The two questions differ, because the two characters differ. A head
    /// this reader drops is read into the text of a port, and a port that
    /// swallows a head draws an edge nobody wrote, so a head that points left
    /// is a refusal wherever it stands on a line of the picture. A stroke this
    /// reader drops costs nothing, because a path and an escape stand in the
    /// prose of a port every day.
    fn refuse_drawing(&self) -> Result<(), GraphError> {
        for row in (0..self.cells.len()).filter(|&row| self.is_drawn(row)) {
            let line = self.line(row);
            if line.chars().any(|glyph| LEFTWARD_HEADS.contains(&glyph)) {
                return Err(GraphError::Leftward(Snippet::new(&line)));
            }
            let columns = self.cells.get(row).map_or(0, Vec::len);
            if (0..columns).any(|column| self.is_drawn_diagonal(Place { row, column })) {
                return Err(GraphError::Diagonal(Snippet::new(&line)));
            }
        }
        Ok(())
    }

    /// Every step that stands on a line with no wire on it.
    ///
    /// A line that holds no wire draws nothing, so it joins no step to any
    /// other. It is a node all the same when the whole of it reads as one
    /// step, because a stream of one step is a plan and the reader who wrote
    /// it wants to hear about it. Every other such line is prose, and prose
    /// names no node.
    fn lone_steps(&self) -> Vec<Port> {
        let mut ports: Vec<Port> = Vec::new();
        for row in (0..self.cells.len()).filter(|&row| !self.is_drawn(row)) {
            let Some(last) = self.cells.get(row).map_or(0, Vec::len).checked_sub(1) else {
                continue;
            };
            let Some((text, place)) = self.read_text(row, 0, last) else {
                continue;
            };
            let Some(step) = one_step(&text) else {
                continue;
            };
            ports.push(Port {
                place,
                text,
                step: Some(step),
            });
        }
        ports
    }

    /// Every net of the picture, each one the places of its cells.
    ///
    /// The nets arrive in the order of their first cell, so every list this
    /// module builds out of them stands in the order of the text.
    fn nets(&self) -> Vec<Vec<Place>> {
        let mut seen: Vec<Vec<bool>> = self
            .cells
            .iter()
            .map(|row| vec![false; row.len()])
            .collect();
        let mut nets: Vec<Vec<Place>> = Vec::new();
        for (row, line) in self.cells.iter().enumerate() {
            for column in 0..line.len() {
                let at = Place { row, column };
                if seen[row][column] || self.sides(at).is_none() {
                    continue;
                }
                nets.push(self.net_from(at, &mut seen));
            }
        }
        nets
    }

    /// The net the cell at `start` belongs to.
    ///
    /// One flood fill, over the joins of rule 2: a wire joins the neighbor on
    /// a side it touches when that neighbor touches the side that faces back.
    fn net_from(&self, start: Place, seen: &mut [Vec<bool>]) -> Vec<Place> {
        let mut net: Vec<Place> = Vec::new();
        let mut queue: VecDeque<Place> = VecDeque::from([start]);
        seen[start.row][start.column] = true;
        while let Some(at) = queue.pop_front() {
            net.push(at);
            let Some(sides) = self.sides(at) else {
                continue;
            };
            for side in Side::ALL {
                if !sides.touches(side) {
                    continue;
                }
                let Some(next) = side.from(at) else { continue };
                if !self
                    .sides(next)
                    .is_some_and(|other| other.touches(side.facing()))
                {
                    continue;
                }
                if seen[next.row][next.column] {
                    continue;
                }
                seen[next.row][next.column] = true;
                queue.push_back(next);
            }
        }
        net
    }

    /// The text `net` reaches on each of its sides.
    fn wiring(&self, net: &[Place]) -> Wiring {
        let mut before: Vec<Port> = Vec::new();
        let mut after: Vec<Port> = Vec::new();
        for &at in net {
            let Some(sides) = self.sides(at) else {
                continue;
            };
            if sides.touches(Side::Left) && self.is_free_end(at, Side::Left) {
                before.extend(self.port(at, Side::Left));
            }
            if sides.touches(Side::Right) && self.is_free_end(at, Side::Right) {
                after.extend(self.port(at, Side::Right));
            }
        }
        let row = net.iter().map(|place| place.row).min();
        let spans_lines = net.iter().any(|place| Some(place.row) != row);
        let box_drawn = net
            .iter()
            .filter_map(|&at| Self::glyph(&self.cells, at))
            .any(|glyph| BOX_DRAWING.contains(&glyph));
        Wiring {
            before,
            after,
            spans_lines,
            row,
            box_drawn,
        }
    }

    /// Is `side` of the cell at `at` a free end of its net?
    ///
    /// A free end is where the net stops and the text of a port starts. A cell
    /// whose neighbor on that side faces back is inside the net.
    fn is_free_end(&self, at: Place, side: Side) -> bool {
        !side
            .from(at)
            .and_then(|next| self.sides(next))
            .is_some_and(|other| other.touches(side.facing()))
    }

    /// The step the picture names beyond the free end at `at`, or `None` when
    /// it names none.
    ///
    /// The walk steps over the spaces. It finds no port when it leaves the
    /// line, and no port when the first cell that is not a space draws a wire:
    /// two nets with nothing but spaces between them name nothing.
    ///
    /// This reading is lenient, and it must be. A port carries the text a
    /// reader wrote and the step that text names, and the step is `None` when
    /// the text names none. The claim comes before every refusal: a chain
    /// broken over two lines reaches this function, and it must reach the chain
    /// reader after it with no message of its own. [`refuse_ports`] names such
    /// a text with [`GraphError::NotAStep`], after a net of the same text has
    /// claimed it.
    fn port(&self, at: Place, side: Side) -> Option<Port> {
        let near = self.first_mark(at, side)?;
        if self.sides(near).is_some() {
            return None;
        }
        let far = self.text_edge(near, side);
        let (from, to) = match side {
            Side::Left => (far.column, near.column),
            _ => (near.column, far.column),
        };
        let (text, place) = self.read_text(at.row, from, to)?;
        let step = one_step(&text);
        Some(Port { place, text, step })
    }

    /// The first cell beyond `at` on `side` that is not a space, or `None`
    /// when the walk leaves the line.
    fn first_mark(&self, at: Place, side: Side) -> Option<Place> {
        let mut place = side.from(at)?;
        while self.is_space(place) {
            place = side.from(place)?;
        }
        self.cell(place).map(|_| place)
    }

    /// The far end of the text of a port that starts at `near`.
    ///
    /// The text runs to the cell before the nearest wire beyond it, or to the
    /// edge of the line. So `#1 ──→ #2 ──→ #3` gives `#2` to both of the nets
    /// that touch it, and `#249  (gallery)` is one port and not two.
    fn text_edge(&self, near: Place, side: Side) -> Place {
        let mut edge = near;
        while let Some(next) = side.from(edge) {
            if self.cell(next).is_none() || self.sides(next).is_some() {
                break;
            }
            edge = next;
        }
        edge
    }

    /// The text of the columns `from` to `to` of `row`, and the place its
    /// first character stands in.
    ///
    /// The space around the text is dropped, because a port is the words a
    /// reader wrote and never the space that lines the picture up. Gives
    /// `None` for a run of nothing but spaces.
    fn read_text(&self, row: usize, from: usize, to: usize) -> Option<(String, Place)> {
        let line = self.cells.get(row)?;
        let mut text = String::new();
        let mut start: Option<usize> = None;
        for column in from..=to {
            let Some(&cell) = line.get(column) else {
                break;
            };
            // The second column of a wide character carries no character of
            // its own, and the character that owns it already stands in the
            // text.
            let Cell::Start(glyph) = cell else { continue };
            if !glyph.is_whitespace() && start.is_none() {
                start = Some(column);
            }
            text.push(glyph);
        }
        start.map(|column| (text.trim().to_string(), Place { row, column }))
    }
}

/// The text a net reaches beyond one of its free ends.
///
/// It carries the text and not the step alone, because a text that names no
/// step is a refusal and a message names it back to the reader.
struct Port {
    /// Where the first character of the text stands.
    place: Place,
    /// The text, with the space around it dropped.
    text: String,
    /// The step the text names, or `None` when it names none.
    step: Option<Step>,
}

/// One net of the picture, and the text it reaches on each of its sides.
///
/// A net that reaches text on one side and nothing on the other stands here
/// with an empty list. It gives no edge, and [`refuse_ports`] refuses it.
struct Wiring {
    /// The text the net reaches on its left. Each step of it comes before each
    /// step of `after`.
    before: Vec<Port>,
    /// The text the net reaches on its right.
    after: Vec<Port>,
    /// The cells of the net stand on more than one line.
    spans_lines: bool,
    /// The first line the cells of the net stand on, or `None` when the net
    /// holds no cell.
    ///
    /// [`streams_claim`] reads it, and every net it reads stands on one line
    /// alone: a net that joins two steps and spans the lines claims the text
    /// by itself, through [`Wiring::claims`].
    row: Option<usize>,
    /// A cell of the net holds a character of the box-drawing block.
    box_drawn: bool,
}

impl Wiring {
    /// Every port of the net, the left ones first.
    fn ports(&self) -> impl Iterator<Item = &Port> {
        self.before.iter().chain(&self.after)
    }

    /// Does this net claim the text for the reader of pictures on its own?
    ///
    /// It claims the text when it names two steps or more and its cells stand
    /// on more than one line. The second half is what keeps `#1 ──→ #2` a
    /// chain: both of its steps stand on one line, and the chain reader
    /// answers such a text today.
    ///
    /// A port whose text names no step counts for nothing here. A picture is
    /// claimed by the work it joins, so a text this reader claims is a text
    /// that draws at least one edge.
    ///
    /// This is one of the two claims. A picture of streams that never join
    /// holds no net that spans the lines, and [`streams_claim`] reads the nets
    /// of the whole text together to find one.
    fn claims(&self) -> bool {
        self.spans_lines
            && self.ports().filter(|port| port.step.is_some()).count() >= CLAIMING_PORTS
    }

    /// Does the net reach a step on its left and a step on its right?
    ///
    /// Such a net draws at least one edge, so it is one row of work. A text of
    /// two of them or more says two rows, and that is what no chain says.
    fn joins_two_steps(&self) -> bool {
        names_a_step(&self.before) && names_a_step(&self.after)
    }

    /// Does the net reach a step on one side and nothing at all on the other?
    ///
    /// A chain somebody broke over two lines ends its first line with such a
    /// net: the `──→` of `#1 ──→ #2 ──→` reaches `#2` on its left and the end
    /// of the line on its right, and it says that the order runs on. So a text
    /// that holds one is one chain, however many rows of work stand in it.
    ///
    /// A net that reaches text on both sides is no such net, whether that text
    /// names a step or not. [`refuse_ports`] reads that text later, after a
    /// claim, and names it back to the reader.
    fn is_half(&self) -> bool {
        (self.before.is_empty() && names_a_step(&self.after))
            || (self.after.is_empty() && names_a_step(&self.before))
    }
}

/// Does one port of `ports` name a step?
fn names_a_step(ports: &[Port]) -> bool {
    ports.iter().any(|port| port.step.is_some())
}

/// Do the nets of the text draw streams that never join?
///
/// Two rows of work with no bus between them say the one thing the paste says:
/// two people start now. Every wire of such a picture stands on one line, so
/// no net of it claims the text through [`Wiring::claims`], and the chain
/// reader answers the whole page as one line of work. That answer names one
/// issue where two are ready.
///
/// So the text is claimed when two nets or more each join a step on their left
/// to a step on their right, and three tests stand beside that count. Each of
/// them keeps a text the chain reader answers today.
///
/// The lines keep a chain a chain. `#1 ──→ #2 ──→ #3` holds two nets that each
/// join two steps, and both of them stand on the one line the reader wrote.
///
/// A character of the box-drawing block keeps a page of prose out. `This
/// depends on #1 → #2.` on one line and `Also see #3 → #4.` on another names
/// issues on two lines and draws nothing. No word holds a character of that
/// block, so one of them is the mark that somebody drew something.
///
/// A net that reaches a step on one side and nothing on the other keeps a
/// wrapped chain out. `#1 ──→ #2 ──→` on one line and `#3 ──→ #4` on the next
/// is one order somebody broke in two, and the trailing `──→` says so. The
/// test reads every net of the text, because the net that says it stands
/// beside the rows and not inside one of them.
///
/// The price is a chain wrapped after a box-drawn wire. `#1 ──→ #2,` on one
/// line and `#3 ──→ #4` on the next reads as two streams, because nothing in
/// it says the second row continues the first.
fn streams_claim(wirings: &[Wiring]) -> bool {
    let joining: Vec<&Wiring> = wirings
        .iter()
        .filter(|wiring| wiring.joins_two_steps())
        .collect();
    joining.len() >= CLAIMING_NETS
        && joining
            .first()
            .is_some_and(|first| joining.iter().any(|wiring| wiring.row != first.row))
        && joining.iter().any(|wiring| wiring.box_drawn)
        && !wirings.iter().any(Wiring::is_half)
}

/// How far a walk of the wires has gone with one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// The walk has not reached the node.
    New,
    /// The node stands on the path the walk holds now.
    OnPath,
    /// The walk left the node, and every step after it.
    Done,
}

/// The steps a picture names, and the steps that come before each of them.
pub struct Graph {
    /// The steps, one for each node of the picture.
    steps: Vec<Step>,
    /// For each step, the positions of the steps that come before it.
    before: Vec<Vec<usize>>,
}

impl Graph {
    /// The steps of the picture, in a topological order.
    ///
    /// Every step stands after each step it waits for, and a tie goes to the
    /// step that stands first in the text. So the rows of an answer keep each
    /// stream of the picture together, and a reader reads the streams the way
    /// they wrote them.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// The positions of the steps that come before the step at `position`.
    ///
    /// The positions count in [`steps`](Self::steps), and they stand in
    /// ascending order. Every one of them is smaller than `position`, because
    /// the steps stand in a topological order.
    ///
    /// A position outside the graph gives an empty list, because a caller that
    /// walks the steps of another graph asks a question about nothing.
    #[must_use]
    pub fn before(&self, position: usize) -> &[usize] {
        self.before
            .get(position)
            .map_or(NO_POSITIONS, Vec::as_slice)
    }

    /// The positions of the steps that come after the step at each position.
    ///
    /// A [`Graph`] holds the steps before each step, because that is the
    /// question a row of the answer asks. A walk of the wires asks the other
    /// question, so it turns the lists around one time and reads them many
    /// times.
    fn after(&self) -> Vec<Vec<usize>> {
        let mut after: Vec<Vec<usize>> = vec![Vec::new(); self.steps.len()];
        for (position, before) in self.before.iter().enumerate() {
            for &earlier in before {
                after[earlier].push(position);
            }
        }
        after
    }

    /// This graph, or the cycle that keeps it from naming a step to start.
    ///
    /// # Errors
    ///
    /// Gives [`GraphError::Cycle`] with the numbers of one cycle.
    fn refuse_cycle(self) -> Result<Self, GraphError> {
        match self.cycle() {
            Some(cycle) => Err(GraphError::Cycle(cycle)),
            None => Ok(self),
        }
    }

    /// This graph, with its steps in a topological order.
    ///
    /// Kahn's algorithm over a queue that always takes the ready step with the
    /// earliest place in the text. A step is ready when every step before it
    /// stands in the order already, so a tie between two streams goes to the
    /// stream the reader wrote first and each stream stays together.
    ///
    /// The positions of the steps before each step are read against the new
    /// numbering, because a caller reads them as places of
    /// [`steps`](Self::steps).
    ///
    /// It runs after [`refuse_cycle`](Self::refuse_cycle), so every step
    /// reaches the queue: a step that never becomes ready is a step of a cycle,
    /// and a picture with a cycle never reaches this function.
    fn in_topological_order(self) -> Self {
        let after = self.after();
        let mut waiting: Vec<usize> = self.before.iter().map(Vec::len).collect();
        let mut ready: BinaryHeap<Reverse<usize>> = (0..self.steps.len())
            .filter(|position| waiting[*position] == 0)
            .map(Reverse)
            .collect();
        let mut order: Vec<usize> = Vec::with_capacity(self.steps.len());
        while let Some(Reverse(position)) = ready.pop() {
            order.push(position);
            for &next in &after[position] {
                waiting[next] -= 1;
                if waiting[next] == 0 {
                    ready.push(Reverse(next));
                }
            }
        }
        let mut rank: Vec<usize> = vec![0; self.steps.len()];
        for (place, &position) in order.iter().enumerate() {
            rank[position] = place;
        }
        let steps: Vec<Step> = order.iter().map(|&position| self.steps[position]).collect();
        let before: Vec<Vec<usize>> = order
            .iter()
            .map(|&position| {
                let mut earlier: Vec<usize> = self.before[position]
                    .iter()
                    .map(|&earlier| rank[earlier])
                    .collect();
                earlier.sort_unstable();
                earlier
            })
            .collect();
        Self { steps, before }
    }

    /// The numbers of one cycle of the picture, or `None` when the picture
    /// holds none.
    ///
    /// A depth first walk, and it names the one cycle that walk finds. A
    /// topological sort names every node it could not put in an order instead,
    /// and for one knot of two steps in a picture of twenty that is eighteen
    /// numbers a reader must read past to reach the two that hold the knot.
    ///
    /// The walk carries its own path rather than the stack of the machine,
    /// because a picture arrives from a clipboard and a clipboard holds a page.
    fn cycle(&self) -> Option<Vec<IssueNumber>> {
        let after = self.after();
        let mut marks: Vec<Mark> = vec![Mark::New; self.steps.len()];
        // The nodes of the path, and for each of them the place of the step to
        // walk to next.
        let mut path: Vec<usize> = Vec::new();
        let mut walk: Vec<(usize, usize)> = Vec::new();
        for start in 0..self.steps.len() {
            if marks[start] != Mark::New {
                continue;
            }
            marks[start] = Mark::OnPath;
            path.push(start);
            walk.push((start, 0));
            while let Some((node, next)) = walk.pop() {
                let Some(&successor) = after[node].get(next) else {
                    // Every step after this node is walked, so the node leaves
                    // the path. It stands last, because each node the walk
                    // pushed after it left the path already.
                    marks[node] = Mark::Done;
                    path.pop();
                    continue;
                };
                walk.push((node, next + 1));
                match marks[successor] {
                    // The path holds the successor, so the wires return to it.
                    // The cycle is the tail of the path from that node on.
                    Mark::OnPath => {
                        if let Some(from) = path.iter().position(|&node| node == successor) {
                            return Some(
                                path[from..]
                                    .iter()
                                    .map(|&node| self.steps[node].number())
                                    .collect(),
                            );
                        }
                    }
                    // A node the walk already left reaches no node of the path,
                    // so it opens no cycle.
                    Mark::Done => {}
                    Mark::New => {
                        marks[successor] = Mark::OnPath;
                        path.push(successor);
                        walk.push((successor, 0));
                    }
                }
            }
        }
        None
    }

    /// Every number the picture names, once, in the order of the steps.
    ///
    /// The number of a step comes before the number the step closes, because
    /// the pull request is the work and the issue is what the work finishes.
    /// A number that stands twice in the picture is one node, so it arrives
    /// once and one query to GitHub answers the whole picture. This is the
    /// rule `Plan::numbers` states for a plan, and a graph states it the same
    /// way so one query answers either shape.
    #[must_use]
    pub fn numbers(&self) -> Vec<IssueNumber> {
        let mut numbers: Vec<IssueNumber> = Vec::new();
        for step in &self.steps {
            for number in [Some(step.number()), step.closes()].into_iter().flatten() {
                if !numbers.contains(&number) {
                    numbers.push(number);
                }
            }
        }
        numbers
    }
}

/// Why a picture is not a graph.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GraphError {
    /// The text a wire reaches names no step.
    ///
    /// A stream label beside a wire is a plan this form does not carry, so the
    /// reader names the text rather than dropping the wire that reaches it.
    #[error("{0:?} stands beside a wire and is not a step, and a picture joins steps only")]
    NotAStep(Snippet),
    /// A net reaches a step on one side and nothing on the other.
    ///
    /// The message names the step the net has, because that step is the half
    /// of the order the reader wrote and the other half is what nobody can
    /// guess.
    #[error("{0:?} stands beside a wire that reaches no second step, and a wire joins two steps")]
    HalfNet(Snippet),
    /// A line of the picture holds an arrowhead that points left.
    ///
    /// A picture drawn from right to left is rare, and a guess at it is worse
    /// than a refusal: the reader who drew it means the opposite order, and an
    /// answer that names the last issue first sends somebody to the wrong work.
    #[error("{0:?} holds a leftward arrowhead, and this reader follows a wire from left to right")]
    Leftward(Snippet),
    /// A stroke that runs corner to corner stands beside a wire.
    ///
    /// A diagonal touches no side of a cell, so the rule that makes two wires
    /// one net cannot read it. The reader refuses the line rather than dropping
    /// the wire, because a dropped wire is an order the answer loses. A stroke
    /// with no wire beside it stands in the prose of a port, where it draws
    /// nothing and earns no refusal.
    #[error("{0:?} holds a diagonal wire, and a diagonal touches no side of a cell")]
    Diagonal(Snippet),
    /// The wires of the picture return to a step that comes before them.
    ///
    /// The numbers are the steps of one real cycle, in the order a walk of the
    /// wires meets them. A cycle has no step to start, and an answer of
    /// "nothing is ready" hides the reason, so the message names the steps that
    /// wait for each other.
    #[error("the wires return to {}, so this picture names no step to start first", list(.0))]
    Cycle(Vec<IssueNumber>),
}

/// The graph `text` draws, or `None` when `text` draws none.
///
/// The claim and the read share all of their work, so one function does both.
/// A text this reader does not claim gives `None` and no message, because the
/// chain reader takes such a text next: a chain broken over two lines must
/// reach that reader and not a refusal of this one.
///
/// Two rules claim, and either one of them is enough. One net that names two
/// steps and stands on more than one line claims the text, through
/// [`Wiring::claims`]. A text of two nets or more that each join two steps
/// claims it as a picture of streams that never join, through
/// [`streams_claim`], which reads the nets of the whole text together.
///
/// Most nodes of the graph come out of a net. A step that stands on a line with
/// no wire is a node with no edge, and it is read after a net claims the text,
/// so it never claims the text on its own.
///
/// # Errors
///
/// Gives the refusals of [`GraphError`] for a picture this reader claims and
/// cannot read. They stand between the claim and the graph: a leftward
/// arrowhead, a diagonal wire, a port whose text is not a step, a net with a
/// port on one side and nothing on the other, and a cycle. The drawing is read
/// first, because a head that points the wrong way is what makes the text
/// beside it read wrong, and the cycle is read last, because it is a question
/// about the graph and not about the picture.
pub fn read(text: &str) -> Option<Result<Graph, GraphError>> {
    let grid = Grid::new(text);
    // A net with no port at all is dropped without a word. The border of a box
    // table touches no step, so it carries no edge and it names no node.
    let wirings: Vec<Wiring> = grid
        .nets()
        .iter()
        .map(|net| grid.wiring(net))
        .filter(|wiring| !wiring.before.is_empty() || !wiring.after.is_empty())
        .collect();
    if !wirings.iter().any(Wiring::claims) && !streams_claim(&wirings) {
        return None;
    }
    Some(
        grid.refuse_drawing()
            .and_then(|()| refuse_ports(&wirings))
            .and_then(|()| build(&wirings, &grid.lone_steps()).refuse_cycle())
            .map(Graph::in_topological_order),
    )
}

/// The refusal the nets of a claimed picture earn, or `Ok` when every net joins
/// steps alone.
///
/// It runs over every net of the picture and not over the claiming net alone.
/// One net claims the text, and the whole text is then a picture: a wire that
/// reaches a label somewhere else in it draws an edge the graph loses, and a
/// reader who wrote that label meant work by it.
///
/// The nets arrive in the order of their first cell, and each net names the
/// earliest port it holds. So one picture earns the same message every time,
/// and the message names a text near the top of it.
fn refuse_ports(wirings: &[Wiring]) -> Result<(), GraphError> {
    for wiring in wirings {
        if let Some(port) = wiring
            .ports()
            .filter(|port| port.step.is_none())
            .min_by_key(|port| port.place)
        {
            return Err(GraphError::NotAStep(Snippet::new(&port.text)));
        }
        // A net with no port at all never reaches this function, so the first
        // port of a net that is short on one side is the port it has.
        if wiring.before.is_empty() || wiring.after.is_empty() {
            if let Some(port) = wiring.ports().min_by_key(|port| port.place) {
                return Err(GraphError::HalfNet(Snippet::new(&port.text)));
            }
        }
    }
    Ok(())
}

/// The graph the nets of a picture draw.
///
/// A node is keyed on the number of its step, so a number that stands twice in
/// the picture is one node that carries the edges of both places. The step of
/// the earliest place stands, because a reader who wrote the pair of a step
/// once wrote it where the step first appears.
///
/// One edge stands between two nodes, however many nets draw it. A step that
/// two rows of a bus reach comes before the step on the other side of that bus
/// one time, and a list that names it twice would tell a reader to wait for it
/// twice.
///
/// `lone` holds the steps that stand on a line with no wire. Each of them is a
/// node with no edge, and a number that stands in the picture as well is the
/// same one node.
fn build(wirings: &[Wiring], lone: &[Port]) -> Graph {
    let mut ports: Vec<(&Port, Step)> = wirings
        .iter()
        .flat_map(Wiring::ports)
        .chain(lone)
        .filter_map(|port| port.step.map(|step| (port, step)))
        .collect();
    ports.sort_by_key(|(port, _)| port.place);
    let mut nodes: BTreeMap<IssueNumber, (Place, Step)> = BTreeMap::new();
    for (port, step) in ports {
        nodes.entry(step.number()).or_insert((port.place, step));
    }
    let mut order: Vec<(Place, Step)> = nodes.into_values().collect();
    order.sort_by_key(|(place, _)| *place);
    let steps: Vec<Step> = order.iter().map(|(_, step)| *step).collect();
    let positions: BTreeMap<IssueNumber, usize> = steps
        .iter()
        .enumerate()
        .map(|(position, step)| (step.number(), position))
        .collect();

    let mut before: Vec<Vec<usize>> = vec![Vec::new(); steps.len()];
    for wiring in wirings {
        for after in wiring.after.iter().filter_map(|port| port.step) {
            let Some(&to) = positions.get(&after.number()) else {
                continue;
            };
            for earlier in wiring.before.iter().filter_map(|port| port.step) {
                let Some(&from) = positions.get(&earlier.number()) else {
                    continue;
                };
                if !before[to].contains(&from) {
                    before[to].push(from);
                }
            }
        }
    }
    // A step that reaches itself stays, so [`Graph::cycle`] finds it. A build
    // that dropped it would hand back a picture with the knot taken out of it,
    // and the answer would then name a step nobody can start.
    for positions in &mut before {
        positions.sort_unstable();
    }
    Graph { steps, before }
}

/// The graph a plan of streams draws, or `None` when it draws none.
// The command line is the caller, and the command line is the half of issue
// #436 that lands next. The expectation itself fails on the day the run calls
// this, so it goes then and nobody has to remember it. The tests call it
// already, which is why the attribute stands outside a test build.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the run that answers a plan is the caller, and it lands next"
    )
)]
pub fn of_plan(plan: &Plan) -> Option<Result<Graph, GraphError>> {
    let _ = plan;
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::BOX_TABLE;

    /// The paste of issue #418.
    ///
    /// Two streams that join. The first line and the third line reach the same
    /// bus, and no reader that walks tokens or lines sees that.
    const PASTE: &str = "\
#242 ──→ #247 ──┐
                ├──→ #249  (gallery)
#246 ──→ #248 ──┘";

    /// The same picture, with the bus drawn taller.
    ///
    /// A `│` stands between the corner and the tee, and between the tee and
    /// the corner under it. The plan is the same plan, so the graph is the
    /// same graph.
    const TALL_PASTE: &str = "\
#242 ──→ #247 ──┐
                │
                ├──→ #249  (gallery)
                │
#246 ──→ #248 ──┘";

    /// The fan-out of issue #418.
    ///
    /// One net with one port on its left and two on its right. The same four
    /// rules read it, because every left port comes before every right port
    /// whichever side holds more of them.
    const FAN_OUT: &str = "\
#1 ──┬──→ #2
     └──→ #3";

    /// `picture`, drawn with `+`, `-`, `|`, and `>`.
    ///
    /// Built out of the light form rather than typed a second time, so the two
    /// hold one picture and a test that reads them apart reads the drawing and
    /// never the plan.
    fn ascii(picture: &str) -> String {
        picture
            .chars()
            .map(|c| match c {
                '─' => '-',
                '│' => '|',
                '┌' | '┐' | '└' | '┘' | '├' | '┤' | '┬' | '┴' | '┼' => '+',
                '→' => '>',
                other => other,
            })
            .collect()
    }

    /// Two streams that join, drawn in ASCII with no space around a wire.
    ///
    /// Every run of this picture ends at a digit or at a `#`, and neither of
    /// those is a letter. So the rule that keeps a word out of the picture
    /// leaves this drawing alone, and a reader who lines a picture up tight
    /// keeps every edge.
    const TIGHT_ASCII: &str = "\
#1-->#2--+
         +-->#4
#3-->#5--+";

    /// Each light character, and the heavy and the double character that draw
    /// the same corner.
    const SETS: [(char, char, char); 11] = [
        ('─', '━', '═'),
        ('│', '┃', '║'),
        ('┌', '┏', '╔'),
        ('┐', '┓', '╗'),
        ('└', '┗', '╚'),
        ('┘', '┛', '╝'),
        ('├', '┣', '╠'),
        ('┤', '┫', '╣'),
        ('┬', '┳', '╦'),
        ('┴', '┻', '╩'),
        ('┼', '╋', '╬'),
    ];

    /// `picture`, drawn with the heavy set.
    fn heavy(picture: &str) -> String {
        redrawn(picture, |set| set.1)
    }

    /// `picture`, drawn with the double set.
    fn double(picture: &str) -> String {
        redrawn(picture, |set| set.2)
    }

    /// `picture`, with each light character replaced by the one `pick` names.
    fn redrawn(picture: &str, pick: fn(&(char, char, char)) -> char) -> String {
        picture
            .chars()
            .map(|c| SETS.iter().find(|set| set.0 == c).map_or(c, pick))
            .collect()
    }

    /// A plan of parallel work, as a Markdown table.
    ///
    /// The divider of such a table is a run of hyphens, and a hyphen is a wire
    /// beside another hyphen. The run reaches no step for all that, because a
    /// bar of the table stands between it and every cell.
    const MARKDOWN_TABLE: &str = "\
| Stream | Order | Zone | Notes |
| --- | --- | --- | --- |
| A | #1 → #2 | src/a | The two hunks sit 265 lines apart in a 5113-line file. |
| B | #3 | src/b | One issue, no neighbors. |";

    /// Two streams that never join.
    ///
    /// Two rows of work, each one a wire between two steps, and no bus
    /// anywhere. The picture says that two people start at the same moment, as
    /// much as the paste does, and no net of it stands on more than one line.
    const TWO_STREAMS: &str = "\
#242 ──→ #247
#246 ──→ #248";

    /// A chain somebody broke over two lines.
    ///
    /// The `──→` at the end of the first line reaches `#2` on its left and the
    /// end of the line on its right, so the order runs on. That net is what
    /// parts a wrapped chain from two streams that never join.
    const WRAPPED_CHAIN: &str = "\
#1 ──→ #2 ──→
#3 ──→ #4";

    /// A page of prose that names four issues on two lines.
    ///
    /// Two arrows and two lines, and a page all the same. The text beside each
    /// arrow is a sentence, so no port of it names a step.
    const PROSE_PAGE: &str = "\
This depends on #1 → #2.
Also see #3 → #4.";

    /// The same two lines, with a bare step on each side of each arrow.
    ///
    /// Every port of this text names a step, and the two nets stand on two
    /// lines, so the box-drawing character is the one test it fails. `→`
    /// stands in a sentence as often as in a picture, and `─` stands in a
    /// picture alone.
    const BARE_ARROWS: &str = "\
#1 → #2
#3 → #4";

    /// A node whose text holds wide characters, and the bus under it.
    ///
    /// `日本語` takes three characters and six display columns. The corner of
    /// the first line stands over the tee of the second line by display
    /// column, so the two are one net. A grid that counted characters would
    /// stand that corner three columns to the left of the tee, and the bus
    /// would break in two.
    const WIDE_NODE: &str = "\
#1 (日本語) ──┐
              ├──→ #3
#2 ───────────┘";

    /// The same picture, drawn with characters of one column each.
    ///
    /// Every cell of it stands in the same display column as the picture over
    /// it, so the two draw one graph. It stands here to say which rule is at
    /// work: the drawing did not change, and the width of the characters
    /// inside one node did.
    const PLAIN_NODE: &str = "\
#1 (abcdef) ──┐
              ├──→ #3
#2 ───────────┘";

    /// A picture that names a stream with a letter.
    ///
    /// The bus of the second line joins `#4`, `#5`, and `#6`, so a net of the
    /// picture claims the text. The wire of the first line reaches `A` on its
    /// left, and `A` is a label and not a step.
    const LABEL_PORT: &str = "\
A ──→ #4 ──┐
#5 ────────┴──→ #6";

    /// A picture whose last wire reaches nothing on its right.
    ///
    /// The fan-out of the first two lines claims the text. The wire of the
    /// third line reaches `#5` on its left and the end of the line on its
    /// right, so it says that `#5` comes before nothing.
    const HALF_NET: &str = "\
#1 ──┬──→ #3
     └──→ #4
#5 ──→";

    /// A fan-out that claims the text, to stand over a line under test.
    ///
    /// Two of its three ports name a step and its cells stand on two lines, so
    /// this net alone makes the text a picture. The line under it is then read
    /// as a line of that picture.
    const CLAIMING_FAN: &str = "\
#1 ──┬──→ #3
     └──→ #4";

    /// [`CLAIMING_FAN`], with `line` under it.
    fn under_a_picture(line: &str) -> String {
        format!("{CLAIMING_FAN}\n{line}")
    }

    /// A picture whose wires return to the step they left.
    ///
    /// A wire runs from left to right, always, so a cycle is drawn by a number
    /// that stands twice: `#1` comes before `#2` on the first line, and `#2`
    /// comes before `#1` on the third. The bus hangs `#4` under both of them,
    /// so the picture holds a node the cycle does not.
    const CYCLE_OF_TWO: &str = "\
#1 ──→ #2 ──┐
            ├──→ #4
#2 ──→ #1 ──┘";

    /// The same shape, with a third step inside the cycle.
    const CYCLE_OF_THREE: &str = "\
#1 ──→ #2 ──→ #3 ──┐
                   ├──→ #5
#3 ──→ #1 ─────────┘";

    /// A step the picture joins to itself.
    ///
    /// The shortest cycle there is. The bus makes the text a picture, so the
    /// wire between the two `#1` reaches the reader of a graph.
    const SELF_EDGE: &str = "\
#1 ──→ #1 ──┐
            └──→ #3";

    /// The issue numbers `values` names.
    fn numbers_of(values: &[u64]) -> Vec<IssueNumber> {
        values
            .iter()
            .map(|&value| IssueNumber::new(value).expect("a test number is an issue number"))
            .collect()
    }

    /// Two streams that join, drawn so that every order reads differently.
    ///
    /// The text names the steps in the order 4, 5, 9, 1, 2, because the bus
    /// reaches `#9` on the second line. A topological order that ties on the
    /// text names them 4, 5, 1, 2, 9, and one that ties on the number names
    /// them 1, 2, 4, 5, 9. So this picture parts the three.
    const TIE: &str = "\
#4 ──→ #5 ──┐
            ├──→ #9
#1 ──→ #2 ──┘";

    /// The numbers of the steps of `graph`, in the order the graph holds them.
    fn order(graph: &Graph) -> Vec<u64> {
        graph
            .steps()
            .iter()
            .map(|step| step.number().get())
            .collect()
    }

    /// The graph `text` draws.
    fn graph_of(text: &str) -> Graph {
        read(text)
            .expect("the text draws a graph")
            .expect("the picture reads")
    }

    /// The refusal `text` earns.
    ///
    /// A [`Graph`] writes no `Debug` of itself, so this reads the error out of
    /// the answer rather than through `expect_err`.
    fn refusal(text: &str) -> GraphError {
        match read(text).expect("the picture claims the text") {
            Ok(_) => panic!("the picture reads, and this text is a refusal"),
            Err(error) => error,
        }
    }

    /// The edges of `graph`: the number of the step before, and the number of
    /// the step after.
    ///
    /// Sorted, so a test states the shape of the graph and never the order the
    /// steps stand in. That order is the order of the text today and a
    /// topological order in the slice that answers a graph, and a test of the
    /// shape must read the same under both.
    fn edges(graph: &Graph) -> Vec<(u64, u64)> {
        let mut edges: Vec<(u64, u64)> = Vec::new();
        for (position, step) in graph.steps().iter().enumerate() {
            for &before in graph.before(position) {
                let earlier = graph.steps()[before].number().get();
                edges.push((earlier, step.number().get()));
            }
        }
        edges.sort_unstable();
        edges
    }

    /// The number of every node of `graph`, sorted for the same reason.
    fn nodes(graph: &Graph) -> Vec<u64> {
        let mut numbers: Vec<u64> = graph
            .steps()
            .iter()
            .map(|step| step.number().get())
            .collect();
        numbers.sort_unstable();
        numbers
    }

    /// Every number `graph` names, sorted.
    ///
    /// Sorted for the reason [`nodes`] is sorted: the order of this list is
    /// the order of the steps, and a test of what the list holds must read the
    /// same before and after a topological sort.
    fn named(graph: &Graph) -> Vec<u64> {
        let mut numbers: Vec<u64> = graph.numbers().iter().map(|number| number.get()).collect();
        numbers.sort_unstable();
        numbers
    }

    /// The plan of issue #436, as a Markdown table.
    ///
    /// Four streams, and the three shapes a `Waits for` cell is written in: an
    /// empty cell, one blocker, and two of them. S1 waits for the one step of
    /// S0, and S2 waits for a step of S0 and a step of S1, so the plan draws
    /// an edge into the first step of a stream from one stream and from two.
    const WAITS_TABLE: &str = "\
| Stream | Order | Waits for | Zone | Notes |
|--------|-------|-----------|------|-------|
| S0 — daemon leak | #96 | | crates/tsm (serve.rs) | Do first, solo. |
| S1 — lifecycle | #91 | #96 | crates/tsm (kill.rs) | |
| S2 — install | #89 → #94 | #96, #91 | crates/tsm (shell-init) | Same hotspot as S1. |
| S3 — keymap | #86 | | packages/web | Disjoint. |";

    /// The same four streams, as a record for each one.
    ///
    /// S0 writes an empty `Waits for` field and S3 writes none at all, because
    /// a stream that waits for nothing is written both ways and the two say
    /// the same thing.
    const WAITS_RECORDS: &str = "\
Stream: S0 — daemon leak
Order: #96
Waits for:
Zone: crates/tsm (serve.rs)
Notes: Do first, solo.
────────────────────────────────────────
Stream: S1 — lifecycle
Order: #91
Waits for: #96
Zone: crates/tsm (kill.rs)
────────────────────────────────────────
Stream: S2 — install
Order: #89 → #94
Waits for: #96, #91
Zone: crates/tsm (shell-init)
Notes: Same hotspot as S1.
────────────────────────────────────────
Stream: S3 — keymap
Order: #86
Zone: packages/web
Notes: Disjoint.";

    /// The same four streams, drawn as a box table.
    ///
    /// The form a reader copies out of a terminal. The `Waits for` cells of S0
    /// and S3 hold a run of spaces here, because a drawn cell is padded to the
    /// width of its column.
    const WAITS_BOX_TABLE: &str = "\
┌──────────────────┬───────────┬───────────┬─────────────────────────┬─────────────────────┐
│ Stream           │ Order     │ Waits for │ Zone                    │ Notes               │
├──────────────────┼───────────┼───────────┼─────────────────────────┼─────────────────────┤
│ S0 — daemon leak │ #96       │           │ crates/tsm (serve.rs)   │ Do first, solo.     │
├──────────────────┼───────────┼───────────┼─────────────────────────┼─────────────────────┤
│ S1 — lifecycle   │ #91       │ #96       │ crates/tsm (kill.rs)    │                     │
├──────────────────┼───────────┼───────────┼─────────────────────────┼─────────────────────┤
│ S2 — install     │ #89 → #94 │ #96, #91  │ crates/tsm (shell-init) │ Same hotspot as S1. │
├──────────────────┼───────────┼───────────┼─────────────────────────┼─────────────────────┤
│ S3 — keymap      │ #86       │           │ packages/web            │ Disjoint.           │
└──────────────────┴───────────┴───────────┴─────────────────────────┴─────────────────────┘";

    /// The plan `text` writes.
    fn plan_of(text: &str) -> Plan {
        crate::plan::parse(text).expect("the text is a plan")
    }

    /// The graph the plan `text` writes.
    fn graph_of_plan(text: &str) -> Graph {
        of_plan(&plan_of(text))
            .expect("the plan draws a graph")
            .expect("the plan reads")
    }

    /// The refusal the plan `text` earns.
    ///
    /// A [`Graph`] writes no `Debug` of itself, so this reads the error out of
    /// the answer rather than through `expect_err`, exactly as [`refusal`]
    /// does for a picture.
    fn plan_refusal(text: &str) -> GraphError {
        match of_plan(&plan_of(text)).expect("the plan draws a graph") {
            Ok(_) => panic!("the plan reads, and this plan is a refusal"),
            Err(error) => error,
        }
    }

    /// A plan of the streams `rows` names, as a Markdown table.
    ///
    /// Each row is the label of a stream, the text of its `Order` cell, and
    /// the text of its `Waits for` cell. Three columns and no prose, so a test
    /// about one cell writes that cell and nothing around it.
    fn table_of(rows: &[(&str, &str, &str)]) -> String {
        let mut text = String::from("| Stream | Order | Waits for |\n| --- | --- | --- |\n");
        for (label, order, waits) in rows {
            text.push_str(&format!("| {label} | {order} | {waits} |\n"));
        }
        text
    }

    #[test]
    fn reads_the_two_streams_that_join_of_the_paste() {
        let graph = graph_of(PASTE);
        assert_eq!(nodes(&graph), vec![242, 246, 247, 248, 249]);
        assert_eq!(
            edges(&graph),
            vec![(242, 247), (246, 248), (247, 249), (248, 249)]
        );
    }

    #[test]
    fn a_taller_bus_is_the_same_bus() {
        assert_eq!(nodes(&graph_of(TALL_PASTE)), nodes(&graph_of(PASTE)));
        assert_eq!(edges(&graph_of(TALL_PASTE)), edges(&graph_of(PASTE)));
    }

    #[test]
    fn a_fan_out_names_one_step_before_two() {
        let graph = graph_of(FAN_OUT);
        assert_eq!(nodes(&graph), vec![1, 2, 3]);
        assert_eq!(edges(&graph), vec![(1, 2), (1, 3)]);
    }

    #[test]
    fn the_ascii_spellings_draw_the_same_picture() {
        assert_eq!(
            nodes(&graph_of(&ascii(TALL_PASTE))),
            nodes(&graph_of(PASTE))
        );
        assert_eq!(
            edges(&graph_of(&ascii(TALL_PASTE))),
            edges(&graph_of(PASTE))
        );
        assert_eq!(edges(&graph_of(&ascii(FAN_OUT))), edges(&graph_of(FAN_OUT)));
    }

    #[test]
    fn the_heavy_set_and_the_double_set_draw_the_same_picture() {
        assert_eq!(
            nodes(&graph_of(&heavy(TALL_PASTE))),
            nodes(&graph_of(PASTE))
        );
        assert_eq!(
            edges(&graph_of(&heavy(TALL_PASTE))),
            edges(&graph_of(PASTE))
        );
        assert_eq!(
            edges(&graph_of(&double(TALL_PASTE))),
            edges(&graph_of(PASTE))
        );
    }

    #[test]
    fn a_node_carries_prose_beside_its_number() {
        // The text of a port runs to the wire beside it and never to the first
        // space, so the prose of a node arrives with the number and the reader
        // of a step drops it. `gallery` is thus a word and not an error.
        let graph = graph_of(
            "\
#1 ──┐
     ├──→ #249  (gallery)
#2 ──┘",
        );
        assert_eq!(nodes(&graph), vec![1, 2, 249]);
        assert_eq!(edges(&graph), vec![(1, 249), (2, 249)]);
    }

    #[test]
    fn a_node_holds_the_pull_request_and_the_issue_it_closes() {
        // One piece of work is sometimes two numbers. The reader of a step
        // reads the pair, so a picture holds it exactly as a table holds it:
        // the pull request is the work, and the issue is what the work closes.
        let graph = graph_of(
            "\
#1 ──┐
     ├──→ #4 (in flight, PR #15)
#2 ──┘",
        );
        assert_eq!(nodes(&graph), vec![1, 2, 15]);
        let work = graph
            .steps()
            .iter()
            .find(|step| step.number().get() == 15)
            .expect("the picture names the pull request");
        assert_eq!(work.closes().map(IssueNumber::get), Some(4));
    }

    #[test]
    fn a_step_that_stands_twice_is_one_node() {
        // Two streams that name the same issue join there, whether a wire says
        // so or not. The node carries the edges of both places it stands, and
        // it carries each of them once: the bus reaches `#2` from two rows,
        // and `#2` comes before `#4` one time.
        let graph = graph_of(
            "\
#1 ──→ #2 ──┐
            ├──→ #4
#3 ──→ #2 ──┘",
        );
        assert_eq!(nodes(&graph), vec![1, 2, 3, 4]);
        assert_eq!(edges(&graph), vec![(1, 2), (2, 4), (3, 2)]);
    }

    #[test]
    fn a_chain_on_one_line_is_a_chain() {
        // The net of this text names two steps, and every cell of it stands on
        // one line. So this reader claims nothing and the chain reader answers
        // the text, which is what it does today.
        assert!(read("#1 ──→ #2").is_none());
        assert!(read("#1 --> #2").is_none());

        // A longer chain holds two nets that each join two steps, and both of
        // them stand on the one line the reader wrote. The lines are what part
        // this text from two streams that never join.
        assert!(read("#1 ──→ #2 ──→ #3").is_none());
        assert!(read("#1 --> #2 --> #3").is_none());
    }

    #[test]
    fn two_streams_that_never_join_are_two_streams() {
        // Two rows of work with no bus between them say the same thing the
        // paste says: two people start now. Every net of this text stands on
        // one line, so the rule that reads a bus reads nothing here, and a
        // chain reader answers the whole page as one line of work.
        let graph = graph_of(TWO_STREAMS);
        assert_eq!(nodes(&graph), vec![242, 246, 247, 248]);
        assert_eq!(edges(&graph), vec![(242, 247), (246, 248)]);
    }

    #[test]
    fn a_chain_wrapped_over_two_lines_is_a_chain() {
        // The `──→` at the end of the first line reaches `#2` on its left and
        // nothing on its right, so the order runs on and the two lines are one
        // chain. A reader of pictures that claimed this text would answer half
        // an order, and the chain reader answers the whole of it.
        assert!(read(WRAPPED_CHAIN).is_none());
    }

    #[test]
    fn a_page_with_no_box_drawn_wire_is_no_picture() {
        // A page of prose names issues on line after line, and it draws
        // nothing. No word holds a character of the box-drawing block, so a
        // character of that block is the mark that somebody drew something.
        assert!(read(PROSE_PAGE).is_none());
        // The same two lines with a bare step on each side of each arrow. Both
        // of its nets join two steps and the two stand on two lines, so this
        // text fails the one test the page over it fails as well.
        assert!(read(BARE_ARROWS).is_none());
    }

    #[test]
    fn the_texts_this_reader_claimed_before_are_claimed_still() {
        // The second rule adds a shape and takes none away. The paste is the
        // picture this module was written for, and the two tables keep their
        // own readers.
        assert_eq!(
            edges(&graph_of(PASTE)),
            vec![(242, 247), (246, 248), (247, 249), (248, 249)]
        );
        assert!(read(BOX_TABLE).is_none());
        assert!(read(MARKDOWN_TABLE).is_none());
    }

    #[test]
    fn the_box_drawn_table_of_a_plan_is_a_table() {
        // The border of a box-drawn table is one net that touches every cell
        // of it, and it stands on every line of the table. It names no step,
        // because a bar touches its top and its bottom and never its sides, so
        // the net has no port at all and the table keeps its own reader.
        assert!(read(BOX_TABLE).is_none());
    }

    #[test]
    fn a_markdown_table_and_an_ascii_table_are_tables() {
        assert!(read(MARKDOWN_TABLE).is_none());
        assert!(read(&ascii(BOX_TABLE)).is_none());
    }

    #[test]
    fn an_indented_picture_and_a_fenced_one_draw_the_same_graph() {
        // A paste out of a Markdown list arrives indented, and a paste out of
        // a document arrives inside a fence with prose over it. An indent
        // moves every cell of the picture by the same number of columns, so
        // the geometry is the geometry. A line with no wire and no step is
        // ignored, so the fence and the sentence cost nothing.
        let indented: String = PASTE.lines().map(|line| format!("    {line}\n")).collect();
        assert_eq!(edges(&graph_of(&indented)), edges(&graph_of(PASTE)));

        let fenced = format!("The plan of the week:\n\n```text\n{PASTE}\n```\n");
        assert_eq!(edges(&graph_of(&fenced)), edges(&graph_of(PASTE)));
    }

    #[test]
    fn a_sentence_over_the_picture_changes_nothing() {
        // Prose holds a slash and a hyphen inside a word. A slash draws no
        // wire at all, and the hyphen of `30-line` draws none either: no
        // neighbor of it draws a wire, and a letter ends its run.
        const PROSE: &str = "See docs/plan.md. Each edit lands in a 30-line window.";
        assert!(read(PROSE).is_none());

        let page = format!("{PROSE}\n\n{PASTE}");
        assert_eq!(nodes(&graph_of(&page)), nodes(&graph_of(PASTE)));
        assert_eq!(edges(&graph_of(&page)), edges(&graph_of(PASTE)));
    }

    #[test]
    fn a_wide_character_shifts_no_wire() {
        let wide = graph_of(WIDE_NODE);
        assert_eq!(nodes(&wide), vec![1, 2, 3]);
        assert_eq!(edges(&wide), vec![(1, 3), (2, 3)]);
        assert_eq!(edges(&graph_of(PLAIN_NODE)), edges(&wide));
    }

    #[test]
    fn a_label_beside_a_wire_is_not_a_step() {
        // A stream label beside a wire is a plan this form does not carry. So
        // the reader refuses it and names it, rather than dropping the wire
        // that reaches it: a reader who wrote `A` meant something by it, and a
        // graph that quietly loses one edge answers with the wrong issue.
        assert_eq!(refusal(LABEL_PORT), GraphError::NotAStep(Snippet::new("A")));
    }

    #[test]
    fn a_wire_that_reaches_one_step_alone_is_refused() {
        // A wire with a step on one side and nothing on the other says half of
        // an order. A reader who drew it meant a second step, and the reader
        // cannot guess which one, so the message names the step it has.
        assert_eq!(refusal(HALF_NET), GraphError::HalfNet(Snippet::new("#5")));
    }

    #[test]
    fn a_leftward_arrowhead_is_refused_and_the_message_names_the_line() {
        // Each of the four spellings of the head, because a reader picks the
        // one their font draws best and the plan is the same plan.
        for head in ['\u{2190}', '\u{27f5}', '\u{21d0}', '\u{25c0}'] {
            let line = format!("#5 {head}── #6");
            assert_eq!(
                refusal(&under_a_picture(&line)),
                GraphError::Leftward(Snippet::new(&line)),
                "the head {head:?}"
            );
        }
    }

    #[test]
    fn a_diagonal_wire_is_refused_and_the_message_names_the_line() {
        // The two ASCII spellings and the three box-drawing ones. A diagonal
        // touches no side of a cell, so the picture says an order this reader
        // cannot follow, whichever character draws it. Each stroke stands
        // beside the wire on its left, which is what makes it a stroke of the
        // picture and not a character of prose.
        for stroke in ['/', '\\', '\u{2571}', '\u{2572}', '\u{2573}'] {
            let line = format!("#5 ──{stroke} #6");
            assert_eq!(
                refusal(&under_a_picture(&line)),
                GraphError::Diagonal(Snippet::new(&line)),
                "the stroke {stroke:?}"
            );
        }
    }

    #[test]
    fn a_slash_in_the_prose_of_a_port_draws_no_wire() {
        // The text of a port is prose, and prose holds a path, an escape, a
        // date, and two words a slash parts. A stroke inside such a text
        // stands beside no wire, so it draws nothing and each of these
        // pictures is the picture of the paste.
        for label in [
            "#249  (src/gallery)",
            "#249  (a\\b)",
            "#249  (2026/09/03)",
            "#249  (this and/or that)",
        ] {
            let picture = PASTE.replace("#249  (gallery)", label);
            let graph = graph_of(&picture);
            assert_eq!(
                nodes(&graph),
                nodes(&graph_of(PASTE)),
                "the label {label:?}"
            );
            assert_eq!(
                edges(&graph),
                edges(&graph_of(PASTE)),
                "the label {label:?}"
            );
        }
    }

    #[test]
    fn a_doubled_hyphen_in_the_prose_of_a_port_draws_no_wire() {
        // The text of a port is prose, and prose holds a flag, a width, and a
        // word a hyphen parts. A hyphen stands beside a hyphen inside such a
        // text, so the neighbor alone says nothing about the picture. The run
        // of the hyphens says it: a run that ends at a letter is a word, and a
        // word draws no wire. Each of these pictures is the picture of the
        // paste.
        for label in [
            "#249  (pass --hidden)",
            "#249  (the gallery--the last one)",
            "#249  (a 30-line window)",
            "#249  (the well-known gallery)",
        ] {
            let picture = PASTE.replace("#249  (gallery)", label);
            let graph = graph_of(&picture);
            assert_eq!(
                nodes(&graph),
                nodes(&graph_of(PASTE)),
                "the label {label:?}"
            );
            assert_eq!(
                edges(&graph),
                edges(&graph_of(PASTE)),
                "the label {label:?}"
            );
        }

        // The other half of the rule. A run that ends at a digit or at a `#`
        // is the wire a reader drew, so a picture drawn tight keeps every
        // edge and the rule takes nothing away from it.
        let tight = graph_of(TIGHT_ASCII);
        assert_eq!(nodes(&tight), vec![1, 2, 3, 4, 5]);
        assert_eq!(edges(&tight), vec![(1, 2), (2, 4), (3, 5), (5, 4)]);
    }

    #[test]
    fn a_line_with_no_wire_is_never_asked_about_a_head_or_a_stroke() {
        // A sentence over the picture holds a slash inside a path and an arrow
        // inside a phrase. A line with no wire on it draws nothing, so neither
        // of them belongs to the picture and the paste under it still reads.
        let page = format!("see docs/plan.md \u{2190} here\n\n{PASTE}");
        assert_eq!(edges(&graph_of(&page)), edges(&graph_of(PASTE)));
    }

    #[test]
    fn a_cycle_is_refused_and_the_message_names_the_steps_of_it() {
        // The steps of the cycle alone. Every node the cycle reaches is
        // blocked as well, and a message that names all of them tells a reader
        // to read five rows to find the two that hold the knot. The order is
        // the order a walk of the wires meets them, so the message reads as
        // the walk that found it.
        assert_eq!(
            refusal(CYCLE_OF_TWO),
            GraphError::Cycle(numbers_of(&[1, 2]))
        );
        assert_eq!(
            refusal(CYCLE_OF_THREE),
            GraphError::Cycle(numbers_of(&[1, 2, 3]))
        );
    }

    #[test]
    fn a_step_joined_to_itself_is_a_cycle_of_one() {
        // The shortest cycle there is. It waits for itself, so it never
        // starts, and the reader must hear which step holds the knot.
        assert_eq!(refusal(SELF_EDGE), GraphError::Cycle(numbers_of(&[1])));
    }

    #[test]
    fn a_step_with_no_wire_beside_it_is_a_node_with_no_edge() {
        // A stream of one step is a plan as much as a stream of five is, and a
        // reader who writes one under a picture wants to hear about it. So the
        // node stands in the graph with no step before it.
        let graph = graph_of(&under_a_picture("#6"));
        assert_eq!(nodes(&graph), vec![1, 3, 4, 6]);
        assert_eq!(edges(&graph), vec![(1, 3), (1, 4)]);
        let alone = graph
            .steps()
            .iter()
            .position(|step| step.number().get() == 6)
            .expect("the picture names the step that stands alone");
        assert!(graph.before(alone).is_empty());
    }

    #[test]
    fn a_line_of_prose_that_names_a_number_is_no_node() {
        // A line with no wire is a node only when the whole of it reads as one
        // step. A sentence that names an issue writes no step, because `See`
        // is no number, and a node for it would tell somebody to start work
        // the plan never named.
        let graph = graph_of(&under_a_picture("See #123 for the details"));
        assert_eq!(nodes(&graph), vec![1, 3, 4]);
    }

    #[test]
    fn the_steps_stand_in_a_topological_order() {
        // Every step stands after the steps it waits for, and a tie goes to
        // the step that stands first in the text. That keeps each stream
        // together: the paste prints the top row, then the bottom row, then
        // the step that joins them. The text names `#249` on its second line,
        // so the order of the text is not this order.
        assert_eq!(order(&graph_of(PASTE)), vec![242, 247, 246, 248, 249]);
    }

    #[test]
    fn a_tie_goes_to_the_step_that_stands_first_in_the_text() {
        // Two streams start at the same moment, so the order between them is a
        // tie. The reader wrote one of them first, and the answer reads the way
        // the reader wrote it.
        assert_eq!(order(&graph_of(TIE)), vec![4, 5, 1, 2, 9]);
    }

    #[test]
    fn the_steps_before_a_step_name_places_of_the_topological_order() {
        // The positions a row reads are the positions of the rows it prints
        // beside, so they must count in the order the steps stand in. Each
        // list is in ascending order, and every position of it is smaller than
        // the position of the step that holds it.
        let graph = graph_of(PASTE);
        assert_eq!(order(&graph), vec![242, 247, 246, 248, 249]);
        assert!(graph.before(0).is_empty());
        assert_eq!(graph.before(1), [0]);
        assert!(graph.before(2).is_empty());
        assert_eq!(graph.before(3), [2]);
        assert_eq!(graph.before(4), [1, 3]);
    }

    #[test]
    fn a_text_this_reader_does_not_claim_raises_nothing() {
        // The claim stands before every refusal, and this test pins that
        // order. Each of these three lines holds a drawing this reader refuses
        // inside a picture, and each of them stands on one line, so the chain
        // reader answers it. A refusal that ran before the claim would take
        // every one of them away from that reader and answer a chain with a
        // message about a picture nobody drew.
        assert!(read("#1 \u{2500}\u{2500}/ #2").is_none());
        assert!(read("#2 \u{2190}\u{2500}\u{2500} #1").is_none());
        assert!(read("#1 \u{2500}\u{2500}\u{2192}").is_none());
    }

    #[test]
    fn the_numbers_of_a_graph_name_each_one_once_and_the_work_first() {
        // One query answers the whole picture, so this list is what the query
        // asks about. `#2` stands twice in the picture and is one node, so it
        // is asked about one time. A pair gives two numbers, the pull request
        // ahead of the issue it closes, because the pull request is the work.
        let graph = graph_of(
            "\
#1 ──→ #2 ──┐
            ├──→ #4 (in flight, PR #15)
#3 ──→ #2 ──┘",
        );
        let numbers: Vec<u64> = graph.numbers().iter().map(|number| number.get()).collect();
        let mut once = numbers.clone();
        once.sort_unstable();
        assert_eq!(once, vec![1, 2, 3, 4, 15]);

        let work = numbers
            .iter()
            .position(|&number| number == 15)
            .expect("the list names the pull request");
        let closed = numbers
            .iter()
            .position(|&number| number == 4)
            .expect("the list names the issue the pull request closes");
        assert!(work < closed, "{numbers:?}");
    }

    #[test]
    fn the_message_of_a_cycle_reads_the_numbers_as_a_sentence() {
        // The one function that writes a list of numbers writes this one, so
        // the message of a cycle reads the way the note of a report reads. A
        // message that wrote the list the way a `Vec` writes itself would name
        // the type and not the plan.
        assert_eq!(
            refusal(CYCLE_OF_TWO).to_string(),
            "the wires return to #1 and #2, so this picture names no step to start first"
        );
        assert_eq!(
            refusal(CYCLE_OF_THREE).to_string(),
            "the wires return to #1, #2 and #3, so this picture names no step to start first"
        );
    }

    #[test]
    fn a_plan_that_names_a_blocker_draws_the_nodes_and_the_edges_of_it() {
        // The nodes stand in the order the plan writes them: the streams in
        // the order of the plan, and the chain of a stream inside it. The
        // edges are the chain of each stream, and one edge into the first step
        // of a stream for each blocker its cell names. `#94` waits for `#89`
        // alone, because the steps inside a stream keep their own chain.
        let graph = graph_of_plan(WAITS_TABLE);
        assert_eq!(order(&graph), vec![96, 91, 89, 94, 86]);
        assert_eq!(edges(&graph), vec![(89, 94), (91, 89), (96, 89), (96, 91)]);
    }

    #[test]
    fn the_three_written_forms_of_one_plan_give_one_graph() {
        // A reader writes the plan as a table, as a record for each stream, or
        // as the box table a terminal draws. The plan is the same plan, so the
        // graph is the same graph.
        for form in [WAITS_RECORDS, WAITS_BOX_TABLE] {
            assert_eq!(
                order(&graph_of_plan(form)),
                order(&graph_of_plan(WAITS_TABLE))
            );
            assert_eq!(
                edges(&graph_of_plan(form)),
                edges(&graph_of_plan(WAITS_TABLE))
            );
        }
    }

    #[test]
    fn a_number_that_stands_in_two_streams_of_a_plan_is_one_node() {
        // Two streams that name the same issue join there, as they do in a
        // picture. The node carries the edges of both places, and it carries
        // each of them once: the chain of S1 and the cell of S2 both say that
        // `#1` comes before `#2`, and a list that named it twice would tell a
        // reader to wait for it twice.
        let graph = graph_of_plan(&table_of(&[("S1", "#1 → #2", ""), ("S2", "#2 → #3", "#1")]));
        assert_eq!(nodes(&graph), vec![1, 2, 3]);
        assert_eq!(edges(&graph), vec![(1, 2), (2, 3)]);
    }

    #[test]
    fn a_blocker_that_stands_in_no_order_field_is_a_node_of_its_own() {
        // A blocker the repository does not have must reach the rows and turn
        // the run red, and a row of the answer is the only place that says so.
        // So the number is a node, and the one query names it.
        let graph = graph_of_plan(&table_of(&[("S1", "#91", "#96")]));
        assert_eq!(nodes(&graph), vec![91, 96]);
        assert_eq!(edges(&graph), vec![(96, 91)]);
        assert_eq!(named(&graph), vec![91, 96]);
    }
}
