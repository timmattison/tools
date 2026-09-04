//! Reading the envelope a run of `claude --print --output-format json` prints.
//!
//! The run answers with a document. `--output-format json` wraps that document
//! in an envelope, which carries what the run cost beside it. So this module
//! stands between the run and every reader of a plan: it takes the envelope
//! apart and gives back the document.
//!
//! # Two kinds of field, and two kinds of strictness
//!
//! `result` is the plan, so a text that carries none of it is a refusal. Every
//! other field is a number about the run, and a missing one costs a clause of
//! one line. A refusal there would throw away a plan the reader already paid
//! for, so an absent number leaves its clause out and the plan still stands.

use serde_json::Value;

use crate::build::{refusal_of, BuildError};
use crate::chain::Snippet;

/// The key that holds the document the run answered with.
const RESULT: &str = "result";

/// The key that says the run failed, whatever its exit status was.
const IS_ERROR: &str = "is_error";

/// What one run of `claude` answered.
#[derive(Debug)]
pub struct Envelope {
    /// The document the run answered with.
    document: String,
}

impl Envelope {
    /// The envelope `printed` holds.
    ///
    /// # Errors
    ///
    /// Gives [`BuildError::BadEnvelope`] for a text that is no JSON envelope,
    /// and for one whose `result` is absent or is no string. Gives the
    /// refusals of [`refusal_of`] for an envelope whose `is_error` is true:
    /// its `result` then holds the reason the run gives and never a plan, and
    /// a reader handed that document would get the refusal of the plan reader
    /// naming that reason as though somebody had pasted it.
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
        if document
            .get(IS_ERROR)
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(refusal_of(result));
        }
        Ok(Self {
            document: result.to_string(),
        })
    }

    /// The document the run answered with.
    #[must_use]
    pub fn document(&self) -> &str {
        &self.document
    }

    /// The one line that says what the run cost, or nothing for an envelope
    /// that carries no number at all.
    ///
    /// `effort` is the level the run was asked for, which the envelope does
    /// not carry: no field of it names one, so the caller passes the level it
    /// asked for and a run that asked for none earns no such words.
    #[must_use]
    pub fn report(&self, effort: Option<&str>) -> Option<String> {
        let _ = effort;
        None
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
        assert_eq!(read.document(), "# The plan\n");
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

    #[test]
    fn an_envelope_that_says_it_is_an_error_carries_that_reason() {
        // The run exits 0 and says so inside the envelope. Its `result` then
        // holds the reason and never a plan, so a reader handed that document
        // would get the refusal of the plan reader, naming the reason as
        // though somebody had pasted it.
        let said = serde_json::json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "result": "the model is overloaded",
        })
        .to_string();
        let refused = Envelope::read(&said).expect_err("an error is no plan");
        assert_eq!(
            refused,
            BuildError::Failed {
                said: "the model is overloaded".to_string()
            }
        );
    }

    #[test]
    fn an_envelope_of_a_run_that_could_not_log_in_names_claude_login() {
        let said = serde_json::json!({
            "is_error": true,
            "result": "Invalid API key · Please run /login",
        })
        .to_string();
        assert_eq!(
            Envelope::read(&said).expect_err("no account is no plan"),
            BuildError::NotAuthenticated
        );
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
            (1_886_u64, "1.9s"),
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
}
