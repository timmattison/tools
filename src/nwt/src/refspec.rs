//! The fetch refspecs of the remotes of a repository, and the one question
//! that `nwt` asks them: to which ref does each remote map the branch
//! `refs/heads/<name>`?
//!
//! This file holds only the interface and its tests. The rules come in the
//! next change.

/// The pattern for `git config --get-regexp` that selects the fetch refspec of
/// each remote.
pub(crate) const REMOTE_FETCH_KEY_PATTERN: &str = r"^remote\..+\.fetch$";

/// A remote that maps `refs/heads/<name>` to a ref, and that ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MappedRef {
    /// The name of the remote, for example `origin`.
    pub(crate) remote: String,
    /// The ref where the fetch refspec of the remote puts the branch, for
    /// example `refs/remotes/origin/issue-33`.
    pub(crate) tracking_ref: String,
}

/// The fetch refspecs of each remote.
#[derive(Debug, Clone, Default)]
pub(crate) struct RemoteFetchRefspecs {}

impl RemoteFetchRefspecs {
    /// Read the output of `git config -z --get-regexp` for
    /// [`REMOTE_FETCH_KEY_PATTERN`].
    pub(crate) fn from_config_listing(_listing: &[u8]) -> Self {
        Self::default()
    }

    /// Each remote that maps `refs/heads/<name>` to a ref, with that ref.
    pub(crate) fn map_branch(&self, _name: &str) -> Vec<MappedRef> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default fetch refspec of `origin`.
    const DEFAULT_ORIGIN: (&str, &str) =
        ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*");

    /// The default fetch refspec of `upstream`.
    const DEFAULT_UPSTREAM: (&str, &str) = (
        "remote.upstream.fetch",
        "+refs/heads/*:refs/remotes/upstream/*",
    );

    /// The output of `git config -z --get-regexp` for `records`: each record
    /// is the key, a line break and the value, and a NUL ends it.
    fn listing(records: &[(&str, &str)]) -> Vec<u8> {
        records
            .iter()
            .flat_map(|(key, value)| format!("{key}\n{value}\0").into_bytes())
            .collect()
    }

    /// Map `name` through the fetch refspecs of `records`.
    fn map(records: &[(&str, &str)], name: &str) -> Vec<MappedRef> {
        RemoteFetchRefspecs::from_config_listing(&listing(records)).map_branch(name)
    }

    /// The answer for a remote `remote` that maps the branch to
    /// `tracking_ref`.
    fn mapped(remote: &str, tracking_ref: &str) -> MappedRef {
        MappedRef {
            remote: remote.to_owned(),
            tracking_ref: tracking_ref.to_owned(),
        }
    }

    /// The default refspec puts the branch under the name of the remote. A
    /// `/` in the branch name stays.
    #[test]
    fn the_default_refspec_maps_the_branch_under_the_name_of_the_remote() {
        assert_eq!(
            map(&[DEFAULT_ORIGIN], "issue-33"),
            vec![mapped("origin", "refs/remotes/origin/issue-33")]
        );
        assert_eq!(
            map(&[DEFAULT_ORIGIN], "feature/login"),
            vec![mapped("origin", "refs/remotes/origin/feature/login")]
        );
    }

    /// A leading `+` forces the update in a fetch. It does not change the
    /// ref that the refspec maps to.
    #[test]
    fn a_leading_plus_changes_nothing() {
        for refspec in [
            "refs/heads/*:refs/remotes/o/*",
            "+refs/heads/*:refs/remotes/o/*",
        ] {
            assert_eq!(
                map(&[("remote.o.fetch", refspec)], "issue-33"),
                vec![mapped("o", "refs/remotes/o/issue-33")],
                "{refspec}"
            );
        }
    }

    /// A refspec without `*` maps its own source and no other ref.
    #[test]
    fn an_exact_refspec_maps_only_its_own_branch() {
        let records = [
            (
                "remote.origin.fetch",
                "refs/heads/issue-33:refs/remotes/origin/kept",
            ),
            DEFAULT_UPSTREAM,
        ];

        assert_eq!(
            map(&records, "issue-33"),
            vec![
                mapped("origin", "refs/remotes/origin/kept"),
                mapped("upstream", "refs/remotes/upstream/issue-33"),
            ]
        );
        assert_eq!(
            map(&records, "issue-34"),
            vec![mapped("upstream", "refs/remotes/upstream/issue-34")]
        );
    }

    /// The `*` of a pattern matches the text between a prefix and a suffix.
    /// The destination takes that text in place of its own `*`, between its
    /// own prefix and suffix.
    #[test]
    fn a_pattern_puts_the_matched_text_between_its_prefix_and_its_suffix() {
        let records = [
            (
                "remote.o.fetch",
                "refs/heads/feat-*-wip:refs/remotes/o/f-*-w",
            ),
            DEFAULT_UPSTREAM,
        ];

        assert_eq!(
            map(&records, "feat-login-wip"),
            vec![
                mapped("o", "refs/remotes/o/f-login-w"),
                mapped("upstream", "refs/remotes/upstream/feat-login-wip"),
            ]
        );
        for name in ["feat-login", "login-wip"] {
            assert_eq!(
                map(&records, name),
                vec![mapped("upstream", &format!("refs/remotes/upstream/{name}"))],
                "{name} has only one of the two ends of the pattern"
            );
        }
    }

    /// A pattern with only a prefix around its `*` maps each name that starts
    /// with that prefix.
    #[test]
    fn a_pattern_with_a_prefix_only_keeps_the_rest_of_the_name() {
        assert_eq!(
            map(
                &[("remote.o.fetch", "refs/heads/feat-*:refs/remotes/o/f-*")],
                "feat-login"
            ),
            vec![mapped("o", "refs/remotes/o/f-login")]
        );
    }

    /// Git demands that the name is at least as long as the prefix and the
    /// suffix of the pattern together, so the two never share a character.
    /// The text that `*` matches can be empty.
    #[test]
    fn the_prefix_and_the_suffix_of_a_pattern_do_not_overlap() {
        let records = [
            ("remote.o.fetch", "refs/heads/a*a:refs/remotes/o/overlap/*"),
            ("remote.o.fetch", "+refs/heads/*:refs/remotes/o/*"),
        ];

        assert_eq!(
            map(&records, "a"),
            vec![mapped("o", "refs/remotes/o/a")],
            "the one `a` cannot be both the prefix and the suffix"
        );
        assert_eq!(
            map(&records, "aba"),
            vec![mapped("o", "refs/remotes/o/overlap/b")]
        );
        assert_eq!(
            map(&records, "aa"),
            vec![mapped("o", "refs/remotes/o/overlap/")],
            "the `*` matches the empty text between the prefix and the suffix"
        );
    }

    /// A negative refspec without `*` keeps its remote from mapping exactly
    /// that branch. Each other branch still maps.
    #[test]
    fn a_negative_exact_refspec_keeps_the_remote_from_mapping_that_branch() {
        let records = [
            DEFAULT_ORIGIN,
            ("remote.origin.fetch", "^refs/heads/issue-33"),
            DEFAULT_UPSTREAM,
        ];

        assert_eq!(
            map(&records, "issue-33"),
            vec![mapped("upstream", "refs/remotes/upstream/issue-33")]
        );
        assert_eq!(
            map(&records, "issue-34"),
            vec![
                mapped("origin", "refs/remotes/origin/issue-34"),
                mapped("upstream", "refs/remotes/upstream/issue-34"),
            ]
        );
    }

    /// A negative pattern keeps its remote from mapping each branch that it
    /// matches, and its place among the refspecs of the remote does not
    /// matter.
    #[test]
    fn a_negative_pattern_keeps_the_remote_from_mapping_each_branch_it_matches() {
        let records = [
            ("remote.origin.fetch", "^refs/heads/issue-*"),
            DEFAULT_ORIGIN,
            DEFAULT_UPSTREAM,
        ];

        assert_eq!(
            map(&records, "issue-33"),
            vec![mapped("upstream", "refs/remotes/upstream/issue-33")]
        );
        assert_eq!(
            map(&records, "feature"),
            vec![
                mapped("origin", "refs/remotes/origin/feature"),
                mapped("upstream", "refs/remotes/upstream/feature"),
            ]
        );
    }

    /// A refspec without a `:` fetches and stores nothing, so it maps nothing,
    /// and the next refspec of the remote gives the answer.
    #[test]
    fn a_refspec_without_a_destination_maps_nothing_and_the_search_goes_on() {
        assert_eq!(
            map(
                &[
                    ("remote.o.fetch", "refs/heads/issue-33"),
                    ("remote.o.fetch", "+refs/heads/*:refs/remotes/o/*"),
                ],
                "issue-33"
            ),
            vec![mapped("o", "refs/remotes/o/issue-33")]
        );
    }

    /// An empty destination means "do not store". Git takes it as the match,
    /// so the search stops there, and the remote maps the branch to no ref.
    #[test]
    fn an_empty_destination_maps_nothing_and_stops_the_search() {
        let records = [
            ("remote.origin.fetch", "refs/heads/issue-33:"),
            DEFAULT_ORIGIN,
            DEFAULT_UPSTREAM,
        ];

        assert_eq!(
            map(&records, "issue-33"),
            vec![mapped("upstream", "refs/remotes/upstream/issue-33")]
        );
        assert_eq!(
            map(&records, "issue-34"),
            vec![
                mapped("origin", "refs/remotes/origin/issue-34"),
                mapped("upstream", "refs/remotes/upstream/issue-34"),
            ]
        );
    }

    /// Git refuses each of these refspecs, and each of its commands that
    /// reads the remotes stops with `fatal: invalid refspec`. So the lookup
    /// takes none of them, and the next refspec of the remote gives the
    /// answer:
    ///
    /// - a negative refspec with a `:`, which is not a negative refspec,
    /// - a `*` in the destination only,
    /// - a `*` in the source only,
    /// - more than one `*` on a side,
    /// - a `*` in a source without a destination.
    #[test]
    fn a_refspec_that_git_refuses_maps_nothing_and_the_search_goes_on() {
        for refused in [
            "^refs/heads/issue-33:refs/remotes/o/negative",
            "refs/heads/issue-33:refs/remotes/o/*",
            "refs/heads/issue-*:refs/remotes/o/one",
            "refs/heads/*-*:refs/remotes/o/*-*",
            "refs/heads/*",
        ] {
            assert_eq!(
                map(
                    &[
                        ("remote.o.fetch", refused),
                        ("remote.o.fetch", "+refs/heads/*:refs/remotes/o/*"),
                    ],
                    "issue-33"
                ),
                vec![mapped("o", "refs/remotes/o/issue-33")],
                "git refuses {refused:?}"
            );
        }
    }

    /// The first refspec of a remote that matches gives the ref, whether it
    /// is exact or a pattern.
    #[test]
    fn the_first_refspec_that_matches_gives_the_ref() {
        let exact = ("remote.o.fetch", "refs/heads/issue-33:refs/remotes/o/first");
        let pattern = ("remote.o.fetch", "+refs/heads/*:refs/remotes/o/*");

        assert_eq!(
            map(&[exact, pattern], "issue-33"),
            vec![mapped("o", "refs/remotes/o/first")]
        );
        assert_eq!(
            map(&[pattern, exact], "issue-33"),
            vec![mapped("o", "refs/remotes/o/issue-33")]
        );
    }

    /// A name with multi-byte characters maps intact, through a pattern with
    /// multi-byte text around its `*` too.
    #[test]
    fn a_name_with_multi_byte_characters_maps_intact() {
        for name in ["日本語", "café", "🎉"] {
            assert_eq!(
                map(&[DEFAULT_ORIGIN], name),
                vec![mapped("origin", &format!("refs/remotes/origin/{name}"))]
            );
        }

        assert_eq!(
            map(
                &[("remote.o.fetch", "refs/heads/café-*-é:refs/remotes/o/*")],
                "café-日本語-é"
            ),
            vec![mapped("o", "refs/remotes/o/日本語")]
        );

        assert_eq!(
            map(
                &[
                    ("remote.o.fetch", "refs/heads/é*é:refs/remotes/o/overlap/*"),
                    ("remote.o.fetch", "+refs/heads/*:refs/remotes/o/*"),
                ],
                "é"
            ),
            vec![mapped("o", "refs/remotes/o/é")],
            "the one `é` cannot be both the prefix and the suffix"
        );
    }

    /// The name of a remote is the text between `remote.` and `.fetch`, and
    /// it can hold dots. `remote.a.b.fetch` is the remote `a.b`.
    #[test]
    fn the_name_of_a_remote_keeps_its_dots() {
        assert_eq!(
            map(
                &[
                    ("remote.a.b.fetch", "+refs/heads/*:refs/remotes/a.b/*"),
                    (
                        "remote.x.fetch.fetch",
                        "+refs/heads/*:refs/remotes/x.fetch/*",
                    ),
                ],
                "issue-33"
            ),
            vec![
                mapped("a.b", "refs/remotes/a.b/issue-33"),
                mapped("x.fetch", "refs/remotes/x.fetch/issue-33"),
            ]
        );
    }

    /// The remotes keep the order of their first record in the listing, and
    /// a later record of a remote adds to the refspecs of that remote.
    #[test]
    fn the_remotes_keep_the_order_of_their_first_record() {
        assert_eq!(
            map(
                &[
                    ("remote.b.fetch", "refs/heads/other:refs/remotes/b/other"),
                    ("remote.a.fetch", "+refs/heads/*:refs/remotes/a/*"),
                    ("remote.b.fetch", "+refs/heads/*:refs/remotes/b/*"),
                ],
                "issue-33"
            ),
            vec![
                mapped("b", "refs/remotes/b/issue-33"),
                mapped("a", "refs/remotes/a/issue-33"),
            ]
        );
    }

    /// Git ignores a remote whose name starts with `/`
    /// (`warning: config remote shorthand cannot begin with '/'`).
    #[test]
    fn a_remote_name_that_starts_with_a_slash_names_no_remote() {
        assert_eq!(
            map(
                &[
                    ("remote./x.fetch", "+refs/heads/*:refs/remotes/slash/*"),
                    DEFAULT_ORIGIN,
                ],
                "issue-33"
            ),
            vec![mapped("origin", "refs/remotes/origin/issue-33")]
        );
    }

    /// A key without a value has no line break in its record. Git refuses it
    /// (`missing value for 'remote.v.fetch'`), so the lookup reads no
    /// refspec from it, and it reads each other record.
    #[test]
    fn a_record_without_a_value_gives_no_refspec() {
        let mut bytes = b"remote.v.fetch\0".to_vec();
        bytes.extend(listing(&[DEFAULT_ORIGIN]));

        assert_eq!(
            RemoteFetchRefspecs::from_config_listing(&bytes).map_branch("issue-33"),
            vec![mapped("origin", "refs/remotes/origin/issue-33")]
        );
    }
}
