//! Reading the envelope that closes a run of `claude`.
//!
//! The run answers with a document, and the envelope wraps that document and
//! carries what the run cost beside it. So this module stands between the run
//! and every reader of a plan: it takes the envelope apart and gives back the
//! document.
//!
//! The envelope arrives as the last line of the stream of events the run
//! writes, and [`crate::stream`] is what picks that line out. This module is
//! handed the one line and never the stream.
//!
//! # A run that failed prints an envelope as well
//!
//! Such an envelope carries the reason the run gives in its `result`, and it
//! carries what the run cost beside it. So the reading and the judging are two
//! steps here. [`Envelope::read`] reads every envelope, the one a failure
//! prints included, and [`Envelope::answer`] is what tells a plan from a
//! reason. A reader who pays for a run that failed still learns the price,
//! because [`Envelope::report`] stands on the near side of that gate.
//!
//! # Two kinds of field, and two kinds of strictness
//!
//! `result` is the plan, so a text that carries none of it is a refusal. Every
//! other field is a number about the run, and a missing one costs a clause of
//! one line. A refusal there would throw away a plan the reader already paid
//! for, so an absent number leaves its clause out and the plan still stands.
//!
//! # The numbers cover the subagents
//!
//! The `plan-parallel-work` skill dispatches a subagent when the backlog holds
//! eight open issues or more, so a report of the parent run alone would
//! under-report every large backlog.
//!
//! One measured run settles it. A parent on `claude-haiku-4-5` was asked to
//! dispatch a subagent on `opus`. Its `modelUsage` named `claude-haiku-4-5`
//! ($0.0671493) and `claude-opus-5[1m]` ($0.2088975), and its
//! `total_cost_usd` was $0.2760468, which is the sum of the two to the last
//! digit. So `modelUsage` names every model of a run, subagents included, and
//! `total_cost_usd` is the sum over it. That run is the fixture of this
//! module.
//!
//! The top-level `usage` of that same envelope reported 10 input tokens and 57
//! output tokens, against the 30 and the 417 that `modelUsage` gives for the
//! parent model alone. It counts the last turn of the parent and nothing else,
//! so the token counts of this report come out of `modelUsage`.

use serde_json::Value;

use crate::build::{refusal_of, BuildError};
use crate::chain::Snippet;

/// The key that holds the document the run answered with.
const RESULT: &str = "result";

/// The key that says the run failed, whatever its exit status was.
const IS_ERROR: &str = "is_error";

/// The key that holds what the whole run cost, in dollars.
const TOTAL_COST_USD: &str = "total_cost_usd";

/// The key that holds one entry for each model the run used.
const MODEL_USAGE: &str = "modelUsage";

/// The key that holds how long the run took, in milliseconds.
const DURATION_MS: &str = "duration_ms";

/// The key of a model entry that holds what that model cost, in dollars.
const COST_USD: &str = "costUSD";

/// The key of a model entry that holds the tokens it was sent.
const INPUT_TOKENS: &str = "inputTokens";

/// The key of a model entry that holds the tokens it wrote.
const OUTPUT_TOKENS: &str = "outputTokens";

/// The key of a model entry that holds the tokens it read out of the cache.
const CACHE_READ_TOKENS: &str = "cacheReadInputTokens";

/// The key of a model entry that holds the tokens it wrote into the cache.
const CACHE_WRITE_TOKENS: &str = "cacheCreationInputTokens";

/// The words the report line opens with.
const OPENING: &str = "plan:";

/// The mark between two clauses of the report line.
const BETWEEN: &str = " \u{b7} ";

/// The dollars below which a report writes four decimal places.
///
/// Two places is what a reader wants of a run that cost dollars. A run of a
/// small backlog on a small model costs less than a cent, and `$0.00` reads as
/// a run that was free.
const CENT: f64 = 0.01;

/// The milliseconds of a second.
const A_SECOND: u64 = 1_000;

/// A thousand, which is where a written count picks up a `k`.
const A_THOUSAND: u64 = 1_000;

/// A million, which is where a written count picks up an `M`.
const A_MILLION: u64 = 1_000_000;

/// The seconds of a minute.
const A_MINUTE: u64 = 60;

/// The seconds of an hour.
const AN_HOUR: u64 = 3_600;

/// What one model of a run was given, and what it cost.
///
/// One struct for each entry of `modelUsage`, because a run uses more than one
/// model whenever the skill dispatches a subagent, and a report that named the
/// parent alone would name the cheaper half of what the reader paid for.
#[derive(Debug)]
struct Model {
    /// The model id, as `modelUsage` keys the entry by.
    id: String,
    /// What this model cost, in dollars.
    cost: f64,
    /// The tokens it was sent.
    input: u64,
    /// The tokens it wrote.
    output: u64,
    /// The tokens it read out of the cache.
    cache_read: u64,
    /// The tokens it wrote into the cache.
    cache_write: u64,
}

/// What one run of `claude` answered, and what it cost.
#[derive(Debug)]
pub struct Envelope {
    /// The document the run answered with.
    document: String,
    /// Whether the run says it failed, whatever its exit status was.
    failed: bool,
    /// What the whole run cost, in dollars, for an envelope that says.
    dollars: Option<f64>,
    /// Every model the run used, the dearest first.
    models: Vec<Model>,
    /// How long the run took, in milliseconds, for an envelope that says.
    milliseconds: Option<u64>,
}

impl Envelope {
    /// The envelope `printed` holds.
    ///
    /// It reads the envelope and it judges none of it. A run that failed
    /// prints an envelope as well, and that envelope carries the reason the
    /// run gives beside what the run cost. So a failure reads here like every
    /// other answer, and [`Self::answer`] is where the two part company.
    ///
    /// # Errors
    ///
    /// Gives [`BuildError::BadEnvelope`] for a text that is no JSON envelope,
    /// and for one whose `result` is absent or is no string.
    pub fn read(printed: &str) -> Result<Self, BuildError> {
        let document: Value =
            serde_json::from_str(printed).map_err(|cause| BuildError::BadEnvelope {
                text: Snippet::new(printed),
                cause: cause.to_string(),
            })?;
        let Some(result) = document.get(RESULT).and_then(Value::as_str) else {
            return Err(BuildError::BadEnvelope {
                text: Snippet::new(printed),
                cause: format!("it carries no {RESULT} string, which is where the plan stands"),
            });
        };
        Ok(Self {
            document: result.to_string(),
            failed: document
                .get(IS_ERROR)
                .and_then(Value::as_bool)
                .unwrap_or(false),
            dollars: document.get(TOTAL_COST_USD).and_then(Value::as_f64),
            models: models_of(document.get(MODEL_USAGE)),
            milliseconds: document.get(DURATION_MS).and_then(Value::as_u64),
        })
    }

    /// The document the run answered with.
    ///
    /// The document stands behind a `Result` for one reason: no caller can
    /// reach a refusal and hand it on as a plan.
    ///
    /// # Errors
    ///
    /// Gives the refusals of [`refusal_of`] for an envelope whose `is_error`
    /// is true: its `result` then holds the reason the run gives and never a
    /// plan, and a reader handed that document would get the refusal of the
    /// plan reader naming that reason as though somebody had pasted it.
    pub fn answer(&self) -> Result<&str, BuildError> {
        if self.failed {
            return Err(refusal_of(&self.document));
        }
        Ok(&self.document)
    }

    /// The one line that says what the run cost, or nothing for an envelope
    /// that carries no number at all.
    ///
    /// `effort` is the level the run was asked for, which the envelope does
    /// not carry: no field of it names one, so the caller passes the level it
    /// asked for and a run that asked for none earns no such words.
    #[must_use]
    pub fn report(&self, effort: Option<&str>) -> Option<String> {
        let clauses: Vec<String> = [
            self.dollars.map(dollars),
            self.models_clause(effort),
            self.tokens_clause(),
            self.milliseconds.map(elapsed),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!clauses.is_empty()).then(|| format!("{OPENING} {}", clauses.join(BETWEEN)))
    }

    /// The clause that names the models and the level they ran at.
    fn models_clause(&self, effort: Option<&str>) -> Option<String> {
        let named = self
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        match (named.is_empty(), effort) {
            (true, None) => None,
            (true, Some(effort)) => Some(format!("effort {effort}")),
            (false, None) => Some(named),
            (false, Some(effort)) => Some(format!("{named} at effort {effort}")),
        }
    }

    /// The clause that names the tokens, summed over every model of the run.
    ///
    /// A part that is zero is left out. A run always reads the cache and
    /// writes it, and a zero there is a number the reader has no use for.
    fn tokens_clause(&self) -> Option<String> {
        let parts: Vec<String> = [
            (self.tokens(|model| model.input), "in"),
            (self.tokens(|model| model.output), "out"),
            (self.tokens(|model| model.cache_read), "cache read"),
            (self.tokens(|model| model.cache_write), "cache write"),
        ]
        .into_iter()
        .filter(|(counted, _)| *counted > 0)
        .map(|(counted, word)| format!("{} {word}", count(counted)))
        .collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }

    /// One token count of every model of the run, added up.
    ///
    /// The sum saturates rather than wrapping. No run reaches the top of a
    /// `u64`, and a report that wrapped would name a number smaller than the
    /// one model it came from.
    fn tokens(&self, of: impl Fn(&Model) -> u64) -> u64 {
        self.models
            .iter()
            .map(of)
            .fold(0_u64, |sum, counted| sum.saturating_add(counted))
    }
}

/// Every model of `usage`, the `modelUsage` of an envelope, the dearest first.
///
/// The order is the order a reader who thinks the plan cost too much reads in,
/// and two models that cost the same stand by their ids, so one envelope
/// always builds one line.
///
/// An absent `modelUsage`, and an entry that names no cost, both give a model
/// of no cost rather than a refusal. The plan is already paid for.
fn models_of(usage: Option<&Value>) -> Vec<Model> {
    let Some(entries) = usage.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut models: Vec<Model> = entries
        .iter()
        .map(|(id, entry)| Model {
            id: id.clone(),
            cost: entry.get(COST_USD).and_then(Value::as_f64).unwrap_or(0.0),
            input: counted(entry, INPUT_TOKENS),
            output: counted(entry, OUTPUT_TOKENS),
            cache_read: counted(entry, CACHE_READ_TOKENS),
            cache_write: counted(entry, CACHE_WRITE_TOKENS),
        })
        .collect();
    models.sort_by(|left, right| {
        right
            .cost
            .partial_cmp(&left.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    models
}

/// The count `key` of `entry` holds, or zero.
fn counted(entry: &Value, key: &str) -> u64 {
    entry.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// `amount` dollars, as the report writes them.
///
/// Two decimal places, and four for an amount under a cent.
fn dollars(amount: f64) -> String {
    if amount < CENT {
        format!("${amount:.4}")
    } else {
        format!("${amount:.2}")
    }
}

/// `counted` tokens, as the report writes them.
///
/// A run reads tens of thousands of tokens out of the cache, and six digits of
/// them say nothing a reader acts on. So a count of a thousand and up is
/// written short: `9.9k`, `118k`, `1.2M`.
fn count(counted: u64) -> String {
    match counted {
        counted if counted < A_THOUSAND => counted.to_string(),
        counted if counted < 10 * A_THOUSAND => short(counted, A_THOUSAND, true, "k"),
        counted if counted < A_MILLION => short(counted, A_THOUSAND, false, "k"),
        counted if counted < 10 * A_MILLION => short(counted, A_MILLION, true, "M"),
        counted => short(counted, A_MILLION, false, "M"),
    }
}

/// `counted` over `divisor`, cut rather than rounded, with `suffix` after it
/// and one decimal place when `decimal` says.
///
/// Cut and never rounded, for two reasons. A report must not name a number the
/// run did not reach, and a cut number always stays inside the step that chose
/// it: 999999 tokens are `999k` and never `1000k`.
///
/// The arithmetic is whole-number arithmetic throughout, so no count is
/// written through an `f64` that cannot hold it.
fn short(counted: u64, divisor: u64, decimal: bool, suffix: &str) -> String {
    if !decimal {
        return format!("{}{suffix}", counted / divisor);
    }
    let tenths = counted / (divisor / 10);
    format!("{}.{}{suffix}", tenths / 10, tenths % 10)
}

/// `milliseconds`, as the report writes them.
///
/// Under a minute it writes seconds with one decimal place, because a fast run
/// and a slow one differ by fractions there. A minute and up it writes whole
/// minutes and seconds, and an hour and up it writes hours as well.
///
/// The seconds are cut and never rounded, for the reason [`short`] gives: a
/// run of 59900 milliseconds is `59.9s` and never `60.0s`, which is a minute
/// written in the shape of the branch below a minute.
fn elapsed(milliseconds: u64) -> String {
    let whole = milliseconds / A_SECOND;
    if whole < A_MINUTE {
        let tenths = milliseconds / (A_SECOND / 10);
        return format!("{}.{}s", tenths / 10, tenths % 10);
    }
    let hours = whole / AN_HOUR;
    let minutes = (whole % AN_HOUR) / A_MINUTE;
    let seconds = whole % A_MINUTE;
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An envelope of the shape a run really prints, with `document` in its
    /// `result`.
    fn envelope(document: &str) -> String {
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": document,
            "total_cost_usd": 0.054_637_9,
            "duration_ms": 1886,
            "num_turns": 1,
        })
        .to_string()
    }

    #[test]
    fn the_result_field_is_the_document_and_the_envelope_is_not() {
        // The whole point of the envelope: the plan is one field of it, and a
        // reader handed the envelope itself gets JSON that is no plan.
        let read = Envelope::read(&envelope("# The plan\n")).expect("the envelope reads");
        assert_eq!(
            read.answer().expect("the envelope holds a plan"),
            "# The plan\n"
        );
    }

    #[test]
    fn half_an_envelope_is_a_refusal() {
        let refused = Envelope::read("{ \"result\": ").expect_err("half a document is no document");
        assert!(
            matches!(refused, BuildError::BadEnvelope { .. }),
            "{refused:?}"
        );
    }

    #[test]
    fn an_envelope_with_no_result_is_a_refusal() {
        let refused = Envelope::read(r#"{"type":"result","duration_ms":12}"#)
            .expect_err("an envelope with no document holds no plan");
        assert!(
            matches!(refused, BuildError::BadEnvelope { .. }),
            "{refused:?}"
        );
    }

    #[test]
    fn a_result_that_is_no_string_is_a_refusal() {
        let refused = Envelope::read(r#"{"result":{"streams":[]}}"#)
            .expect_err("the document is a string, and this is an object");
        assert!(
            matches!(refused, BuildError::BadEnvelope { .. }),
            "{refused:?}"
        );
    }

    #[test]
    fn a_run_that_printed_nothing_is_a_refusal_that_names_claude() {
        // A run that exits 0 and prints nothing at all reaches this reader,
        // and the reader of the message has to know which program went quiet.
        let refused = Envelope::read("").expect_err("nothing is no envelope");
        assert!(
            matches!(refused, BuildError::BadEnvelope { .. }),
            "{refused:?}"
        );
        assert!(refused.to_string().contains("claude"), "{refused}");
    }

    /// An envelope of a run that says it failed, with `reason` in its
    /// `result`.
    ///
    /// It carries the numbers a failing run really carries. A run that failed
    /// after several turns spent money, and the reader paid for it.
    fn refused(reason: &str) -> String {
        serde_json::json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "result": reason,
            "total_cost_usd": 0.054_637_9,
            "duration_ms": 1886,
        })
        .to_string()
    }

    #[test]
    fn an_envelope_that_says_it_is_an_error_carries_that_reason() {
        // The run says it failed inside the envelope. Its `result` then holds
        // the reason and never a plan, so a reader handed that document would
        // get the refusal of the plan reader, naming the reason as though
        // somebody had pasted it.
        let read = Envelope::read(&refused("the model is overloaded")).expect("the envelope reads");
        assert_eq!(
            read.answer().expect_err("an error is no plan"),
            BuildError::Failed {
                said: "the model is overloaded".to_string()
            }
        );
    }

    #[test]
    fn an_envelope_of_a_run_that_could_not_log_in_names_claude_login() {
        let read =
            Envelope::read(&refused("Invalid API key · Please run /login")).expect("it reads");
        assert_eq!(
            read.answer().expect_err("no account is no plan"),
            BuildError::NotAuthenticated
        );
    }

    #[test]
    fn a_run_that_failed_still_says_what_it_cost() {
        // The reader pays for such a run as well. The report stands on the
        // near side of the gate `answer` holds, so the price is written
        // whether the run answered with a plan or with a reason.
        let read = Envelope::read(&refused("the model is overloaded")).expect("the envelope reads");
        assert_eq!(read.report(None), Some("plan: $0.05 · 1.8s".to_string()));
    }

    /// The envelope of one measured run, with a plan in its `result`.
    ///
    /// A recorded run and not a hand-written one, so a field this reader takes
    /// is a field a real `claude` really wrote. Only three values of it were
    /// changed: the `result`, which held the word `4`, and the two ids of the
    /// run, which name a session of one machine.
    ///
    /// The run was a parent on `claude-haiku-4-5` that dispatched a subagent
    /// on `opus`, so its `modelUsage` carries two models and its
    /// `total_cost_usd` is the sum over both.
    const MEASURED: &str = include_str!("../fixtures/claude-envelope.json");

    /// The envelope of [`MEASURED`].
    fn measured() -> Envelope {
        Envelope::read(MEASURED).expect("the measured envelope reads")
    }

    #[test]
    fn the_report_names_the_dollars_the_models_the_tokens_and_the_seconds() {
        // The whole line, from a recorded envelope. The reader pays for the
        // run and this line is the only place the price is written.
        assert_eq!(
            measured().report(Some("low")),
            Some(
                "plan: $0.28 · claude-opus-5[1m], claude-haiku-4-5 at effort low · \
                 32 in, 420 out, 94k cache read, 61k cache write · 1.3s"
                    .to_string()
            )
        );
    }

    #[test]
    fn an_envelope_of_two_models_names_both_the_dearest_first() {
        // A run that dispatched a subagent used two models, and the reader who
        // thinks the plan cost too much reads the expensive one first.
        let line = measured()
            .report(None)
            .expect("the envelope carries numbers");
        let opus = line.find("claude-opus-5[1m]").expect("the subagent model");
        let haiku = line.find("claude-haiku-4-5").expect("the parent model");
        assert!(opus < haiku, "{line}");
    }

    #[test]
    fn a_run_that_asked_for_no_effort_earns_no_such_words() {
        // The envelope carries no effort field, so the level is the caller's
        // to name. A line that named a level nobody chose is worth nothing.
        let line = measured()
            .report(None)
            .expect("the envelope carries numbers");
        assert!(!line.contains("effort"), "{line}");
    }

    #[test]
    fn a_run_that_cost_less_than_a_cent_keeps_the_digits_of_its_price() {
        // Two decimal places write such a run as $0.00, which reads as a run
        // that was free.
        let said = serde_json::json!({ "result": "x", "total_cost_usd": 0.004_637_9 }).to_string();
        assert_eq!(
            Envelope::read(&said)
                .expect("the envelope reads")
                .report(None),
            Some("plan: $0.0046".to_string())
        );
    }

    #[test]
    fn a_run_of_minutes_is_written_in_minutes_and_seconds() {
        for (milliseconds, written) in [
            (1_886_u64, "1.8s"),
            (59_900, "59.9s"),
            (60_000, "1m 0s"),
            (192_000, "3m 12s"),
            (3_852_000, "1h 4m 12s"),
        ] {
            let said =
                serde_json::json!({ "result": "x", "duration_ms": milliseconds }).to_string();
            assert_eq!(
                Envelope::read(&said)
                    .expect("the envelope reads")
                    .report(None),
                Some(format!("plan: {written}")),
                "{milliseconds} milliseconds"
            );
        }
    }

    #[test]
    fn a_count_of_a_thousand_and_up_is_written_short() {
        // A run reads tens of thousands of tokens out of the cache, and six
        // digits of them say nothing a reader acts on.
        let said = serde_json::json!({
            "result": "x",
            "modelUsage": {
                "claude-opus-5": {
                    "costUSD": 1.5,
                    "inputTokens": 118_000,
                    "outputTokens": 9_400,
                    "cacheReadInputTokens": 1_200_000,
                    "cacheCreationInputTokens": 999,
                }
            }
        })
        .to_string();
        assert_eq!(
            Envelope::read(&said)
                .expect("the envelope reads")
                .report(None),
            Some(
                "plan: claude-opus-5 · 118k in, 9.4k out, 1.2M cache read, 999 cache write"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_token_count_of_zero_is_left_out() {
        // A run always reads the cache and writes it, and a zero there is a
        // number the reader has no use for.
        let said = serde_json::json!({
            "result": "x",
            "modelUsage": { "claude-opus-5": { "costUSD": 1.5, "inputTokens": 12, "outputTokens": 34 } }
        })
        .to_string();
        assert_eq!(
            Envelope::read(&said)
                .expect("the envelope reads")
                .report(None),
            Some("plan: claude-opus-5 · 12 in, 34 out".to_string())
        );
    }

    #[test]
    fn an_envelope_that_carries_no_number_at_all_earns_no_line() {
        // A report is a courtesy. A missing number costs a clause, and a
        // missing everything costs the line, and neither costs the plan the
        // reader already paid for.
        assert_eq!(
            Envelope::read(r#"{"result":"the plan"}"#)
                .expect("the envelope reads")
                .report(None),
            None
        );
    }

    #[test]
    fn a_run_that_asked_for_an_effort_and_carries_no_model_still_names_the_level() {
        assert_eq!(
            Envelope::read(r#"{"result":"the plan"}"#)
                .expect("the envelope reads")
                .report(Some("high")),
            Some("plan: effort high".to_string())
        );
    }

    #[test]
    fn a_count_is_cut_and_never_rounded() {
        // A cut count stays inside the step that chose it. A rounded one does
        // not: 999999 would read as 1000k, which is a million written in the
        // shape of the step below a million.
        for (counted, written) in [
            (999_u64, "999"),
            (1_000, "1.0k"),
            (9_999, "9.9k"),
            (10_000, "10k"),
            (94_683, "94k"),
            (999_999, "999k"),
            (1_000_000, "1.0M"),
            (9_999_999, "9.9M"),
            (10_000_000, "10M"),
        ] {
            let said = serde_json::json!({
                "result": "x",
                "modelUsage": { "m": { "costUSD": 1.0, "inputTokens": counted } }
            })
            .to_string();
            let line = Envelope::read(&said)
                .expect("the envelope reads")
                .report(None)
                .expect("the envelope carries numbers");
            assert!(
                line.ends_with(&format!("{written} in")),
                "{counted}: {line}"
            );
        }
    }

    #[test]
    fn the_seconds_of_a_run_are_cut_and_never_rounded() {
        // 59900 milliseconds must not read as 60.0s, which is a minute written
        // in the shape of the branch below a minute.
        for (milliseconds, written) in [(59_900_u64, "59.9s"), (59_999, "59.9s")] {
            let said =
                serde_json::json!({ "result": "x", "duration_ms": milliseconds }).to_string();
            assert_eq!(
                Envelope::read(&said)
                    .expect("the envelope reads")
                    .report(None),
                Some(format!("plan: {written}")),
                "{milliseconds} milliseconds"
            );
        }
    }
}
