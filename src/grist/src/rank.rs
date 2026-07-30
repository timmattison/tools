//! Ordering of candidate merge sequences by resolution cost.

use crate::metrics::OrderingScore;

/// Rank simulated orderings cheapest-first.
#[must_use]
pub fn rank(_scores: Vec<OrderingScore>) -> Vec<OrderingScore> {
    todo!("rank orderings by resolution cost")
}

#[cfg(test)]
mod tests {
    use super::rank;
    use crate::metrics::{BranchName, Files, Hunks, OrderingScore, Stops};

    fn score(order: &[&str], stops: usize, files: usize, hunks: usize) -> OrderingScore {
        OrderingScore::new(
            order.iter().map(|b| BranchName::new(*b)).collect(),
            Stops::new(stops),
            Files::new(files),
            Hunks::new(hunks),
        )
    }

    #[test]
    fn ranks_fewer_hunks_first_even_when_it_stops_more_often() {
        // Hunks are the direct measure of lines a human must hand-merge, so they
        // outrank the number of times the rebase halts.
        let many_hunks_one_stop = score(&["a", "b"], 1, 1, 9);
        let few_hunks_three_stops = score(&["b", "a"], 3, 4, 2);

        let ranked = rank(vec![many_hunks_one_stop.clone(), few_hunks_three_stops.clone()]);

        assert_eq!(ranked, vec![few_hunks_three_stops, many_hunks_one_stop]);
    }
}
