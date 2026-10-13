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

/// The suffix that many remote URLs add to the name of a repository.
const GIT_SUFFIX: &str = ".git";

/// The URL schemes that name a remote on a network host.
const NETWORK_SCHEMES: [&str; 4] = ["ssh", "https", "http", "git"];

/// Splits the URL of a remote into its host and its path.
///
/// The URL form `scheme://[user@]host[:port]/path` and the scp-like form
/// `[user@]host:path` both work. The host keeps its original case and loses
/// the user and the port. Returns `None` for a scheme that is not a network
/// scheme, for a local path, and for a port that is not a number.
fn split_host_and_path(url: &str) -> Option<(&str, &str)> {
    if let Some((scheme, rest)) = url.split_once("://") {
        if !NETWORK_SCHEMES
            .iter()
            .any(|known| known.eq_ignore_ascii_case(scheme))
        {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let host_and_port = authority
            .rsplit_once('@')
            .map_or(authority, |(_, after)| after);
        return match host_and_port.split_once(':') {
            Some((host, port)) => (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
                .then_some((host, path)),
            None => Some((host_and_port, path)),
        };
    }
    let (before, path) = url.split_once(':')?;
    if before.contains('/') {
        return None;
    }
    let host = before.rsplit_once('@').map_or(before, |(_, after)| after);
    Some((host, path))
}

/// Tells if `part` is a valid owner or repository name.
///
/// The part is not empty and holds only ASCII letters, digits, `-`, `_`, and
/// `.`. A part of only `.git` is not a name, because the caller has already
/// removed one `.git` suffix and an empty name is left.
fn is_name_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

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
        let (host, path) = split_host_and_path(url)?;
        let host = host.to_ascii_lowercase();
        let wanted = host == GITHUB_HOST
            || extra_host
                .is_some_and(|extra| !extra.is_empty() && host == extra.to_ascii_lowercase());
        if !wanted {
            return None;
        }
        let path = path.strip_suffix('/').unwrap_or(path);
        let path = path.strip_suffix(GIT_SUFFIX).unwrap_or(path);
        let (owner, name) = path.split_once('/')?;
        if !is_name_part(owner) || !is_name_part(name) {
            return None;
        }
        Some(Self {
            host,
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    /// The owner of the repository, as it appears in the remote URL.
    pub fn owner(&self) -> &str {
        &self.owner
    }
}

impl fmt::Display for GithubRepo {
    /// Writes `host/owner/name`, the form that `gh pr list --repo` takes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.host, self.owner, self.name)
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
