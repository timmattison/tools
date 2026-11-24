//! Rules for the URLs of remote repositories.
//!
//! A host repository has an `origin` URL. The URL of a sidecar is the same URL with
//! only its last segment changed.

use std::path::{Path, PathBuf};

use anyhow::anyhow;
use url::Url;

use crate::name::SidecarName;

/// The suffix that a remote URL can carry.
const GIT_SUFFIX: &str = ".git";

/// The characters that end a path segment: the slash, and the colon of the scp form.
const SEGMENT_SEPARATORS: [char; 2] = ['/', ':'];

/// The prefix of the key of a repository at a local path.
const LOCAL_KEY: &str = "local:";

/// The scheme of a URL that names a local path.
const FILE_SCHEME: &str = "file";

/// The text between the host name and the sidecar name in the name of a sidecar repository.
pub const DATA_INFIX: &str = "-data-";

/// The last path segment of a remote URL, without `.git` and trailing slashes.
/// Returns `None` when the segment is empty.
#[must_use]
pub fn repository_name(url: &str) -> Option<String> {
    let (_, last) = split_last_segment(url);
    let name = last.strip_suffix(GIT_SUFFIX).unwrap_or(last);
    (!name.is_empty()).then(|| name.to_string())
}

/// The segment before the last one, which is the GitHub owner.
/// Returns `None` when the URL has no such segment.
#[must_use]
pub fn owner(url: &str) -> Option<String> {
    let (before, _) = split_last_segment(url);
    let head = before.strip_suffix(SEGMENT_SEPARATORS)?;
    let segment = head.rsplit(SEGMENT_SEPARATORS).next()?;
    (!segment.is_empty()).then(|| segment.to_string())
}

/// The name of the sidecar repository: `<host>-data-<name>`.
#[must_use]
pub fn sidecar_repository(host: &str, name: &SidecarName) -> String {
    format!("{host}{DATA_INFIX}{}", name.as_str())
}

/// The origin URL with only its last segment changed to `<host>-data-<name>`.
/// A `.git` suffix stays. A trailing slash goes.
///
/// # Errors
///
/// Returns an error when the URL has no last segment.
pub fn sidecar_url(origin: &str, name: &SidecarName) -> anyhow::Result<String> {
    let (before, last) = split_last_segment(origin);
    let suffix = if last.ends_with(GIT_SUFFIX) {
        GIT_SUFFIX
    } else {
        ""
    };
    let host = repository_name(origin)
        .ok_or_else(|| anyhow!("The origin URL {origin:?} has no repository name."))?;
    Ok(format!(
        "{before}{}{suffix}",
        sidecar_repository(&host, name)
    ))
}

/// Tells whether two URLs name one repository. The host (in lowercase) and the path must
/// match, or the local path must match. The user, the port, a `.git` suffix, and a
/// trailing slash do not count.
#[must_use]
pub fn same_repository(first: &str, second: &str) -> bool {
    repository_key(first) == repository_key(second)
}

/// Removes the trailing slashes, then splits the URL after its last separator.
/// The first part keeps the separator. The second part is the last segment.
/// A URL with no separator gives an empty first part.
fn split_last_segment(url: &str) -> (&str, &str) {
    let trimmed = url.trim_end_matches('/');
    match trimmed.rfind(SEGMENT_SEPARATORS) {
        // The separator is one ASCII byte, so the byte after it starts a character.
        Some(index) => trimmed.split_at(index + 1),
        None => ("", trimmed),
    }
}

/// Gives the scheme of a URL that starts with `<scheme>://`, or `None`.
fn scheme_of(text: &str) -> Option<&str> {
    let (scheme, _) = text.split_once("://")?;
    let mut characters = scheme.chars();
    let first_is_letter = characters.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest_is_valid = characters.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
    (first_is_letter && rest_is_valid).then_some(scheme)
}

/// Gives the key of the local path `path`: the canonical path when it exists, else
/// the absolute path, else the path as written.
fn local_key(path: &Path) -> String {
    let resolved: PathBuf = path
        .canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    format!("{LOCAL_KEY}{}", resolved.display())
}

/// Splits the scp form `[user@]host:path`. Returns `None` for any other text.
fn split_scp(text: &str) -> Option<(&str, &str)> {
    let rest = match text.split_once('@') {
        Some((user, rest)) if !user.is_empty() && !user.contains('/') => rest,
        _ => text,
    };
    let (host, path) = rest.split_once(':')?;
    (!host.is_empty() && !host.contains('/') && !path.is_empty()).then_some((host, path))
}

/// Gives the key of the repository that a URL names: the host in lowercase and the
/// path, or the resolved local path.
fn repository_key(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let bare = trimmed.strip_suffix(GIT_SUFFIX).unwrap_or(trimmed);

    if let Some(scheme) = scheme_of(bare) {
        let Ok(parsed) = Url::parse(bare) else {
            return format!("raw:{bare}");
        };
        if scheme.eq_ignore_ascii_case(FILE_SCHEME) {
            if let Ok(path) = parsed.to_file_path() {
                return local_key(&path);
            }
        }
        let host = parsed.host_str().unwrap_or_default().to_lowercase();
        return format!("{host}:{}", parsed.path().trim_start_matches('/'));
    }

    if let Some((host, path)) = split_scp(bare) {
        return format!("{}:{}", host.to_lowercase(), path.trim_start_matches('/'));
    }

    local_key(Path::new(bare))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCP: &str = "git@github.com:timmattison/megarepo.git";

    fn name(raw: &str) -> SidecarName {
        SidecarName::parse(raw).expect("test name is valid")
    }

    #[test]
    fn infix_is_dash_data_dash() {
        assert_eq!(DATA_INFIX, "-data-");
    }

    #[test]
    fn repository_name_of_scp_url() {
        assert_eq!(repository_name(SCP).as_deref(), Some("megarepo"));
    }

    #[test]
    fn repository_name_of_https_url_with_trailing_slash() {
        assert_eq!(
            repository_name("https://github.com/timmattison/megarepo/").as_deref(),
            Some("megarepo")
        );
    }

    #[test]
    fn repository_name_of_empty_url_is_none() {
        assert_eq!(repository_name(""), None);
        assert_eq!(repository_name("///"), None);
    }

    #[test]
    fn owner_of_scp_url() {
        assert_eq!(owner(SCP).as_deref(), Some("timmattison"));
    }

    #[test]
    fn owner_of_local_path() {
        assert_eq!(
            owner("/tmp/x/remotes/fixture-owner/megarepo.git").as_deref(),
            Some("fixture-owner")
        );
    }

    #[test]
    fn owner_of_multibyte_segment() {
        assert_eq!(owner("/tmp/日本/megarepo.git").as_deref(), Some("日本"));
    }

    #[test]
    fn owner_without_owner_is_none() {
        assert_eq!(owner("megarepo.git"), None);
    }

    #[test]
    fn sidecar_repository_joins_host_infix_and_name() {
        assert_eq!(
            sidecar_repository("megarepo", &name("testdata")),
            "megarepo-data-testdata"
        );
    }

    #[test]
    fn sidecar_url_of_scp_url_keeps_git_suffix() {
        assert_eq!(
            sidecar_url(SCP, &name("testdata")).unwrap(),
            "git@github.com:timmattison/megarepo-data-testdata.git"
        );
    }

    #[test]
    fn sidecar_url_of_https_url_without_suffix() {
        assert_eq!(
            sidecar_url("https://github.com/timmattison/megarepo", &name("testdata")).unwrap(),
            "https://github.com/timmattison/megarepo-data-testdata"
        );
    }

    #[test]
    fn sidecar_url_drops_trailing_slash() {
        assert_eq!(
            sidecar_url(
                "https://github.com/timmattison/megarepo/",
                &name("testdata")
            )
            .unwrap(),
            "https://github.com/timmattison/megarepo-data-testdata"
        );
    }

    #[test]
    fn sidecar_url_of_multibyte_path_works() {
        assert_eq!(
            sidecar_url("/tmp/日本/megarepo.git", &name("testdata")).unwrap(),
            "/tmp/日本/megarepo-data-testdata.git"
        );
    }

    #[test]
    fn sidecar_url_of_empty_url_is_an_error() {
        assert!(sidecar_url("", &name("testdata")).is_err());
    }

    #[test]
    fn same_repository_ignores_scheme_form_host_case_and_suffix() {
        assert!(same_repository(
            "git@github.com:timmattison/megarepo-data-testdata.git",
            "https://GitHub.com/timmattison/megarepo-data-testdata"
        ));
    }

    #[test]
    fn same_repository_ignores_user_and_port() {
        assert!(same_repository(
            "ssh://git@github.com:22/a/b.git",
            "https://github.com/a/b"
        ));
    }

    #[test]
    fn same_repository_is_false_for_a_different_owner() {
        assert!(!same_repository(
            "git@github.com:a/x.git",
            "git@github.com:b/x.git"
        ));
    }

    #[test]
    fn same_repository_is_false_for_a_different_host() {
        assert!(!same_repository(
            "https://github.com/a/x",
            "https://gitlab.com/a/x"
        ));
    }

    #[test]
    fn same_repository_for_local_paths_ignores_slash_and_suffix() {
        assert!(same_repository(
            "/nonexistent-sidecar/x/y.git",
            "/nonexistent-sidecar/x/y/"
        ));
    }

    #[test]
    fn same_repository_for_file_url_and_local_path() {
        assert!(same_repository(
            "file:///nonexistent-sidecar/y.git",
            "/nonexistent-sidecar/y"
        ));
    }

    #[test]
    fn same_repository_is_false_for_different_local_paths() {
        assert!(!same_repository(
            "/nonexistent-sidecar/x",
            "/nonexistent-sidecar/y"
        ));
    }
}
