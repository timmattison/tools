//! A runner for tests. It answers a known command and records each call.

use std::sync::Mutex;
use std::time::Duration;

use super::{CommandOutput, RunError, Runner};

/// A runner that answers from a table and never starts a process.
#[derive(Debug, Default)]
pub struct FakeRunner {
    answers: Vec<(Vec<String>, Result<CommandOutput, RunError>)>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl FakeRunner {
    /// Makes a runner with no answer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the answer for `argv`.
    #[must_use]
    pub fn answer(mut self, argv: &[&str], result: Result<CommandOutput, RunError>) -> Self {
        let argv = argv.iter().map(ToString::to_string).collect();
        self.answers.push((argv, result));
        self
    }

    /// Gives each argv that ran, in order.
    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
}

impl Runner for FakeRunner {
    fn run(&self, argv: &[String], _limit: Duration) -> Result<CommandOutput, RunError> {
        self.calls.lock().unwrap().push(argv.to_vec());
        self.answers
            .iter()
            .find(|(known, _)| known.as_slice() == argv)
            .map_or_else(
                || panic!("unexpected command: {argv:?}"),
                |(_, result)| result.clone(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn answers_a_known_argv() {
        let runner = FakeRunner::new().answer(&["a", "b"], Ok(CommandOutput::ok("hello")));
        let output = runner.run(&argv(&["a", "b"]), Duration::from_secs(1));
        assert_eq!(output, Ok(CommandOutput::ok("hello")));
    }

    #[test]
    fn answers_an_error() {
        let error = RunError::NotInstalled {
            program: "a".to_string(),
        };
        let runner = FakeRunner::new().answer(&["a"], Err(error.clone()));
        assert_eq!(runner.run(&argv(&["a"]), Duration::from_secs(1)), Err(error));
    }

    #[test]
    fn records_each_call_in_order() {
        let runner = FakeRunner::new()
            .answer(&["a"], Ok(CommandOutput::ok("")))
            .answer(&["b"], Ok(CommandOutput::ok("")));
        let limit = Duration::from_secs(1);
        runner.run(&argv(&["b"]), limit).unwrap();
        runner.run(&argv(&["a"]), limit).unwrap();
        assert_eq!(runner.calls(), vec![argv(&["b"]), argv(&["a"])]);
    }

    #[test]
    fn answers_the_same_argv_twice() {
        let runner = FakeRunner::new().answer(&["a"], Ok(CommandOutput::ok("x")));
        let limit = Duration::from_secs(1);
        assert_eq!(runner.run(&argv(&["a"]), limit).unwrap().stdout, "x");
        assert_eq!(runner.run(&argv(&["a"]), limit).unwrap().stdout, "x");
        assert_eq!(runner.calls().len(), 2);
    }

    #[test]
    #[should_panic(expected = "unexpected command")]
    fn panics_on_an_unknown_argv() {
        let runner = FakeRunner::new();
        let _ = runner.run(&argv(&["nope"]), Duration::from_secs(1));
    }
}
