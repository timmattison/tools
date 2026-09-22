//! Core port-derivation primitives shared by `portplz` and related tools.
//!
//! This crate provides the stable hashing primitive used to turn an arbitrary
//! string into a deterministic, guaranteed-unprivileged TCP port number.

use sha2::{Digest, Sha256};
use std::path::Path;

/// A TCP port guaranteed to be unprivileged (always `>= 1024`).
///
/// Construct one only via [`unprivileged_port_from_string`], which enforces the
/// unprivileged invariant. The inner value is private and there is intentionally
/// no `Display` implementation, so callers must go through [`DerivedPort::get`]
/// to obtain the raw `u16` — keeping the invariant impossible to bypass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DerivedPort(u16);

impl DerivedPort {
    /// Returns the underlying port number, which is always `>= 1024`.
    #[must_use]
    pub fn get(&self) -> u16 {
        self.0
    }
}

/// Derives a deterministic, unprivileged port from an arbitrary input string.
///
/// The same input always yields the same port, and the result is always
/// `>= 1024` (i.e. never a privileged port).
#[must_use]
pub fn unprivileged_port_from_string(input: &str) -> DerivedPort {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();

    // `result` is a fixed 32-byte SHA-256 digest, so indexing the first two
    // bytes directly is always in bounds (and avoids the `string_slice` lint).
    let mut port = u16::from_be_bytes([result[0], result[1]]);

    while port < 1024 {
        port += 1024;
        port %= 65535;
    }

    DerivedPort(port)
}

/// Environment variable that overrides the detected user.
///
/// When set to a non-negative integer it replaces the live user id in the port
/// derivation. This lets you reproduce another user's port, or pin a stable
/// port in containers/CI where the uid differs from your workstation.
///
/// An empty or whitespace-only value is treated as unset (live detection is
/// used). A non-empty value that is not a non-negative integer is a hard error
/// ([`UserSaltError::InvalidUidOverride`]) — it is no longer silently ignored,
/// which would otherwise hand back a surprising, silently-different port.
pub const PORTPLZ_UID_ENV: &str = "PORTPLZ_UID";

/// Error resolving the current user from the environment.
#[derive(Debug, thiserror::Error)]
pub enum UserSaltError {
    /// `PORTPLZ_UID` was set to a non-empty value that is not a non-negative integer.
    #[error("PORTPLZ_UID must be a non-negative integer, but was set to '{0}'")]
    InvalidUidOverride(String),
}

/// Parses the `PORTPLZ_UID` override value.
///
/// - `None`, or an empty/whitespace-only string → `Ok(None)` (no override; use
///   live detection)
/// - a non-negative integer (after trimming) → `Ok(Some(UserSalt::Uid(n)))`
/// - any other non-empty value → `Err(UserSaltError::InvalidUidOverride)`,
///   carrying the original untrimmed raw string
fn parse_uid_override(raw: Option<&str>) -> Result<Option<UserSalt>, UserSaltError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        // An empty or whitespace-only value clears the override (use live detection).
        return Ok(None);
    }
    match trimmed.parse::<u32>() {
        Ok(uid) => Ok(Some(UserSalt::Uid(uid))),
        // Carry the original untrimmed raw string so the error reflects exactly
        // what the user set.
        Err(_) => Err(UserSaltError::InvalidUidOverride(raw.to_string())),
    }
}

/// Identifies the current user so two people on the same machine derive
/// different ports for the same repo and branch.
///
/// On Unix the identity is the numeric POSIX user id; on platforms without one
/// (e.g. Windows) it falls back to the login name. Use [`UserSalt::current`] for
/// the live value, or construct a fixed [`UserSalt::Uid`]/[`UserSalt::Name`] in
/// tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserSalt {
    /// A numeric POSIX user id (the common case on Unix).
    Uid(u32),
    /// A login name, used where no numeric uid exists (e.g. Windows).
    Name(String),
}

impl UserSalt {
    /// Resolves the current user.
    ///
    /// Honors the [`PORTPLZ_UID_ENV`] override first; otherwise uses the live
    /// POSIX uid on Unix, or the login name on platforms without one.
    ///
    /// # Errors
    /// Returns [`UserSaltError::InvalidUidOverride`] when `PORTPLZ_UID` is set to
    /// a non-empty value that is not a non-negative integer (e.g. `abc` or `-1`).
    /// An empty or whitespace-only value is treated as unset and falls through to
    /// live detection without erroring.
    pub fn current() -> Result<Self, UserSaltError> {
        if let Some(user) = parse_uid_override(std::env::var(PORTPLZ_UID_ENV).ok().as_deref())? {
            return Ok(user);
        }

        #[cfg(unix)]
        {
            // SAFETY: `getuid` is a POSIX call that always succeeds, has no
            // preconditions, and can neither fail nor invoke undefined behavior.
            Ok(Self::Uid(unsafe { libc::getuid() }))
        }
        #[cfg(not(unix))]
        {
            Ok(Self::Name(
                std::env::var("USERNAME")
                    .or_else(|_| std::env::var("USER"))
                    .unwrap_or_else(|_| "unknown".to_string()),
            ))
        }
    }

    /// The component mixed into the port hash to distinguish users.
    ///
    /// A uid renders as its decimal digits (no separators), which contain no
    /// newline, so prefixing it to the location's hash input keeps the
    /// user/location boundary unambiguous. For the `Name` variant, newline
    /// characters (`\n` and `\r`) are stripped from the login name for the same
    /// reason: the derived-port hash input uses `\n` as the boundary between the
    /// user and location components, so a newline inside the name would make
    /// that boundary ambiguous and let two distinct (user, location) pairs
    /// collide onto the same port.
    fn hash_component(&self) -> String {
        match self {
            Self::Uid(uid) => uid.to_string(),
            Self::Name(name) => name.chars().filter(|c| *c != '\n' && *c != '\r').collect(),
        }
    }

    /// Human-readable label appended to `--verbose` output, e.g. `uid 501`
    /// or `user 'alice'`.
    fn label(&self) -> String {
        match self {
            Self::Uid(uid) => format!("uid {uid}"),
            Self::Name(name) => format!("user '{name}'"),
        }
    }
}

/// Describes how the port's hash input was determined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortSource {
    /// Git repo with a branch: hash input is `"repo_name\nbranch"`.
    /// `\n` separator: git branch names cannot contain newlines, so the hash
    /// input is unambiguous even when names contain `@` etc.
    GitRepo { repo_name: String, branch: String },
    /// Git repo with detached HEAD: hash input is just `repo_name`.
    DetachedHead { repo_name: String },
    /// No git repo (--no-git or not a repo): hash input is `dirname`.
    Directory { dirname: String },
}

impl PortSource {
    #[must_use]
    pub fn hash_input(&self) -> String {
        match self {
            Self::GitRepo { repo_name, branch } => format!("{repo_name}\n{branch}"),
            Self::DetachedHead { repo_name } => repo_name.clone(),
            Self::Directory { dirname } => dirname.clone(),
        }
    }

    /// Human-readable one-line description, e.g. `Port 1234 for repo 'x' on branch 'y'`.
    #[must_use]
    pub fn describe(&self, port: DerivedPort) -> String {
        let desc = match self {
            Self::GitRepo { repo_name, branch } => {
                format!("repo '{repo_name}' on branch '{branch}'")
            }
            Self::DetachedHead { repo_name } => format!("repo '{repo_name}' (detached HEAD)"),
            Self::Directory { dirname } => format!("directory '{dirname}' (no git repo)"),
        };
        format!("Port {} for {desc}", port.get())
    }
}

/// Returns the repo root directory basename, consistent across worktrees.
///
/// Uses `common_dir()` which points to the shared `.git` directory,
/// then takes the parent (repo root) and extracts its basename.
/// For worktrees, `common_dir()` always points back to the main repo's
/// `.git` directory, so this returns the same name regardless of which
/// worktree you're in.
fn get_repo_root_name(repo: &gix::Repository) -> Option<String> {
    let common = std::fs::canonicalize(repo.common_dir()).ok()?;
    common
        .parent()
        .and_then(|p| p.file_name())
        .map(|name| name.to_string_lossy().to_string())
}

fn get_git_branch(repo: &gix::Repository) -> Option<String> {
    match repo.head() {
        Ok(head) => head.referent_name().map(|n| n.shorten().to_string()),
        Err(_) => None,
    }
}

/// The result of deriving a port: the port, how it was derived, for whom, and
/// under which name.
#[derive(Debug, Clone)]
pub struct Derivation {
    pub port: DerivedPort,
    pub source: PortSource,
    pub user: UserSalt,
    /// The name that told this application apart from the others in the same
    /// location, exactly as the caller gave it. It is `None` when the caller
    /// gave none. This is the name a person typed, not the component that
    /// reached the hash, so a description echoes what that person wrote.
    pub name: Option<String>,
}

impl Derivation {
    /// One-line human-readable description including the user, e.g.
    /// `Port 51877 for repo 'foo' on branch 'main' (uid 501)`, or
    /// `Port 40122 for repo 'tools' on branch 'main' named 'api' (uid 501)`
    /// when the derivation carries a name.
    #[must_use]
    pub fn describe(&self) -> String {
        let named = match &self.name {
            Some(name) => format!(" named '{name}'"),
            None => String::new(),
        };
        format!(
            "{}{named} ({})",
            self.source.describe(self.port),
            self.user.label()
        )
    }
}

/// Errors that can occur while deriving a port.
#[derive(Debug, thiserror::Error)]
pub enum DeriveError {
    #[error("invalid path: no basename")]
    NoBasename,
}

/// Marks the start and the end of the name at the head of the hash input.
///
/// No other component can hold this byte. A path component cannot contain a
/// NUL, because the kernel forbids it. A git branch name cannot contain one. A
/// uid renders as decimal digits. A login name arrives from the environment,
/// which holds C strings. So an input that holds a NUL carries a name, an input
/// that holds none does not, and the two sets cannot meet — whatever the
/// repository, the branch, the directory, the user, and the name are. A tag
/// made of ordinary text gives no such proof: it only holds until somebody's
/// login name is that text.
const NAME_FRAME: char = '\0';

/// The component mixed into the port hash to name one application apart from
/// another in the same location.
///
/// [`NAME_FRAME`] marks where the name ends, so the name must not hold one: a
/// name that did could close its own frame early and read as a different
/// (name, user, location) triple. Strip it, for the reason
/// [`UserSalt::hash_component`] strips a newline. A newline needs no stripping
/// here, because the frame and not the separator is what ends the name, and
/// stripping one would make two different names share a port.
fn name_hash_component(name: &str) -> String {
    name.chars().filter(|c| *c != NAME_FRAME).collect()
}

/// Derives the port for `path`.
///
/// When `no_git` is true, or `path` is not inside a git repo, the directory
/// basename is used; otherwise the repo-root name plus the current branch
/// (detached HEAD falls back to just the repo-root name). `user` is mixed into
/// the hash so different users derive different ports for the same location.
///
/// `name` names one application apart from another in the same location, so a
/// repository that holds more than one application can give each of them its
/// own port. It is a third component beside the repository and the branch, and
/// it replaces neither.
///
/// # Errors
/// Returns [`DeriveError::NoBasename`] if `path` has no final path component.
pub fn derive(
    path: &Path,
    no_git: bool,
    user: &UserSalt,
    name: Option<&str>,
) -> Result<Derivation, DeriveError> {
    let basename = path
        .file_name()
        .ok_or(DeriveError::NoBasename)?
        .to_string_lossy()
        .into_owned();

    let source = if no_git {
        PortSource::Directory { dirname: basename }
    } else {
        match gix::discover(path) {
            Ok(repo) => {
                let repo_name = get_repo_root_name(&repo).unwrap_or(basename);
                match get_git_branch(&repo) {
                    Some(branch) => PortSource::GitRepo { repo_name, branch },
                    None => PortSource::DetachedHead { repo_name },
                }
            }
            Err(_) => PortSource::Directory { dirname: basename },
        }
    };

    let unnamed = format!("{}\n{}", user.hash_component(), source.hash_input());
    let hash_input = match name {
        // The framed name sits at the head of the input, in front of everything
        // the unnamed derivation hashes. The unnamed input is thus unchanged,
        // and no named input can read as an unnamed one.
        Some(name) => format!(
            "{NAME_FRAME}{}{NAME_FRAME}{unnamed}",
            name_hash_component(name)
        ),
        None => unnamed,
    };
    let port = unprivileged_port_from_string(&hash_input);
    Ok(Derivation {
        port,
        source,
        user: user.clone(),
        name: name.map(ToString::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The port `derive` gives for `/tmp` with `no_git` under uid 0, whose hash
    /// input is `"0\ntmp"`. The `portplz` CLI test pins the same value through
    /// the binary.
    const UNNAMED_TMP_PORT_UID0: u16 = 19_642;

    /// Other tools and running services already depend on the ports that
    /// `derive` gives today, so a derivation that carries no name must keep its
    /// port for ever. This test pins one such port at the library boundary.
    #[test]
    fn unnamed_derivation_keeps_its_port() {
        let derivation = derive(Path::new("/tmp"), true, &UserSalt::Uid(0), None).expect("derive");
        assert_eq!(
            derivation.port.get(),
            UNNAMED_TMP_PORT_UID0,
            "the port of a derivation that carries no name must never change"
        );
    }

    /// A repository can hold more than one application, and each one needs its
    /// own port. So two names in one place must not share a port.
    #[test]
    fn two_names_in_one_place_give_two_ports() {
        let path = Path::new("/example/myrepo");
        let api = derive(path, true, &UserSalt::Uid(501), Some("api")).expect("derive");
        let site = derive(path, true, &UserSalt::Uid(501), Some("site")).expect("derive");
        assert_ne!(
            api.port.get(),
            site.port.get(),
            "two names in one place must give two different ports"
        );
    }

    /// A derivation that carries a name must never land on the port of a
    /// derivation that carries none.
    ///
    /// Issue #519 names the exact pair: a directory `foo` with the name `main`
    /// and no git, beside the repository `foo` on the branch `main`. Append the
    /// name after the separator and both read `{user}\nfoo\nmain`, so the two
    /// share a port and each one silently takes the other's service.
    #[test]
    fn a_name_cannot_collide_with_a_derivation_that_has_none() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let repo = tmp.path().join("foo");
        std::fs::create_dir(&repo).expect("create the repository directory");
        init_repo(&repo, "main");

        let user = UserSalt::Uid(501);
        let unnamed_repo = derive(&repo, false, &user, None).expect("derive");
        let named_directory =
            derive(Path::new("/example/foo"), true, &user, Some("main")).expect("derive");

        assert_ne!(
            named_directory.port.get(),
            unnamed_repo.port.get(),
            "the directory 'foo' named 'main' must not take the port of the repository 'foo' on \
             the branch 'main'"
        );
    }

    /// A port is only useful if it comes back. The same name in the same place
    /// must give the same port on every call.
    #[test]
    fn one_name_in_one_place_always_gives_one_port() {
        let path = Path::new("/example/myrepo");
        let first = derive(path, true, &UserSalt::Uid(501), Some("api")).expect("derive");
        let second = derive(path, true, &UserSalt::Uid(501), Some("api")).expect("derive");
        assert_eq!(
            first.port.get(),
            second.port.get(),
            "one name in one place must give one port on every call"
        );
    }

    /// The name is a third component beside the repository and the branch, and
    /// it replaces neither. So the same name in two places keeps two ports.
    #[test]
    fn one_name_in_two_places_gives_two_ports() {
        let here = derive(
            Path::new("/example/project-a"),
            true,
            &UserSalt::Uid(501),
            Some("api"),
        )
        .expect("derive");
        let there = derive(
            Path::new("/example/project-b"),
            true,
            &UserSalt::Uid(501),
            Some("api"),
        )
        .expect("derive");
        assert_ne!(
            here.port.get(),
            there.port.get(),
            "one name in two places must give two different ports"
        );
    }

    /// The user salt continues to apply, so two people on one machine can run
    /// the same named application side by side.
    #[test]
    fn the_user_salt_still_applies_to_a_named_derivation() {
        let path = Path::new("/example/myrepo");
        let mine = derive(path, true, &UserSalt::Uid(501), Some("api")).expect("derive");
        let yours = derive(path, true, &UserSalt::Uid(502), Some("api")).expect("derive");
        assert_ne!(
            mine.port.get(),
            yours.port.get(),
            "two users must derive two ports for one named application"
        );
    }

    /// A newline inside the name must not reach into the user or the location.
    ///
    /// Both derivations below spell the same four pieces in the same order —
    /// `api`, `501`, `foo`, `bar` — and differ only in which component each
    /// piece belongs to. A framing that ended the name at a newline would hand
    /// them one port. [`NAME_FRAME`] ends it instead, and a name holds no frame
    /// byte, so the two stay apart.
    #[test]
    fn a_newline_in_the_name_cannot_forge_a_boundary() {
        let name_is_one_piece = derive(
            Path::new("/example/foo\nbar"),
            true,
            &UserSalt::Name("501".into()),
            Some("api"),
        )
        .expect("derive");
        let name_is_two_pieces = derive(
            Path::new("/example/bar"),
            true,
            &UserSalt::Name("foo".into()),
            Some("api\n501"),
        )
        .expect("derive");
        assert_ne!(
            name_is_one_piece.port.get(),
            name_is_two_pieces.port.get(),
            "a newline in the name must not let it read as the user and the location"
        );
    }

    /// The frame byte ends the name, so the name must not carry one — and a
    /// newline must survive, or two names that differ only by one would share a
    /// port.
    #[test]
    fn the_name_component_strips_the_frame_byte_and_keeps_a_newline() {
        let stripped = name_hash_component("api\u{0}501");
        assert!(
            !stripped.contains(NAME_FRAME),
            "a name must carry no frame byte, got: {stripped:?}"
        );
        assert_eq!(
            name_hash_component("api"),
            "api",
            "a name that carries no frame byte must pass through unchanged"
        );
        assert_eq!(
            name_hash_component("a\nb"),
            "a\nb",
            "a newline is ordinary text inside a name, and must survive"
        );
    }

    /// Without the name in the description, a user who runs two applications
    /// out of one repository cannot tell which one a port belongs to.
    #[test]
    fn describe_names_the_name() {
        let derivation = derive(
            Path::new("/example/myrepo"),
            true,
            &UserSalt::Uid(501),
            Some("api"),
        )
        .expect("derive");
        assert_eq!(
            derivation.describe(),
            format!(
                "Port {} for directory 'myrepo' (no git repo) named 'api' (uid 501)",
                derivation.port.get()
            )
        );
    }

    /// The description of a derivation that carries no name must not gain a
    /// word when the named one gains one.
    #[test]
    fn describe_says_nothing_about_a_name_there_is_not() {
        let derivation = derive(
            Path::new("/example/myrepo"),
            true,
            &UserSalt::Uid(501),
            None,
        )
        .expect("derive");
        assert_eq!(
            derivation.describe(),
            format!(
                "Port {} for directory 'myrepo' (no git repo) (uid 501)",
                derivation.port.get()
            )
        );
    }

    #[test]
    fn parse_uid_override_rejects_non_numeric() {
        assert!(
            parse_uid_override(Some("abc")).is_err(),
            "a non-numeric PORTPLZ_UID must be a hard error"
        );
    }

    #[test]
    fn parse_uid_override_rejects_negative() {
        assert!(
            parse_uid_override(Some("-1")).is_err(),
            "a negative PORTPLZ_UID must be a hard error"
        );
    }

    #[test]
    fn parse_uid_override_accepts_integer() {
        assert_eq!(
            parse_uid_override(Some("5")).expect("valid uid"),
            Some(UserSalt::Uid(5))
        );
    }

    #[test]
    fn parse_uid_override_trims_surrounding_whitespace() {
        assert_eq!(
            parse_uid_override(Some("  7 ")).expect("valid uid"),
            Some(UserSalt::Uid(7))
        );
    }

    #[test]
    fn parse_uid_override_treats_blank_as_unset() {
        assert_eq!(
            parse_uid_override(Some("")).expect("empty is unset"),
            None,
            "an empty PORTPLZ_UID must clear the override without erroring"
        );
        assert_eq!(
            parse_uid_override(Some("   ")).expect("whitespace is unset"),
            None,
            "a whitespace-only PORTPLZ_UID must clear the override without erroring"
        );
    }

    #[test]
    fn parse_uid_override_treats_missing_as_unset() {
        assert_eq!(
            parse_uid_override(None).expect("missing is unset"),
            None,
            "an unset PORTPLZ_UID must use live detection without erroring"
        );
    }

    #[test]
    fn test_port_generation() {
        let port = unprivileged_port_from_string("test");
        assert!(port.get() >= 1024);
        assert!(port.get() < 65535);
    }

    #[test]
    fn test_consistent_port() {
        assert_eq!(
            unprivileged_port_from_string("example").get(),
            unprivileged_port_from_string("example").get()
        );
    }

    #[test]
    fn test_different_users_get_different_ports() {
        let path = std::path::Path::new("/example/myrepo");
        let a = derive(path, true, &UserSalt::Uid(501), None).expect("derive");
        let b = derive(path, true, &UserSalt::Uid(502), None).expect("derive");
        assert_ne!(
            a.port.get(),
            b.port.get(),
            "different users must derive different ports for the same location"
        );
    }

    #[test]
    fn test_describe_includes_uid_label() {
        let path = std::path::Path::new("/example/myrepo");
        let d = derive(path, true, &UserSalt::Uid(501), None).expect("derive");
        assert!(
            d.describe().contains("(uid 501)"),
            "verbose description must include the uid, got: {}",
            d.describe()
        );
    }

    #[test]
    fn test_name_hash_component_strips_newlines() {
        // A name containing newlines must not leak them into the hash component,
        // or the `\n` boundary between the user and location components becomes
        // ambiguous and two distinct (user, location) pairs could collide.
        let component = UserSalt::Name("a\nb\rc".into()).hash_component();
        assert!(
            !component.contains('\n'),
            "Name hash component must not contain a newline, got: {component:?}"
        );
        assert!(
            !component.contains('\r'),
            "Name hash component must not contain a carriage return, got: {component:?}"
        );

        // A newline-free name must pass through unchanged (no over-stripping).
        assert_eq!(
            UserSalt::Name("alice".into()).hash_component(),
            "alice",
            "a name without newlines must be unchanged"
        );
    }

    #[test]
    fn test_describe_includes_name_label() {
        let path = std::path::Path::new("/example/myrepo");
        let d = derive(path, true, &UserSalt::Name("alice".into()), None).expect("derive");
        assert!(
            d.describe().contains("(user 'alice')"),
            "verbose description must include the login name, got: {}",
            d.describe()
        );
    }

    #[test]
    fn test_different_inputs() {
        assert_ne!(
            unprivileged_port_from_string("branch-a").get(),
            unprivileged_port_from_string("branch-b").get()
        );
    }

    #[test]
    fn test_different_repos_same_branch_different_ports() {
        let source_a = PortSource::GitRepo {
            repo_name: "project-a".into(),
            branch: "main".into(),
        };
        let source_b = PortSource::GitRepo {
            repo_name: "project-b".into(),
            branch: "main".into(),
        };
        assert_ne!(
            unprivileged_port_from_string(&source_a.hash_input()).get(),
            unprivileged_port_from_string(&source_b.hash_input()).get(),
        );
    }

    #[test]
    fn test_port_source_git_repo_hash_input() {
        let source = PortSource::GitRepo {
            repo_name: "myproject".into(),
            branch: "main".into(),
        };
        assert_eq!(source.hash_input(), "myproject\nmain");
    }

    #[test]
    fn test_port_source_detached_head_hash_input() {
        let source = PortSource::DetachedHead {
            repo_name: "myproject".into(),
        };
        assert_eq!(source.hash_input(), "myproject");
    }

    #[test]
    fn test_port_source_directory_hash_input() {
        let source = PortSource::Directory {
            dirname: "some-dir".into(),
        };
        assert_eq!(source.hash_input(), "some-dir");
    }

    #[test]
    fn test_port_source_describe_git_repo() {
        let source = PortSource::GitRepo {
            repo_name: "myproject".into(),
            branch: "main".into(),
        };
        let port = unprivileged_port_from_string(&source.hash_input());
        assert_eq!(
            source.describe(port),
            format!("Port {} for repo 'myproject' on branch 'main'", port.get())
        );
    }

    #[test]
    fn test_port_source_describe_detached() {
        let source = PortSource::DetachedHead {
            repo_name: "myproject".into(),
        };
        let port = unprivileged_port_from_string(&source.hash_input());
        assert_eq!(
            source.describe(port),
            format!("Port {} for repo 'myproject' (detached HEAD)", port.get())
        );
    }

    #[test]
    fn test_port_source_describe_directory() {
        let source = PortSource::Directory {
            dirname: "some-dir".into(),
        };
        let port = unprivileged_port_from_string(&source.hash_input());
        assert_eq!(
            source.describe(port),
            format!("Port {} for directory 'some-dir' (no git repo)", port.get())
        );
    }

    #[test]
    fn test_separator_prevents_cross_component_collision() {
        let source_1 = PortSource::GitRepo {
            repo_name: "a@b".into(),
            branch: "c".into(),
        };
        let source_2 = PortSource::GitRepo {
            repo_name: "a".into(),
            branch: "b@c".into(),
        };
        assert_ne!(source_1.hash_input(), source_2.hash_input());
    }

    #[test]
    fn test_get_repo_root_name_returns_valid_basename() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo = gix::discover(path).expect("Should find git repo");
        let name = get_repo_root_name(&repo);
        assert!(
            name.is_some(),
            "Should find repo root name for a valid repo"
        );
        let name = name.unwrap();
        assert!(!name.is_empty(), "Repo root name should not be empty");
        assert!(!name.contains('/'), "Should be a basename, not a path");
        assert!(!name.contains('\\'), "Should be a basename, not a path");
    }

    #[test]
    fn test_worktree_and_main_repo_share_root_name() {
        // Discover repo from the current path (may be a worktree)
        let worktree_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let worktree_repo = gix::discover(worktree_path).expect("Should find repo");
        let worktree_name = get_repo_root_name(&worktree_repo).expect("Should get repo root name");

        // Discover repo from the main repo root (parent of common_dir)
        let common = std::fs::canonicalize(worktree_repo.common_dir()).unwrap();
        let main_repo_root = common.parent().unwrap();
        let main_repo = gix::discover(main_repo_root).expect("Should find main repo");
        let main_name = get_repo_root_name(&main_repo).expect("Should get main repo root name");

        assert_eq!(
            worktree_name, main_name,
            "get_repo_root_name should return the same name from both worktree and main repo"
        );
    }

    fn run_git(dir: &std::path::Path, args: &[&str]) {
        // Shed the whole inherited `GIT_` family, then pin the two config
        // files. The sweep comes first so the pins win, and it is a prefix
        // rather than the three names this fixture used to list: a list
        // strips nothing new the day git adds a variable.
        let mut command = std::process::Command::new("git");
        gitscratch::shed_inherited_git_environment(&mut command);

        let status = command
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("invoke git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Makes `dir` a git repository whose HEAD is born on `branch`.
    ///
    /// The empty commit is what makes the branch report deterministically: an
    /// unborn HEAD has no referent name.
    fn init_repo(dir: &std::path::Path, branch: &str) {
        run_git(dir, &["init", "-b", branch]);
        run_git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@e",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
    }

    #[test]
    fn test_derive_on_git_init_repo_is_gitrepo_and_stable() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let dir = tmp.path();
        init_repo(dir, "testbranch");

        let d1 = derive(dir, false, &UserSalt::Uid(501), None).expect("derive should succeed");
        let d2 = derive(dir, false, &UserSalt::Uid(501), None).expect("derive should succeed");
        assert_eq!(
            d1.port.get(),
            d2.port.get(),
            "derived port must be stable across calls"
        );
        match &d1.source {
            PortSource::GitRepo { branch, .. } => assert_eq!(branch, "testbranch"),
            other => panic!("expected GitRepo source, got {other:?}"),
        }
    }
}
