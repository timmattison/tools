//! Enumerating and evaluating every order the branches could land in.

use crate::metrics::BranchName;

/// Every order the branches could land in, input order first.
#[must_use]
pub fn permutations(_branches: &[BranchName]) -> Vec<Vec<BranchName>> {
    todo!("enumerate branch orderings")
}

#[cfg(test)]
mod tests {
    use super::permutations;
    use crate::metrics::BranchName;
    use std::collections::HashSet;

    fn branches(names: &[&str]) -> Vec<BranchName> {
        names.iter().map(|n| BranchName::new(*n)).collect()
    }

    #[test]
    fn enumerates_every_ordering_exactly_once_starting_with_the_input_order() {
        let input = branches(&["a", "b", "c"]);

        let orderings = permutations(&input);

        assert_eq!(
            orderings.first(),
            Some(&input),
            "the order the user typed should be evaluated first so ties favour it"
        );
        assert_eq!(orderings.len(), 6);
        let distinct: HashSet<_> = orderings.iter().collect();
        assert_eq!(distinct.len(), 6, "orderings must not repeat");
    }
}
