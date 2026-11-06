//! Rules for the URLs of remote repositories.
//!
//! A host repository has an `origin` URL. The URL of a sidecar is the same URL with
//! only its last segment changed.

use crate::name::SidecarName;

/// The text between the host name and the sidecar name in the name of a sidecar repository.
pub const DATA_INFIX: &str = "-data-";

/// The last path segment of a remote URL, without `.git` and trailing slashes.
/// Returns `None` when the segment is empty.
#[must_use]
pub fn repository_name(url: &str) -> Option<String> {
    let _ = url;
    None
}

/// The segment before the last one, which is the GitHub owner.
/// Returns `None` when the URL has no such segment.
#[must_use]
pub fn owner(url: &str) -> Option<String> {
    let _ = url;
    None
}

/// The name of the sidecar repository: `<host>-data-<name>`.
#[must_use]
pub fn sidecar_repository(host: &str, name: &SidecarName) -> String {
    let _ = (host, name);
    String::new()
}

/// The origin URL with only its last segment changed to `<host>-data-<name>`.
/// A `.git` suffix stays. A trailing slash goes.
///
/// # Errors
///
/// Returns an error when the URL has no last segment.
pub fn sidecar_url(origin: &str, name: &SidecarName) -> anyhow::Result<String> {
    let _ = (origin, name);
    Ok(String::new())
}

/// Tells whether two URLs name one repository. The host (in lowercase) and the path must
/// match, or the local path must match. The user, the port, a `.git` suffix, and a
/// trailing slash do not count.
#[must_use]
pub fn same_repository(first: &str, second: &str) -> bool {
    let _ = (first, second);
    false
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
