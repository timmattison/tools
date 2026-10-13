//! Open pull requests, read from the output of `gh pr list`.
//!
//! `cwt` shows the numbers of the open pull requests after the branch of a
//! worktree. One `gh pr list --json` call for each repository gives all the
//! data. This module holds the pure part of that work: it reads the URL of a
//! remote, and it reads the JSON that `gh` prints. It runs no command and it
//! needs no network, so every rule in it has an offline test.

use std::fmt;

use serde::Deserialize;

/// The number of one pull request.
///
/// A newtype keeps a pull request number apart from other integers. It reads
/// from a bare JSON number and it displays with a leading `#`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(transparent)]
pub struct PullNumber(u64);

impl fmt::Display for PullNumber {
    /// Writes the number in the form that GitHub shows, for example `#547`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// The host that `gh` talks to when `GH_HOST` is not set.
const GITHUB_HOST: &str = "github.com";

/// A repository on GitHub, named by host, owner, and name.
///
/// `gh pr list` asks one repository for its pull requests. This type holds the
/// three parts that name that repository. The host is in lowercase, because
/// host names ignore ASCII case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRepo {
    host: String,
    owner: String,
    name: String,
}

impl GithubRepo {
    /// Reads the URL of a git remote.
    ///
    /// Returns `Some` only when the host is `github.com` or equals
    /// `extra_host`. The caller passes the value of the `GH_HOST` environment
    /// variable as `extra_host`. An empty `extra_host` matches nothing.
    ///
    /// The function accepts the scp-like form `[user@]HOST:OWNER/NAME` and the
    /// URL forms `ssh://`, `https://`, `http://`, and `git://`. A trailing `/`
    /// and a trailing `.git` are optional. It returns `None` for local paths,
    /// `file://` URLs, other hosts, and paths that are not exactly
    /// `OWNER/NAME`. An SSH alias such as `github-work` is another host, so it
    /// returns `None` too.
    pub fn from_remote_url(url: &str, extra_host: Option<&str>) -> Option<Self> {
        let _ = (url, extra_host);
        None
    }

    /// The owner of the repository, as it appears in the remote URL.
    pub fn owner(&self) -> &str {
        &self.owner
    }
}

impl fmt::Display for GithubRepo {
    /// Writes `host/owner/name`, the form that `gh pr list --repo` takes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_number_displays_with_a_hash() {
        assert_eq!(PullNumber(547).to_string(), "#547");
    }

    #[test]
    fn pull_number_reads_from_a_bare_json_number() {
        let number: PullNumber = serde_json::from_str("548").unwrap();
        assert_eq!(number, PullNumber(548));
    }

    /// Reads `url` with no extra host and gives the `host/owner/name` form.
    fn parsed(url: &str) -> Option<String> {
        GithubRepo::from_remote_url(url, None).map(|repo| repo.to_string())
    }

    #[test]
    fn scp_like_form_is_read_with_and_without_dot_git() {
        assert_eq!(
            parsed("git@github.com:owner/name.git").as_deref(),
            Some("github.com/owner/name")
        );
        assert_eq!(
            parsed("git@github.com:owner/name").as_deref(),
            Some("github.com/owner/name")
        );
        assert_eq!(
            parsed("github.com:owner/name").as_deref(),
            Some("github.com/owner/name")
        );
    }

    #[test]
    fn https_form_is_read_in_every_spelling() {
        for url in [
            "https://github.com/owner/name",
            "https://github.com/owner/name.git",
            "https://github.com/owner/name/",
            "https://github.com/owner/name.git/",
            "https://user@github.com/owner/name",
            "https://github.com:443/owner/name",
            "http://github.com/owner/name",
        ] {
            assert_eq!(
                parsed(url).as_deref(),
                Some("github.com/owner/name"),
                "{url}"
            );
        }
    }

    #[test]
    fn ssh_and_git_schemes_are_read() {
        for url in [
            "ssh://git@github.com/owner/name.git",
            "ssh://git@github.com:22/owner/name.git",
            "git://github.com/owner/name.git",
            "HTTPS://github.com/owner/name",
        ] {
            assert_eq!(
                parsed(url).as_deref(),
                Some("github.com/owner/name"),
                "{url}"
            );
        }
    }

    #[test]
    fn host_comparison_ignores_case_and_stores_lowercase() {
        assert_eq!(
            parsed("git@GitHub.com:owner/name.git").as_deref(),
            Some("github.com/owner/name")
        );
    }

    #[test]
    fn owner_is_the_owner_part() {
        let repo = GithubRepo::from_remote_url("git@github.com:Owner/name.git", None).unwrap();
        assert_eq!(repo.owner(), "Owner");
    }

    #[test]
    fn extra_host_is_matched_only_when_given_and_not_empty() {
        let url = "git@ghe.example.com:org/repo.git";
        let repo = GithubRepo::from_remote_url(url, Some("ghe.example.com")).unwrap();
        assert_eq!(repo.to_string(), "ghe.example.com/org/repo");
        assert!(GithubRepo::from_remote_url(url, Some("GHE.Example.com")).is_some());
        assert!(GithubRepo::from_remote_url(url, None).is_none());
        assert!(GithubRepo::from_remote_url(url, Some("")).is_none());
        assert!(GithubRepo::from_remote_url("git@:org/repo.git", Some("")).is_none());
        assert!(GithubRepo::from_remote_url(url, Some("other.example.com")).is_none());
    }

    #[test]
    fn urls_that_are_not_a_github_repository_are_refused() {
        for url in [
            "git@gitlab.com:owner/name.git",
            "https://gitlab.com/owner/name",
            "/srv/repo.git",
            "../repo",
            "file:///srv/x.git",
            "git@github-work:owner/name.git",
            "https://github.com/a/b/c",
            "https://github.com/owner",
            "https://github.com/owner/",
            "https://github.com//name",
            "https://github.com/owner/.git",
            "https://github.com/owner/na me",
            "https://github.com/owner/na\u{e9}me",
            "https://github.com/\u{65e5}\u{672c}/name",
            "",
        ] {
            assert_eq!(parsed(url), None, "{url}");
        }
    }
}
