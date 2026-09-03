//! Reading a plan drawn as a picture.

use thiserror::Error;

use crate::chain::{IssueNumber, Snippet};
use crate::plan::Step;

/// The list a position outside the graph gives.
///
/// A named constant, because [`Graph::before`] gives a slice and an empty
/// `Vec` of its own would live no longer than the call.
const NO_POSITIONS: &[usize] = &[];

/// The steps a picture names, and the steps that come before each of them.
pub struct Graph {
    /// The steps, one for each node of the picture.
    steps: Vec<Step>,
    /// For each step, the positions of the steps that come before it.
    before: Vec<Vec<usize>>,
}

impl Graph {
    /// The steps of the picture, in the order they stand in the text.
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
    #[error("{0:?} is not a step")]
    NotAStep(Snippet),
}

/// The graph `text` draws, or `None` when `text` draws none.
///
/// The claim and the read share all of their work, so one function does both.
/// A text this reader does not claim gives `None` and no message, because the
/// chain reader takes such a text next.
///
/// # Errors
///
/// Gives the refusals of [`GraphError`] for a picture this reader claims and
/// cannot read.
pub fn read(_text: &str) -> Option<Result<Graph, GraphError>> {
    None
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
}
