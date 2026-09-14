//! Holding a plan to what its own issues say comes first.
//!
//! A plan is a claim about the order of the work, and it is written by a person
//! or by a run of a model. The issues make a claim of their own under `Blocked
//! by`. The two can disagree, and a plan once put #170 before #168 while #170
//! said that #168 blocked it. `wn` answered the plan, and the reader started
//! #170 and found it blocked.
//!
//! So the answer reads both claims, and three things can come of that:
//!
//! * The plan holds every blocker its open issues name, and it answers as it
//!   always did.
//! * The plan puts an issue before its own blocker. That is a refusal, because
//!   an answer to that plan sends somebody to work that cannot start.
//! * The plan leaves a blocker out. The blocker joins the graph, the answer
//!   says the step waits for it, and a note says the plan did not. A plan
//!   whose own order holds a cycle cannot become a graph, so that plan is
//!   refused, and the refusal names the wait as well as the cycle.
//!
//! A blocker that stands nowhere in the plan is asked about, because the plan
//! says nothing about its state. A finished one changes nothing. An open one
//! joins the graph as a step, and the blockers it names are read in turn.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::chain::{list, IssueNumber};
use crate::graph::{of_parts, Graph, GraphError, Work};
use crate::plan::Step;
use crate::report::{Entry, States};

/// One edge of an order: the number of the step before, and the number of the
/// step after.
type Edge = (IssueNumber, IssueNumber);

/// The steps of the answer that wait for work the plan did not say they wait
/// for, and that work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftOut {
    /// The number of the step that waits.
    pub step: IssueNumber,
    /// The numbers of the steps it waits for, which the plan did not name.
    pub blockers: Vec<IssueNumber>,
}

/// The plan, once what its issues say comes first is part of it.
pub enum Settled {
    /// The plan holds every blocker its open issues name. `states` holds what
    /// GitHub said about every number, those asked about while settling
    /// included.
    Agrees(States),
    /// The plan left a blocker out, and `graph` holds it.
    Adds {
        /// The steps of the plan and the blockers it left out, with every edge
        /// of the plan and every edge the issues name.
        graph: Graph,
        /// What GitHub said about every step of `graph`.
        states: States,
        /// What the plan left out, one element for each step that waits for
        /// more than the plan said.
        left_out: Vec<LeftOut>,
    },
}

/// The plan `graph` draws, held to what its open issues say comes first.
///
/// `states` is what GitHub said about every number of `graph`. `fetch` asks
/// GitHub about more numbers, and it is called only for a blocker whose state
/// nobody asked about yet.
///
/// # Errors
///
/// Gives an [`OrderError`] for a plan that puts an issue before its own
/// blocker, for blockers that come before each other, and for an order with a
/// cycle of its own once the issues add a wait to it. Gives the error of
/// `fetch` when GitHub cannot answer.
pub fn settle(
    graph: &Graph,
    states: States,
    fetch: &dyn Fn(&[IssueNumber]) -> anyhow::Result<Vec<Entry>>,
) -> anyhow::Result<Settled> {
    let mut states = states;
    // A blocker that names the issue of a pair names the pair, because the pull
    // request does the work. A number the plan names nowhere names itself.
    let work = Work::of(graph.steps());
    let plan_edges = graph.edges();
    let mut steps: Vec<Step> = graph.steps().to_vec();
    let mut added: Vec<Edge> = Vec::new();
    let mut left_out: Vec<LeftOut> = Vec::new();

    // The first round examines the steps of the plan, and each round after it
    // examines the blockers the round before it added. A round adds a step only
    // for a blocker no step holds, so the rounds end when the blockers the
    // issues name run out, and every number of a repository is finite.
    let mut examined = 0;
    while examined < steps.len() {
        let round: Vec<Step> = steps.get(examined..).unwrap_or_default().to_vec();
        examined = steps.len();
        let unknown = unknown_blockers(&round, &states, &work);
        if !unknown.is_empty() {
            states.extend(fetch(&unknown)?);
        }
        for step in round {
            for (listed_by, named) in blockers_of(step, &states) {
                let blocker = work.names(named);
                if blocker == step.number() || states.entry(blocker).status.is_finished() {
                    continue;
                }
                let order: Vec<Edge> = plan_edges.iter().chain(&added).copied().collect();
                if path(&order, blocker, step.number()).is_some() {
                    continue;
                }
                if path(&plan_edges, step.number(), blocker).is_some() {
                    return Err(OrderError::Reversed {
                        step: step.number(),
                        blocker,
                        listed_by,
                        named,
                    }
                    .into());
                }
                if let Some(cycle) = path(&order, step.number(), blocker) {
                    return Err(OrderError::Cycle(cycle).into());
                }
                added.push((blocker, step.number()));
                if !steps.iter().any(|held| held.number() == blocker) {
                    steps.push(work.step(blocker));
                }
                match left_out.iter_mut().find(|held| held.step == step.number()) {
                    Some(held) => held.blockers.push(blocker),
                    None => left_out.push(LeftOut {
                        step: step.number(),
                        blockers: vec![blocker],
                    }),
                }
            }
        }
    }

    if added.is_empty() {
        return Ok(Settled::Agrees(states));
    }
    let mut edges = plan_edges;
    edges.extend(added);
    let graph = of_parts(steps, &edges).map_err(|err| knotted(err, &left_out))?;
    Ok(Settled::Adds {
        graph,
        states,
        left_out,
    })
}

/// The refusal of a graph that the edges of the plan and the waits the issues
/// add cannot build.
///
/// A wait joins a blocker to a step only when no walk returns from that step
/// to that blocker, so no cycle runs through a wait. A cycle that `err` names
/// is thus a cycle of the plan alone, and the reader of streams answered that
/// plan before the issues added a wait. The wait is what changed, so the
/// refusal names the first wait of `left_out` with the cycle. Any other error
/// of the graph passes through as it is.
fn knotted(err: GraphError, left_out: &[LeftOut]) -> anyhow::Error {
    let wait = left_out
        .first()
        .and_then(|held| held.blockers.first().map(|&blocker| (held.step, blocker)));
    match (err, wait) {
        (GraphError::Cycle(cycle), Some((step, blocker))) => OrderError::Knotted {
            cycle,
            step,
            blocker,
        }
        .into(),
        (err, _) => err.into(),
    }
}

/// Every blocker the issues of `step` name, with the number of the issue that
/// names it, or nothing when `step` is not open.
///
/// A step names its own number, and a pair names the issue its pull request
/// closes as well. The body is on the issue, so the blockers of the issue are
/// the blockers of the step. A step nobody can start again waits for nothing.
fn blockers_of(step: Step, states: &States) -> Vec<Edge> {
    if !states.entry(step.number()).status.is_open() {
        return Vec::new();
    }
    [Some(step.number()), step.closes()]
        .into_iter()
        .flatten()
        .flat_map(|listed_by| {
            states
                .entry(listed_by)
                .blocked_by
                .into_iter()
                .map(move |named| (listed_by, named))
        })
        .collect()
}

/// The blockers the steps of `round` name whose state nobody asked about yet,
/// each one once, so one query answers for the whole round.
fn unknown_blockers(round: &[Step], states: &States, work: &Work) -> Vec<IssueNumber> {
    let mut unknown: Vec<IssueNumber> = Vec::new();
    for &step in round {
        for (_, named) in blockers_of(step, states) {
            let blocker = work.names(named);
            if !states.knows(blocker) && !unknown.contains(&blocker) {
                unknown.push(blocker);
            }
        }
    }
    unknown
}

/// The numbers of one walk from `from` to `to` along `edges`, both ends
/// included, or `None` when no walk reaches `to`.
///
/// The walk is breadth first, so the walk it names is a shortest one, and a
/// refusal that names it names no more steps than it must.
fn path(edges: &[Edge], from: IssueNumber, to: IssueNumber) -> Option<Vec<IssueNumber>> {
    let mut came_from: BTreeMap<IssueNumber, IssueNumber> = BTreeMap::new();
    let mut reached: BTreeSet<IssueNumber> = BTreeSet::from([from]);
    let mut queue: VecDeque<IssueNumber> = VecDeque::from([from]);
    while let Some(at) = queue.pop_front() {
        if at == to {
            let mut walk = vec![to];
            let mut back = to;
            while let Some(&earlier) = came_from.get(&back) {
                walk.push(earlier);
                back = earlier;
            }
            walk.reverse();
            return Some(walk);
        }
        for &(before, after) in edges {
            if before == at && reached.insert(after) {
                came_from.insert(after, at);
                queue.push_back(after);
            }
        }
    }
    None
}

/// The sentence that closes a refusal the reader repairs in the order itself.
const FIX_THE_ORDER: &str = "Fix the order, or run wn --refresh to build a new plan";

/// Why a plan cannot be answered once its issues are read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrderError {
    /// The plan puts `step` before `blocker`, and `listed_by` says `named`
    /// blocks it.
    ///
    /// `listed_by` is `step` itself, or the issue `step` closes. `named` is
    /// `blocker` itself, or an issue whose work `blocker` does.
    #[error(
        "the order puts {step} before {blocker}, but {listed_by} says it is blocked by {named}. {}",
        FIX_THE_ORDER
    )]
    Reversed {
        step: IssueNumber,
        blocker: IssueNumber,
        listed_by: IssueNumber,
        named: IssueNumber,
    },
    /// No edge of the plan goes against a blocker, and the blockers the issues
    /// name still return to where they started. The numbers are the walk from
    /// the step to its blocker, and the blocker comes before the step.
    #[error(
        "the order returns to {} once the blockers each issue names join the order, \
         so no step can start first",
        list(.0)
    )]
    Cycle(Vec<IssueNumber>),
    /// The order returns to the steps of `cycle`, and the issues add a wait:
    /// `step` waits for `blocker`.
    ///
    /// A plan of streams can name one number in two orders. The reader of
    /// streams answers each stream on its own, so it answers such a plan while
    /// the issues add no wait. A wait joins one step to another, and only a
    /// graph can show that. A graph cannot hold the cycle, so the run refuses.
    /// An answer of streams would name `step` as ready while `blocker` is
    /// open, and that is the failure this check exists to stop.
    ///
    /// No cycle runs through a wait the issues add, so the steps of `cycle`
    /// are steps of the order alone. The message names the wait as well,
    /// because the wait is why an order that answered before now refuses.
    #[error(
        "the order returns to {}, and {step} waits for {blocker}, \
         which only an order with no cycle can show. {}",
        list(.cycle),
        FIX_THE_ORDER
    )]
    Knotted {
        cycle: Vec<IssueNumber>,
        step: IssueNumber,
        blocker: IssueNumber,
    },
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::graph::of_parts;
    use crate::plan::Step;
    use crate::report::Status;

    fn issue(number: u64) -> IssueNumber {
        IssueNumber::new(number).expect("the test number is an issue number")
    }

    fn numbers(values: &[u64]) -> Vec<IssueNumber> {
        values.iter().map(|value| issue(*value)).collect()
    }

    fn entry(number: u64, status: Status, blocked_by: &[u64]) -> Entry {
        Entry {
            number: issue(number),
            title: format!("title of {number}"),
            status,
            closes: None,
            blocked_by: numbers(blocked_by),
        }
    }

    fn open(number: u64, blocked_by: &[u64]) -> Entry {
        entry(number, Status::Open, blocked_by)
    }

    fn done(number: u64, blocked_by: &[u64]) -> Entry {
        entry(number, Status::Done, blocked_by)
    }

    /// The graph of `steps`, with an edge from each `(before, after)` pair.
    fn graph(steps: &[Step], edges: &[(u64, u64)]) -> Graph {
        let edges: Vec<(IssueNumber, IssueNumber)> = edges
            .iter()
            .map(|(before, after)| (issue(*before), issue(*after)))
            .collect();
        of_parts(steps.to_vec(), &edges).expect("the test graph holds no cycle")
    }

    fn step(number: u64) -> Step {
        Step::new(issue(number), None)
    }

    /// The graph of one chain of steps, in the order `values` writes them.
    fn line(values: &[u64]) -> Graph {
        let steps: Vec<Step> = values.iter().map(|value| step(*value)).collect();
        let edges: Vec<(u64, u64)> = values.windows(2).map(|pair| (pair[0], pair[1])).collect();
        graph(&steps, &edges)
    }

    /// A `fetch` that answers from `known`, and the numbers of every call it
    /// was given.
    struct Answers {
        known: Vec<Entry>,
        asked: RefCell<Vec<Vec<u64>>>,
    }

    impl Answers {
        fn of(known: Vec<Entry>) -> Self {
            Self {
                known,
                asked: RefCell::new(Vec::new()),
            }
        }

        /// Answer for `wanted` the way GitHub answers: one entry for each
        /// number, and a missing entry for a number the repository lacks.
        fn fetch(&self, wanted: &[IssueNumber]) -> anyhow::Result<Vec<Entry>> {
            self.asked
                .borrow_mut()
                .push(wanted.iter().map(|number| number.get()).collect());
            Ok(wanted
                .iter()
                .map(|number| {
                    self.known
                        .iter()
                        .find(|entry| entry.number == *number)
                        .cloned()
                        .unwrap_or_else(|| entry(number.get(), Status::Missing, &[]))
                })
                .collect())
        }

        fn asked(&self) -> Vec<Vec<u64>> {
            self.asked.borrow().clone()
        }
    }

    fn settled(graph: &Graph, states: Vec<Entry>, answers: &Answers) -> anyhow::Result<Settled> {
        settle(graph, States::of(states), &|wanted| answers.fetch(wanted))
    }

    /// The refusal a settle gave, or a panic that says what it gave instead.
    fn refusal(result: anyhow::Result<Settled>) -> OrderError {
        match result {
            Ok(Settled::Agrees(_)) => panic!("the plan was refused, and it agreed"),
            Ok(Settled::Adds { .. }) => panic!("the plan was refused, and it grew"),
            Err(err) => err
                .downcast_ref::<OrderError>()
                .cloned()
                .unwrap_or_else(|| panic!("the refusal is an order error, and it is {err:#}")),
        }
    }

    /// The graph and the note a settle that grew gave, or a panic.
    fn grown(result: anyhow::Result<Settled>) -> (Graph, States, Vec<LeftOut>) {
        match result {
            Ok(Settled::Adds {
                graph,
                states,
                left_out,
            }) => (graph, states, left_out),
            Ok(Settled::Agrees(_)) => panic!("the plan grew, and it agreed"),
            Err(err) => panic!("the plan grew, and it was refused: {err:#}"),
        }
    }

    fn agrees(result: anyhow::Result<Settled>) -> States {
        match result {
            Ok(Settled::Agrees(states)) => states,
            Ok(Settled::Adds { left_out, .. }) => {
                panic!("the plan agreed, and it grew by {left_out:?}")
            }
            Err(err) => panic!("the plan agreed, and it was refused: {err:#}"),
        }
    }

    /// The numbers of the steps of `graph`, in its order.
    fn order(graph: &Graph) -> Vec<u64> {
        graph
            .steps()
            .iter()
            .map(|step| step.number().get())
            .collect()
    }

    /// The numbers of the steps that come before the step `number` names.
    fn before(graph: &Graph, number: u64) -> Vec<u64> {
        let position = graph
            .steps()
            .iter()
            .position(|step| step.number() == issue(number))
            .unwrap_or_else(|| panic!("the graph holds #{number}"));
        graph
            .before(position)
            .iter()
            .map(|earlier| graph.steps()[*earlier].number().get())
            .collect()
    }

    #[test]
    fn a_plan_that_holds_every_blocker_agrees_and_asks_nothing() {
        let answers = Answers::of(Vec::new());
        agrees(settled(
            &line(&[168, 170]),
            vec![open(168, &[]), open(170, &[168])],
            &answers,
        ));
        assert!(answers.asked().is_empty(), "it asked {:?}", answers.asked());
    }

    #[test]
    fn a_plan_that_puts_an_issue_before_its_own_blocker_is_refused() {
        // The plan of the wave tank, as the run wrote it.
        let answers = Answers::of(Vec::new());
        let err = refusal(settled(
            &line(&[170, 168]),
            vec![open(170, &[168]), open(168, &[])],
            &answers,
        ));
        assert_eq!(
            err,
            OrderError::Reversed {
                step: issue(170),
                blocker: issue(168),
                listed_by: issue(170),
                named: issue(168),
            }
        );
        assert_eq!(
            err.to_string(),
            "the order puts #170 before #168, but #170 says it is blocked by #168. \
             Fix the order, or run wn --refresh to build a new plan"
        );
    }

    #[test]
    fn a_blocker_further_down_the_order_is_refused_as_well() {
        let answers = Answers::of(Vec::new());
        let err = refusal(settled(
            &line(&[170, 169, 168]),
            vec![open(170, &[168]), open(169, &[]), open(168, &[])],
            &answers,
        ));
        assert!(
            matches!(err, OrderError::Reversed { step, blocker, .. } if step == issue(170) && blocker == issue(168)),
            "the refusal names #170 and #168, and it is {err:?}"
        );
    }

    #[test]
    fn a_blocker_the_plan_left_out_joins_the_graph() {
        // Two streams that stand apart, and #171 says #168 blocks it.
        let answers = Answers::of(Vec::new());
        let (graph, _, left_out) = grown(settled(
            &graph(&[step(171), step(168)], &[]),
            vec![open(171, &[168]), open(168, &[])],
            &answers,
        ));
        assert_eq!(before(&graph, 171), vec![168]);
        assert_eq!(
            left_out,
            vec![LeftOut {
                step: issue(171),
                blockers: numbers(&[168]),
            }]
        );
        assert!(answers.asked().is_empty(), "it asked {:?}", answers.asked());
    }

    #[test]
    fn a_finished_blocker_changes_nothing() {
        // #168 is done, so the order the plan gives it is a note about work
        // closed early and never a refusal.
        let answers = Answers::of(Vec::new());
        agrees(settled(
            &line(&[170, 168]),
            vec![open(170, &[168]), done(168, &[])],
            &answers,
        ));
    }

    #[test]
    fn a_finished_issue_names_no_blocker() {
        // Nobody starts #170 again, so what it said blocked it is history.
        let answers = Answers::of(Vec::new());
        agrees(settled(
            &line(&[170, 168]),
            vec![done(170, &[168]), open(168, &[])],
            &answers,
        ));
    }

    #[test]
    fn a_finished_blocker_outside_the_plan_is_asked_about_and_changes_nothing() {
        let answers = Answers::of(vec![done(167, &[])]);
        let states = agrees(settled(&line(&[168]), vec![open(168, &[167])], &answers));
        assert_eq!(answers.asked(), vec![vec![167]]);
        assert_eq!(states.entry(issue(167)).status, Status::Done);
    }

    #[test]
    fn an_open_blocker_outside_the_plan_joins_the_graph_with_its_own_blockers() {
        // #170 names #168, and #168 names #167. Neither stands in the plan, so
        // each round asks about the blockers the round before it found.
        let answers = Answers::of(vec![open(168, &[167]), open(167, &[])]);
        let (graph, states, left_out) =
            grown(settled(&line(&[170]), vec![open(170, &[168])], &answers));
        assert_eq!(answers.asked(), vec![vec![168], vec![167]]);
        assert_eq!(order(&graph), vec![167, 168, 170]);
        assert_eq!(before(&graph, 170), vec![168]);
        assert_eq!(before(&graph, 168), vec![167]);
        assert_eq!(states.entry(issue(167)).status, Status::Open);
        assert_eq!(
            left_out,
            vec![
                LeftOut {
                    step: issue(170),
                    blockers: numbers(&[168]),
                },
                LeftOut {
                    step: issue(168),
                    blockers: numbers(&[167]),
                },
            ]
        );
    }

    #[test]
    fn a_blocker_two_steps_name_is_asked_about_once() {
        let answers = Answers::of(vec![open(168, &[])]);
        let (graph, _, _) = grown(settled(
            &graph(&[step(170), step(171)], &[]),
            vec![open(170, &[168]), open(171, &[168])],
            &answers,
        ));
        assert_eq!(answers.asked(), vec![vec![168]]);
        assert_eq!(before(&graph, 170), vec![168]);
        assert_eq!(before(&graph, 171), vec![168]);
    }

    #[test]
    fn a_blocker_the_repository_does_not_have_joins_as_a_step() {
        // A typo in a body is not finished work, so the step still waits, and
        // the row of the missing number says why.
        let answers = Answers::of(Vec::new());
        let (graph, states, _) = grown(settled(&line(&[170]), vec![open(170, &[999])], &answers));
        assert_eq!(before(&graph, 170), vec![999]);
        assert_eq!(states.entry(issue(999)).status, Status::Missing);
    }

    #[test]
    fn the_issue_a_pair_closes_answers_for_the_pair() {
        // `PR#344 (#341)` is one step, and the blockers of #341 are the
        // blockers of that step.
        let answers = Answers::of(Vec::new());
        let pair = Step::new(issue(344), Some(issue(341)));
        let (graph, _, left_out) = grown(settled(
            &graph(&[pair, step(168)], &[]),
            vec![open(344, &[]), open(341, &[168]), open(168, &[])],
            &answers,
        ));
        assert_eq!(before(&graph, 344), vec![168]);
        assert_eq!(left_out[0].step, issue(344));
    }

    #[test]
    fn a_blocker_whose_work_a_pair_does_is_that_pair() {
        // #170 says #168 blocks it, and `PR#345 (#168)` does the work of #168.
        // The plan puts #170 before that pair.
        let answers = Answers::of(Vec::new());
        let pair = Step::new(issue(345), Some(issue(168)));
        let err = refusal(settled(
            &graph(&[step(170), pair], &[(170, 345)]),
            vec![open(170, &[168]), open(345, &[]), open(168, &[])],
            &answers,
        ));
        assert_eq!(
            err,
            OrderError::Reversed {
                step: issue(170),
                blocker: issue(345),
                listed_by: issue(170),
                named: issue(168),
            }
        );
    }

    #[test]
    fn blockers_that_come_before_each_other_only_through_the_issues_are_refused() {
        // The plan says #1 before #3 and nothing more. #1 says #2 blocks it,
        // and #2 says #3 blocks it. No single blocker goes against the plan,
        // and the three together return to where they started.
        let answers = Answers::of(Vec::new());
        let err = refusal(settled(
            &graph(&[step(1), step(2), step(3)], &[(1, 3)]),
            vec![open(1, &[2]), open(2, &[3]), open(3, &[])],
            &answers,
        ));
        let message = err.to_string();
        for number in ["#1", "#2", "#3"] {
            assert!(
                message.contains(number),
                "the refusal names {number}, in {message}"
            );
        }
        assert!(
            !message.starts_with("the order puts"),
            "no edge of the plan alone goes against a blocker, in {message}"
        );
        // The same message answers a chain, a table and a picture, so it names
        // the order and never one form of input.
        assert!(
            message.ends_with(
                "once the blockers each issue names join the order, so no step can start first"
            ),
            "the refusal names the order, in {message}"
        );
    }

    /// A plan of streams that names #1 and #2 in two orders, and a third
    /// stream that holds #5 alone.
    const KNOTTED_PLAN: &str = "\
| Stream | Order | Zone |
|--------|-------|------|
| S1 — first | #1 → #2 | a |
| S2 — second | #2 → #1 | b |
| S3 — third | #5 | c |
";

    #[test]
    fn a_knotted_order_that_the_issues_add_a_wait_to_is_refused_with_both() {
        // The reader of streams answers this plan while no issue names a
        // blocker. #5 names #6, so the answer must be a graph, and a graph
        // cannot hold the knot of #1 and #2. The refusal names the knot and
        // the wait, because the wait is what changed.
        let plan = crate::plan::parse(KNOTTED_PLAN).expect("the text is a plan");
        let answers = Answers::of(vec![open(6, &[])]);
        let result = settled(
            &crate::graph::of_streams(&plan),
            vec![open(1, &[]), open(2, &[]), open(5, &[6])],
            &answers,
        );
        let err = match result {
            Ok(_) => panic!("the knotted order was refused, and it answered"),
            Err(err) => err,
        };
        assert_eq!(
            err.to_string(),
            "the order returns to #1 and #2, and #5 waits for #6, \
             which only an order with no cycle can show. \
             Fix the order, or run wn --refresh to build a new plan"
        );
        assert_eq!(
            err.downcast_ref::<OrderError>(),
            Some(&OrderError::Knotted {
                cycle: numbers(&[1, 2]),
                step: issue(5),
                blocker: issue(6),
            }),
            "the refusal is an order error that names the knot and the wait, and it is {err:#}"
        );
    }
}
