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
//! # An ASCII wire needs a neighbor
//!
//! A reader draws the same picture with `-`, `|`, `+`, and `>`, and prose
//! holds all four of those characters. So an ASCII spelling is a wire only
//! when a neighbor on a side it touches is a connector character as well. `a
//! 30-line window` holds no wire, because a digit and a letter stand beside
//! its hyphen. A box-drawing character never stands inside a word, so it needs
//! no such test.
//!
//! # What this module claims
//!
//! A net claims the text when it names two steps or more and its cells stand
//! on more than one line. `#1 ──→ #2` on one line is a chain, and the chain
//! reader still answers it. The border of a box-drawn table touches no step at
//! all, so that table keeps its own reader.

use std::collections::{BTreeMap, VecDeque};

use thiserror::Error;

use crate::chain::{IssueNumber, Snippet};
use crate::plan::{one_step, Step};

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

/// The characters that draw a wire and stand inside prose as well.
///
/// Prose holds all four of them: a hyphen inside a word, a bar between two
/// words, a plus in a sum, and the `>` of a quotation. So each of them is a
/// wire only when a neighbor on a side it touches draws a wire as well. `>`
/// stands here and in [`RIGHTWARD_HEADS`], because it is the head of the
/// ASCII arrow and it is one character of prose.
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
/// This is the whole table, and it is read for the character alone. The rule
/// that an ASCII spelling needs a neighbor stands in [`Grid::sides_at`],
/// because that rule reads the grid and this table does not.
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
        // says so, and [`Grid::sides_at`] holds that rule.
        '-' => Sides::HORIZONTAL,
        '|' => Sides::VERTICAL,
        '+' => Sides::ALL,
        _ => return None,
    };
    Some(sides)
}

/// One line of the picture, one cell for each display column.
type Row = Vec<char>;

/// The picture, as a grid of cells and the wire each of them draws.
struct Grid {
    /// The characters, indexed by line and then by display column.
    cells: Vec<Row>,
    /// The sides each cell touches, or `None` where the cell draws no wire.
    wires: Vec<Vec<Option<Sides>>>,
}

impl Grid {
    /// The grid `text` draws.
    fn new(text: &str) -> Self {
        let cells: Vec<Row> = text.lines().map(|line| line.chars().collect()).collect();
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

    /// The character at `at`, or `None` when `at` is off the grid.
    fn glyph(cells: &[Row], at: Place) -> Option<char> {
        cells.get(at.row)?.get(at.column).copied()
    }

    /// The sides the cell at `at` touches, with the rule for an ASCII
    /// spelling applied.
    ///
    /// An ASCII spelling is a wire only when a neighbor on a side it touches
    /// draws a wire as well. The test reads the neighbor out of the table
    /// alone, so it never asks the same question of the neighbor and never
    /// recurses.
    fn sides_at(cells: &[Row], at: Place) -> Option<Sides> {
        let glyph = Self::glyph(cells, at)?;
        let sides = sides_of(glyph)?;
        if !ASCII_SPELLINGS.contains(&glyph) {
            return Some(sides);
        }
        Side::ALL
            .into_iter()
            .filter(|&side| sides.touches(side))
            .any(|side| {
                side.from(at)
                    .and_then(|next| Self::glyph(cells, next))
                    .is_some_and(|neighbor| sides_of(neighbor).is_some())
            })
            .then_some(sides)
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

    /// The steps `net` joins.
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
        let spans_lines = net.iter().any(|place| place.row != net[0].row);
        Wiring {
            before,
            after,
            spans_lines,
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
        one_step(&text).map(|step| Port { place, step })
    }

    /// The first cell beyond `at` on `side` that is not a space, or `None`
    /// when the walk leaves the line.
    fn first_mark(&self, at: Place, side: Side) -> Option<Place> {
        let mut place = side.from(at)?;
        while self.is_space(place) {
            place = side.from(place)?;
        }
        Self::glyph(&self.cells, place).map(|_| place)
    }

    /// The far end of the text of a port that starts at `near`.
    ///
    /// The text runs to the cell before the nearest wire beyond it, or to the
    /// edge of the line. So `#1 ──→ #2 ──→ #3` gives `#2` to both of the nets
    /// that touch it, and `#249  (gallery)` is one port and not two.
    fn text_edge(&self, near: Place, side: Side) -> Place {
        let mut edge = near;
        while let Some(next) = side.from(edge) {
            if Self::glyph(&self.cells, next).is_none() || self.sides(next).is_some() {
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
            let Some(&glyph) = line.get(column) else {
                break;
            };
            if !glyph.is_whitespace() && start.is_none() {
                start = Some(column);
            }
            text.push(glyph);
        }
        start.map(|column| (text.trim().to_string(), Place { row, column }))
    }
}

/// One step a net names, and where the text of it stands.
struct Port {
    /// Where the first character of the text stands.
    place: Place,
    /// The step the text names.
    step: Step,
}

/// One net of the picture, and the steps it joins.
struct Wiring {
    /// The steps the net names on its left. Each of them comes before each
    /// step of `after`.
    before: Vec<Port>,
    /// The steps the net names on its right.
    after: Vec<Port>,
    /// The cells of the net stand on more than one line.
    spans_lines: bool,
}

impl Wiring {
    /// Does this net claim the text for the reader of pictures?
    ///
    /// It claims the text when it names two steps or more and its cells stand
    /// on more than one line. The second half is what keeps `#1 ──→ #2` a
    /// chain: both of its steps stand on one line, and the chain reader
    /// answers such a text today.
    fn claims(&self) -> bool {
        self.spans_lines && self.before.len() + self.after.len() >= CLAIMING_PORTS
    }
}

/// The steps a picture names, and the steps that come before each of them.
pub struct Graph {
    /// The steps, one for each node of the picture.
    steps: Vec<Step>,
    /// For each step, the positions of the steps that come before it.
    before: Vec<Vec<usize>>,
}

impl Graph {
    /// The steps of the picture, in the order they stand in the text.
    ///
    /// The slice that answers a graph puts them in a topological order, with a
    /// tie going to the step that stands first in the text. A caller of this
    /// slice reads the order of the text.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// The positions of the steps that come before the step at `position`.
    ///
    /// A position outside the graph gives an empty list, because a caller that
    /// walks the steps of another graph asks a question about nothing.
    #[must_use]
    pub fn before(&self, position: usize) -> &[usize] {
        self.before
            .get(position)
            .map_or(NO_POSITIONS, Vec::as_slice)
    }

    /// Every number the picture names, once.
    #[must_use]
    pub fn numbers(&self) -> Vec<IssueNumber> {
        self.steps.iter().map(Step::number).collect()
    }
}

/// Why a picture is not a graph.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GraphError {
    /// The text of a port names no step.
    ///
    /// The slice that refuses a picture builds this. This slice reads
    /// leniently, so a port whose text names no step is simply not a port, and
    /// a text that this reader does not claim reaches the chain reader with no
    /// message of its own.
    #[error("{0:?} is not a step")]
    NotAStep(Snippet),
}

/// The graph `text` draws, or `None` when `text` draws none.
///
/// The claim and the read share all of their work, so one function does both.
/// A text this reader does not claim gives `None` and no message, because the
/// chain reader takes such a text next: a chain broken over two lines must
/// reach that reader and not a refusal of this one.
///
/// # Errors
///
/// Gives the refusals of [`GraphError`] for a picture this reader claims and
/// cannot read. The slice that refuses a picture builds them, and the slice
/// that finds a cycle refuses one here as well.
pub fn read(text: &str) -> Option<Result<Graph, GraphError>> {
    let grid = Grid::new(text);
    let wirings: Vec<Wiring> = grid
        .nets()
        .iter()
        .map(|net| grid.wiring(net))
        .filter(|wiring| !wiring.before.is_empty() || !wiring.after.is_empty())
        .collect();
    if !wirings.iter().any(Wiring::claims) {
        return None;
    }
    Some(Ok(build(&wirings)))
}

/// The graph the nets of a picture draw.
///
/// A node is keyed on the number of its step, so a number that stands twice in
/// the picture is one node that carries the edges of both places. The step of
/// the earliest place stands, because a reader who wrote the pair of a step
/// once wrote it where the step first appears.
fn build(wirings: &[Wiring]) -> Graph {
    let mut ports: Vec<&Port> = wirings
        .iter()
        .flat_map(|wiring| wiring.before.iter().chain(&wiring.after))
        .collect();
    ports.sort_by_key(|port| port.place);
    let mut nodes: BTreeMap<IssueNumber, (Place, Step)> = BTreeMap::new();
    for port in ports {
        nodes
            .entry(port.step.number())
            .or_insert((port.place, port.step));
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
        for after in &wiring.after {
            let Some(&to) = positions.get(&after.step.number()) else {
                continue;
            };
            for earlier in &wiring.before {
                let Some(&from) = positions.get(&earlier.step.number()) else {
                    continue;
                };
                before[to].push(from);
            }
        }
    }
    // A step that reaches itself stays, so the slice that refuses a cycle
    // finds it here rather than in a picture this reader already flattened.
    for positions in &mut before {
        positions.sort_unstable();
    }
    Graph { steps, before }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The graph `text` draws.
    fn graph_of(text: &str) -> Graph {
        read(text)
            .expect("the text draws a graph")
            .expect("the picture reads")
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
}
