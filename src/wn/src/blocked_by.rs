//! What an issue says comes before it.
//!
//! A plan is a claim about the order of the work, and the issues make a claim
//! of their own. The `to-issues` skill writes the blockers of a slice under a
//! heading, one to a list item:
//!
//! ```text
//! ## Blocked by
//!
//! - #168
//! ```
//!
//! A plan once put #170 before #168, and #170 said that #168 blocked it. `wn`
//! answered the plan and sent the reader to #170. This module reads that claim
//! out of the body of an issue, so the answer can hold the plan to it.

use crate::chain::IssueNumber;

/// The numbers `body` names as work that comes before its issue, in the order
/// the body writes them, each one once.
#[must_use]
pub fn read(body: &str) -> Vec<IssueNumber> {
    let _ = body;
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One body, and the numbers it names as blockers.
    struct Case {
        name: &'static str,
        body: &'static str,
        numbers: &'static [u64],
    }

    /// One case for each syntactic form a blocker arrives in, and one for each
    /// form that names an issue and is not a blocker.
    ///
    /// `.claude/skills/plan-parallel-work/scripts/dependency-sections.test.ts`
    /// in timmattison/dotfiles holds the same forms for the gather script of the
    /// skill that writes the plans. A form added here belongs there too.
    const CASES: &[Case] = &[
        // The heading, in the spellings it arrives in.
        Case { name: "a list item under a Blocked by heading", body: "## Blocked by\n\n- #168\n", numbers: &[168] },
        Case { name: "a heading in title case", body: "## Blocked By\n\n- #7\n", numbers: &[7] },
        Case { name: "a heading with closing marks at a deeper level", body: "### Blocked by ###\n\n- #7\n", numbers: &[7] },
        Case {
            name: "a heading with a symbol in front and words after it",
            body: "## \u{26d4} Blocked by (do not start until these land)\n\n- #7\n",
            numbers: &[7],
        },
        Case { name: "a heading indented by three spaces", body: "   ## Blocked by\n\n- #7\n", numbers: &[7] },
        Case { name: "a setext heading", body: "Blocked by\n----------\n\n- #7\n", numbers: &[7] },
        Case { name: "a body with Windows line endings", body: "## Blocked by\r\n\r\n- #168\r\n", numbers: &[168] },
        Case {
            name: "a Depends on heading",
            body: "## Depends on\n\n**#17 (R2 upload).** There is nothing in the bucket until #17 runs.\n",
            numbers: &[17],
        },
        // The block under the heading, in the shapes it arrives in.
        Case { name: "every list marker", body: "## Blocked by\n\n* #1\n+ #2\n1. #3\n2) #4\n", numbers: &[1, 2, 3, 4] },
        Case { name: "a task list item, open and done", body: "## Blocked by\n\n- [ ] #5\n- [x] #6\n", numbers: &[5, 6] },
        Case {
            name: "a number with an annotation after it",
            body: "## Blocked by\n\n- #5 (test-taking UI)\n- #296 \u{2014} Create and switch workspaces\n",
            numbers: &[5, 296],
        },
        Case {
            name: "a list item that repeats the label",
            body: "## Blocked by\n\n- Blocked by #26 (slice 1 scaffolding)\n",
            numbers: &[26],
        },
        Case { name: "several numbers in one item", body: "## Blocked by\n\n- #3, #4 and #5\n", numbers: &[3, 4, 5] },
        Case { name: "a bare number as a paragraph", body: "## Blocked by\n\n#168\n", numbers: &[168] },
        Case {
            name: "a nested list item indented by four spaces",
            body: "## Blocked by\n\n- The solver\n    - #168\n",
            numbers: &[168],
        },
        Case {
            name: "a subheading inside the section",
            body: "## Blocked by\n\n### Before the solver\n\n- #11\n",
            numbers: &[11],
        },
        Case { name: "one number named twice", body: "## Blocked by\n\n- #9\n- #9 (again)\n", numbers: &[9] },
        // The forms that name an issue and are not a blocker.
        Case {
            name: "prose under the heading that names another issue",
            body: "## Blocked by\n\n- #168\n\nIt can run beside #169. Both touch the widget.\n",
            numbers: &[168],
        },
        Case {
            name: "a wrapped paragraph whose second line starts with a number",
            body: "## Blocked by\n\n- #168\n\nIt can run beside\n#169 when the two do not overlap.\n",
            numbers: &[168],
        },
        Case {
            name: "a list item whose second line starts with a number",
            body: "## Blocked by\n\n- The slope solver, which lands in\n  #168 first\n",
            numbers: &[],
        },
        Case { name: "no blocker at all", body: "## Blocked by\n\nNone - can start immediately.\n", numbers: &[] },
        Case {
            name: "a remark in parentheses",
            body: "## Blocked by\n\n(The surviving issues #38/#39 still need triage.)\n",
            numbers: &[],
        },
        Case {
            name: "a number in a code span",
            body: "## Blocked by\n\n- `#9` was the old number of this work\n",
            numbers: &[],
        },
        Case { name: "a heading inside a code fence", body: "```md\n## Blocked by\n\n- #9\n```\n", numbers: &[] },
        Case {
            name: "a list item inside a code fence under the heading",
            body: "## Blocked by\n\n~~~\n- #9\n~~~\n\n- #11\n",
            numbers: &[11],
        },
        Case { name: "a Blocks heading, which is the other direction", body: "## Blocks\n\n- #12\n", numbers: &[] },
        Case {
            name: "a heading of the same level ends the section",
            body: "## Blocked by\n\n- #11\n\n## Acceptance criteria\n\n- #12 still passes\n",
            numbers: &[11],
        },
        Case {
            name: "a Blocks heading at the level of the section ends it",
            body: "### Blocked by\n\n- #11\n\n### Blocks\n\n- #12\n",
            numbers: &[11],
        },
        Case {
            name: "a number of another repository",
            body: "## Blocked by\n\n- timmattison/muxiavelli#294\n- #5\n",
            numbers: &[5],
        },
        Case { name: "a number that is zero", body: "## Blocked by\n\n- #0\n", numbers: &[] },
        Case {
            name: "a number too large for any issue",
            body: "## Blocked by\n\n- #99999999999999999999999\n",
            numbers: &[],
        },
        // A label at the start of a block, with no heading.
        Case { name: "a bold label with a colon", body: "**Blocked by:** #12\n", numbers: &[12] },
        Case {
            name: "a bold label in a list item with two numbers",
            body: "- **Blocked by:** #11 (Phase 1 \u{2014} Scaffold) and #12 (Phase 2 \u{2014} routes).\n",
            numbers: &[11, 12],
        },
        Case {
            name: "a label in a block quote behind a symbol",
            body: "> **\u{1f6a7} Blocked by #12.** Land it before this one.\n",
            numbers: &[12],
        },
        Case { name: "a label that names no issue", body: "**Blocked by:** none.\n", numbers: &[] },
        Case {
            name: "a label at the start of a paragraph",
            body: "Blocked by #9. It edits crates/tsm/src/serve.rs.",
            numbers: &[9],
        },
        Case {
            name: "a label and a section, in the order the body writes them",
            body: "Depends on #3 for the schema.\n\n## Blocked by\n\n- #4\n",
            numbers: &[3, 4],
        },
        // The forms the gather script reads and this reader does not, because
        // neither of them says which issue waits.
        Case { name: "a parent heading", body: "## Parent\n\n#166\n", numbers: &[] },
        Case { name: "a parent label", body: "- **Parent:** #61.\n", numbers: &[] },
        Case {
            name: "a phrase in the middle of a sentence",
            body: "This slice is blocked by #9 until it lands.\n",
            numbers: &[],
        },
        Case {
            name: "a line of a tracker that says what blocks another issue",
            body: "- [ ] #12 \u{2014} Click to open *blocked by #11*\n",
            numbers: &[],
        },
    ];

    #[test]
    fn every_form_of_a_blocker_reads_as_its_case_says() {
        let wrong: Vec<String> = CASES
            .iter()
            .filter_map(|case| {
                let read: Vec<u64> = read(case.body).iter().map(|number| number.get()).collect();
                (read != case.numbers).then(|| {
                    format!(
                        "{}: read {read:?} and wanted {:?} out of {:?}",
                        case.name, case.numbers, case.body
                    )
                })
            })
            .collect();
        assert!(
            wrong.is_empty(),
            "{} of {} cases read wrong:\n{}",
            wrong.len(),
            CASES.len(),
            wrong.join("\n")
        );
    }
}
