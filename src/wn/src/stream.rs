//! Reading the stream of events a run of `claude` writes while it works.
//!
//! `--output-format stream-json` makes the run write one JSON object for each
//! event, one to a line, at the moment the event happens. Two of those kinds
//! matter here. An `assistant` event names the tool the run just reached for,
//! which is what the line on standard error says the run is doing. A `result`
//! event closes the run, and it is the envelope that carries the plan and the
//! price beside it.
//!
//! So this module stands between the pipe and the two readers of it. It reads
//! the pipe one line at a time on a thread of its own, hands each reach on as
//! it arrives, and keeps the envelope for the end.
//!
//! # Every other kind is read past
//!
//! One measured run wrote seven kinds of event, and it wrote four of them
//! before it wrote its first word. A reader that refused a kind it did not know
//! would break on the next version of `claude`, and the two kinds it does know
//! name themselves. So a line of an unknown kind, and a line that is no JSON at
//! all, both cost nothing.
//!
//! # Why the whole of the pipe is kept as well
//!
//! A run that failed says why on one of its two pipes, and a run that mixes the
//! two says it on this one. Such a reason is text and no event, so a reader
//! that kept the events alone would lose it. [`Transcript::printed`] is what
//! [`crate::build`] reads for a run like that.

use std::io::{BufRead, BufReader, Read};
use std::thread;

use serde_json::Value;

/// The key that names the kind of an event.
const TYPE: &str = "type";

/// The kind of the event that closes a run.
///
/// The last event of a run wears it, and it is the envelope
/// [`crate::envelope`] reads.
const RESULT: &str = "result";

/// The kind of the event that says what the run just did.
const ASSISTANT: &str = "assistant";

/// The key of an assistant event that holds the message.
const MESSAGE: &str = "message";

/// The key of a message that holds its blocks.
const CONTENT: &str = "content";

/// The kind of a block that names a tool the run reached for.
const TOOL_USE: &str = "tool_use";

/// The key of such a block that names the tool.
const NAME: &str = "name";

/// The key of such a block that holds what the tool was given.
const INPUT: &str = "input";

/// The keys of that input a person can read, in the order they are tried.
///
/// `description` is the words the run itself wrote about the reach, so it
/// stands first and it is the one a reader wants. The four after it are what
/// the tools that write no description are given: a path, a pattern, the name
/// of a skill. `command` stands last because it is the one that runs to many
/// lines, and a tool that carries a command carries a description beside it.
const DETAILS: [&str; 6] = [
    "description",
    "file_path",
    "pattern",
    "path",
    "skill",
    "command",
];

/// The mark between the name of a tool and the detail.
const NAMED: &str = ": ";

/// The characters of the longest detail the line carries.
///
/// The line is cut to the window it is painted in as well, so this cap is the
/// coarser of the two. It is here because the value of a key has no length a
/// caller can count on, and a line built out of a whole file would be built
/// once for every event of the run.
const WIDEST_DETAIL: usize = 60;

/// The mark that stands where a detail was cut.
const CUT: char = '…';

/// What one run of `claude` wrote on standard output.
#[derive(Debug, Default)]
pub struct Transcript {
    /// Every byte of it, as the run wrote them.
    printed: String,
    /// The last line whose kind is [`RESULT`], for a run that wrote one.
    envelope: Option<String>,
}

impl Transcript {
    /// The envelope of the run, or everything it printed when it wrote none.
    ///
    /// A run that wrote no envelope answered no plan, and the words it did
    /// write are what the refusal must quote. So the fallback is not a guess at
    /// an envelope: it is what a reader of one is handed to complain about.
    #[must_use]
    pub fn envelope(&self) -> &str {
        self.envelope.as_deref().unwrap_or(&self.printed)
    }

    /// Everything the run wrote on standard output.
    #[must_use]
    pub fn printed(&self) -> &str {
        &self.printed
    }
}

/// Read `pipe` to its end on a thread of its own, telling `doing` what the run
/// reaches for as it reaches for it.
///
/// The thread is what makes the line move. A pipe nobody reads fills up and the
/// run blocks in it, and a pipe read at the end says nothing until the run is
/// over — which is the one moment the reader no longer needs telling.
///
/// `doing` is handed one reach at a time, newest last. It is never handed the
/// envelope: the line is cleared before the envelope is read.
pub fn read<R, F>(pipe: R, doing: F) -> thread::JoinHandle<Transcript>
where
    R: Read + Send + 'static,
    F: Fn(&str) + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut transcript = Transcript::default();
        let mut raw = Vec::new();
        loop {
            let more = reader.read_until(b'\n', &mut raw);
            if !raw.is_empty() {
                let line = String::from_utf8_lossy(&raw).into_owned();
                raw.clear();
                take(&mut transcript, &line, &doing);
                transcript.printed.push_str(&line);
            }
            // A read that failed and a read that found the end both end the
            // reading, and what was read up to that point is kept either way. A
            // pipe that broke is a run that ended early, the status of the run
            // is what says so, and the half line before the break can be the
            // one that says why.
            if !matches!(more, Ok(read) if read > 0) {
                break;
            }
        }
        transcript
    })
}

/// Take `line` into `transcript`, and tell `doing` what it says the run does.
fn take(transcript: &mut Transcript, line: &str, doing: &impl Fn(&str)) {
    let clause = line.trim();
    let Ok(event) = serde_json::from_str::<Value>(clause) else {
        return;
    };
    match event.get(TYPE).and_then(Value::as_str) {
        Some(RESULT) => transcript.envelope = Some(clause.to_string()),
        Some(ASSISTANT) => {
            if let Some(reach) = reach_of(&event) {
                doing(&reach);
            }
        }
        _ => {}
    }
}

/// The tool `event` says the run reached for, and the words it wrote for that
/// reach.
///
/// One message carries more than one block, and a run that asks for two tools
/// at once writes both in one event. The last of them is the newest, so it is
/// the one the line names.
///
/// A message that names no tool at all gives nothing. Such an event is the run
/// thinking or the run writing a sentence, and neither one says what it is
/// doing.
fn reach_of(event: &Value) -> Option<String> {
    let blocks = event.get(MESSAGE)?.get(CONTENT)?.as_array()?;
    let tool = blocks
        .iter()
        .rev()
        .find(|block| block.get(TYPE).and_then(Value::as_str) == Some(TOOL_USE))?;
    let name = tool.get(NAME).and_then(Value::as_str)?;
    Some(match detail_of(tool.get(INPUT)) {
        Some(detail) => format!("{name}{NAMED}{detail}"),
        None => name.to_string(),
    })
}

/// The one readable thing `input` holds, for an input that holds one.
fn detail_of(input: Option<&Value>) -> Option<String> {
    let input = input?;
    DETAILS
        .iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .map(opening_line)
        .filter(|detail| !detail.is_empty())
}

/// The first line of `text`, cut to [`WIDEST_DETAIL`] characters.
///
/// One line, because the words go on a line that is one line. A message with a
/// newline in it makes the progress bar as many lines tall, and the frames then
/// smear down the terminal rather than replacing each other.
///
/// Cut by characters and never by bytes. A path, a pattern and a description
/// all carry whatever the reader typed, and a cut through the middle of a
/// character is a run that stops rather than a line that is one word short.
fn opening_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= WIDEST_DETAIL {
        return line.to_string();
    }
    let kept: String = line.chars().take(WIDEST_DETAIL.saturating_sub(1)).collect();
    format!("{kept}{CUT}")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// The stream of one measured run, with the lines and the fields that name
    /// the machine it ran on taken out.
    ///
    /// A fixture rather than a hand-written stream, because the shape is the
    /// whole point: a reader built against a guess at the shape reads nothing
    /// on the day it meets a real run. The run reached for `Bash` one time,
    /// with `echo hi`, and it wrote a description of that reach.
    const MEASURED: &str = include_str!("../fixtures/claude-stream.jsonl");

    /// The reach that run wrote.
    const MEASURED_REACH: &str = "Bash: Output \"hi\"";

    /// The document that run answered with.
    const MEASURED_DOCUMENT: &str = "Command executed successfully — the output is `hi`.";

    /// Read `text` as a stream, and give back the transcript and every reach it
    /// handed on, in order.
    fn stream(text: &str) -> (Transcript, Vec<String>) {
        let reaches = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&reaches);
        let read = read(std::io::Cursor::new(text.to_string()), move |reach| {
            kept.lock()
                .expect("no test of this file panics while it holds the lock")
                .push(reach.to_string());
        });
        let transcript = read.join().expect("the reader thread stands");
        let reaches = reaches
            .lock()
            .expect("the reader thread let the lock go")
            .clone();
        (transcript, reaches)
    }

    /// A pipe that hands over `said` and then breaks.
    ///
    /// A pipe of a child breaks when the child is killed, which is what the
    /// deadline of a run does. What the run wrote before that moment is what a
    /// refusal quotes, so it has to survive the break.
    struct Broken {
        /// What the pipe hands over before it breaks.
        said: Vec<u8>,
    }

    impl Read for Broken {
        fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
            if self.said.is_empty() {
                return Err(std::io::Error::other("the pipe broke"));
            }
            let taken = self.said.len().min(into.len());
            into[..taken].copy_from_slice(&self.said[..taken]);
            self.said.drain(..taken);
            Ok(taken)
        }
    }

    /// One assistant event that reached for `tool` with `description`.
    fn reaching_for(tool: &str, description: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "name": tool,
                    "input": { "description": description },
                }],
            },
        })
        .to_string()
    }

    #[test]
    fn the_envelope_is_the_last_line_of_the_kind_that_closes_a_run() {
        let (transcript, _) = stream(MEASURED);
        let envelope = transcript.envelope();
        assert_eq!(envelope.lines().count(), 1, "{envelope}");
        assert!(envelope.contains(MEASURED_DOCUMENT), "{envelope}");
    }

    #[test]
    fn the_reach_of_a_measured_run_is_the_tool_and_the_words_it_wrote() {
        let (_, reaches) = stream(MEASURED);
        assert_eq!(reaches, vec![MEASURED_REACH.to_string()]);
    }

    #[test]
    fn a_run_that_thinks_and_a_run_that_writes_reach_for_nothing() {
        // Both stand in the measured stream, and neither one says what the run
        // is doing. A line that named them would say "the run is thinking" for
        // as long as the run held one API call open, which is the same lie the
        // constant told.
        let (_, reaches) = stream(MEASURED);
        assert_eq!(reaches.len(), 1, "{reaches:?}");
    }

    #[test]
    fn every_line_the_run_wrote_is_kept() {
        let (transcript, _) = stream(MEASURED);
        assert_eq!(transcript.printed(), MEASURED);
    }

    #[test]
    fn the_reaches_arrive_in_the_order_the_run_made_them() {
        let text = [
            reaching_for("Read", "Read the open issues"),
            reaching_for("Bash", "Check wn CLI flags"),
        ]
        .join("\n");
        let (_, reaches) = stream(&text);
        assert_eq!(
            reaches,
            vec![
                "Read: Read the open issues".to_string(),
                "Bash: Check wn CLI flags".to_string()
            ]
        );
    }

    #[test]
    fn the_last_tool_of_one_event_is_the_one_the_line_names() {
        // A run that asks for two tools at once writes both blocks in one
        // event, and the newest of them is the one a reader wants.
        let event = serde_json::json!({
            "type": "assistant",
            "message": { "content": [
                { "type": "tool_use", "name": "Read", "input": { "file_path": "TODO.md" } },
                { "type": "tool_use", "name": "Glob", "input": { "pattern": "src/**/*.rs" } },
            ] },
        })
        .to_string();
        let (_, reaches) = stream(&event);
        assert_eq!(reaches, vec!["Glob: src/**/*.rs".to_string()]);
    }

    #[test]
    fn a_tool_that_wrote_no_description_is_named_by_what_it_was_given() {
        for (input, named) in [
            (
                serde_json::json!({ "file_path": "README.md" }),
                "R: README.md",
            ),
            (serde_json::json!({ "pattern": "fn main" }), "R: fn main"),
            (serde_json::json!({ "path": "src" }), "R: src"),
            (
                serde_json::json!({ "skill": "review-time" }),
                "R: review-time",
            ),
            (
                serde_json::json!({ "command": "cargo test" }),
                "R: cargo test",
            ),
        ] {
            let event = serde_json::json!({
                "type": "assistant",
                "message": { "content": [
                    { "type": "tool_use", "name": "R", "input": input },
                ] },
            })
            .to_string();
            let (_, reaches) = stream(&event);
            assert_eq!(reaches, vec![named.to_string()]);
        }
    }

    #[test]
    fn the_words_the_run_wrote_stand_in_front_of_what_the_tool_was_given() {
        let event = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{
                "type": "tool_use",
                "name": "Bash",
                "input": { "command": "gh issue list", "description": "List the open issues" },
            }] },
        })
        .to_string();
        let (_, reaches) = stream(&event);
        assert_eq!(reaches, vec!["Bash: List the open issues".to_string()]);
    }

    #[test]
    fn a_tool_that_was_given_nothing_a_reader_reads_is_named_alone() {
        let event = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{
                "type": "tool_use",
                "name": "TodoWrite",
                "input": { "todos": [] },
            }] },
        })
        .to_string();
        let (_, reaches) = stream(&event);
        assert_eq!(reaches, vec!["TodoWrite".to_string()]);
    }

    #[test]
    fn a_detail_of_nothing_but_space_names_the_tool_alone() {
        let event = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{
                "type": "tool_use",
                "name": "Bash",
                "input": { "description": "   " },
            }] },
        })
        .to_string();
        let (_, reaches) = stream(&event);
        assert_eq!(reaches, vec!["Bash".to_string()]);
    }

    #[test]
    fn a_detail_of_many_lines_reaches_the_line_as_one() {
        // The words go on a line that is one line. A message with a newline in
        // it makes the bar as many lines tall, and the frames then smear down
        // the terminal rather than replacing each other.
        let event = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{
                "type": "tool_use",
                "name": "Bash",
                "input": { "command": "set -e\ncargo test\ncargo clippy\n" },
            }] },
        })
        .to_string();
        let (_, reaches) = stream(&event);
        assert_eq!(reaches, vec!["Bash: set -e".to_string()]);
    }

    #[test]
    fn a_detail_longer_than_the_cap_is_cut_and_says_so() {
        let long = "a".repeat(WIDEST_DETAIL * 2);
        let event = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{
                "type": "tool_use",
                "name": "Bash",
                "input": { "description": long },
            }] },
        })
        .to_string();
        let (_, reaches) = stream(&event);
        let reach = reaches.first().expect("the run reached for a tool");
        let detail = reach
            .strip_prefix("Bash: ")
            .expect("the tool names itself first");
        assert_eq!(detail.chars().count(), WIDEST_DETAIL);
        assert!(detail.ends_with(CUT), "{detail}");
    }

    #[test]
    fn a_detail_is_cut_between_characters_and_never_through_one() {
        // A path, a pattern and a description all carry whatever the reader
        // typed. A cut through the middle of a character is a run that stops
        // rather than a line that is one word short.
        for wide in ["日本語", "🎉", "café"] {
            // One character over the cap at the least, whatever the bytes of
            // it weigh. A `repeat` of the cap itself leaves the widest of these
            // exactly at the cap, and a test that asked for a cut there would
            // be asking about a text nothing cuts.
            let long = wide.repeat(WIDEST_DETAIL + 1);
            let event = serde_json::json!({
                "type": "assistant",
                "message": { "content": [{
                    "type": "tool_use",
                    "name": "Read",
                    "input": { "file_path": long },
                }] },
            })
            .to_string();
            let (_, reaches) = stream(&event);
            let reach = reaches.first().expect("the run reached for a tool");
            let detail = reach
                .strip_prefix("Read: ")
                .expect("the tool names itself first");
            assert_eq!(detail.chars().count(), WIDEST_DETAIL, "{detail}");
            assert!(detail.ends_with(CUT), "{detail}");
        }
    }

    #[test]
    fn a_line_of_a_kind_the_reader_does_not_know_costs_nothing() {
        // One measured run wrote seven kinds of event, and four of them stood
        // in front of the first word it said. A reader that refused one would
        // break on the next version of `claude`.
        let text = [
            serde_json::json!({ "type": "rate_limit_event", "rate_limit_info": {} }).to_string(),
            reaching_for("Bash", "Check wn CLI flags"),
        ]
        .join("\n");
        let (_, reaches) = stream(&text);
        assert_eq!(reaches, vec!["Bash: Check wn CLI flags".to_string()]);
    }

    #[test]
    fn a_line_that_is_no_json_at_all_costs_nothing_and_is_still_kept() {
        // A `claude` that mixes its two pipes writes the reason it failed here,
        // and that reason is text and no event. The refusal of such a run
        // quotes it, so the line has to survive the reading.
        let text = format!(
            "the model is overloaded\n{}\n",
            reaching_for("Bash", "Wait")
        );
        let (transcript, reaches) = stream(&text);
        assert_eq!(reaches, vec!["Bash: Wait".to_string()]);
        assert!(
            transcript.printed().contains("the model is overloaded"),
            "{}",
            transcript.printed()
        );
    }

    #[test]
    fn a_pipe_that_broke_keeps_what_it_carried_before_it_broke() {
        // The deadline of a run kills the child, which breaks this pipe. The
        // words in it are the reason such a run gives, and a reader that
        // dropped the half line it was in the middle of would lose them.
        let said = "the model is overloaded";
        let read = read(
            Broken {
                said: said.as_bytes().to_vec(),
            },
            |_| {},
        );
        let transcript = read.join().expect("the reader thread stands");
        assert_eq!(transcript.printed(), said);
    }

    #[test]
    fn a_run_that_wrote_no_envelope_hands_on_everything_it_wrote() {
        let text = "the model is overloaded\n";
        let (transcript, _) = stream(text);
        assert_eq!(transcript.envelope(), text);
    }

    #[test]
    fn the_last_envelope_of_a_stream_is_the_one_that_stands() {
        let text = [
            serde_json::json!({ "type": "result", "result": "the first" }).to_string(),
            serde_json::json!({ "type": "result", "result": "the last" }).to_string(),
        ]
        .join("\n");
        let (transcript, _) = stream(&text);
        assert!(
            transcript.envelope().contains("the last"),
            "{}",
            transcript.envelope()
        );
        assert!(
            !transcript.envelope().contains("the first"),
            "{}",
            transcript.envelope()
        );
    }

    #[test]
    fn a_run_that_wrote_nothing_hands_on_nothing() {
        let (transcript, reaches) = stream("");
        assert_eq!(transcript.printed(), "");
        assert_eq!(transcript.envelope(), "");
        assert!(reaches.is_empty(), "{reaches:?}");
    }

    #[test]
    fn a_last_line_with_no_newline_after_it_is_still_a_line() {
        let text = serde_json::json!({ "type": "result", "result": "the plan" }).to_string();
        let (transcript, _) = stream(&text);
        assert_eq!(transcript.envelope(), text);
    }
}
