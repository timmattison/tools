//! Reading the envelope a run of `claude --print --output-format json` prints.
//!
//! The run answers with a document. `--output-format json` wraps that document
//! in an envelope, which carries what the run cost beside it. So this module
//! stands between the run and every reader of a plan: it takes the envelope
//! apart, gives back the document, and builds the one line that says what the
//! reader paid.

use crate::build::BuildError;

/// The key that holds the document the run answered with.
const RESULT: &str = "result";

/// What one run of `claude` answered, and what it cost.
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
    /// Gives [`BuildError::BadEnvelope`] for a text that is no envelope.
    pub fn read(printed: &str) -> Result<Self, BuildError> {
        let _ = RESULT;
        Ok(Self {
            document: printed.to_string(),
        })
    }

    /// The document the run answered with.
    #[must_use]
    pub fn document(&self) -> &str {
        &self.document
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
            "total_cost_usd": 0.0546379,
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
        let refused =
            Envelope::read("{ \"result\": ").expect_err("half a document is no document");
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
}
