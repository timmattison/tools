//! crap — "Claude, Resume Anywhere Please".
//!
//! Resume a Claude Code session from whatever directory it was originally
//! started in, no matter where you are now. Given a session id, `crap` looks
//! up that session under `~/.claude/projects`, recovers the directory it ran
//! in, changes into it, and re-launches Claude with `--resume <id>`.
//!
//! With `--here`, it instead brings the session to *you*: Claude resolves a
//! `--resume <id>` only against the project folder matching the current working
//! directory, so `crap --here` symlinks the session's transcript into that
//! folder and resumes it as a `--fork-session` (a fresh id), leaving the
//! original transcript untouched. Because the fork only reads that transcript,
//! `--here` works even while the original session is still live in another
//! process. The symlink is removed once the session ends. A second argument
//! (`crap --here <id> <new-id>`) pins the fork to a chosen UUID via
//! `claude --session-id` instead of a random one, provided it does not already
//! name an existing session. `--here` also accepts a cross-user source
//! (`crap --here <id> --user <name>`): the foreign transcript is *copied* into
//! your own tree instead of symlinked, so nothing is ever linked into another
//! user's home, and the copy is cleaned up the same way the symlink is.
//!
//! With `--status`, it resumes nothing: it classifies where the session left
//! off — `waiting-for-user`, `busy`, `awaiting-assistant`, or `empty`, inferred
//! from the last conversational turn in the transcript (or the live process's
//! own status when one is attached) — and prints that one scriptable token.
//! Given no id, `--status` instead lists every session recorded for the current
//! directory, each with its state and the times its transcript was started and
//! last written (read from the transcript's own timestamps, not file mtimes).
//!
//! A session that belongs to another account is found automatically, with no
//! flag at all: `crap <id>` searches your own tree first (the fast path,
//! unchanged) and, only on a miss, falls back to scanning every sibling home
//! that has run Claude, resuming the first readable match. The lookup is
//! self-first, so an id that exists in two accounts always resolves to your own
//! copy. A foreign hit cannot be resumed in place — the transcript belongs to
//! another user, so a `claude --resume` run by *you* could never find it.
//! Instead, `crap` copies it into your own tree and resumes it as a
//! `--fork-session` (a fresh id) at its original recorded directory: the foreign
//! transcript is only ever read, every write lands under your home, and the
//! transient copy is removed once Claude writes the forked transcript, the same
//! way `--here` cleans up its import (a symlink for a same-user source, a copy
//! for a cross-user one).
//!
//! `--user <name>` forces that cross-user path onto one specific account: it
//! searches only that sibling home's `~/.claude/projects` tree (resolved as
//! `<home>/../<name>`) and skips your own entirely, which is also how you
//! disambiguate an id on purpose. The resume itself is the same copy-and-fork.
//! A `--user` that names your own account is a same-user hit and resumes in
//! place as usual. A `--user` that names no account with a `.claude/projects`
//! tree — a typo, or an account that never ran Claude — fails up front with
//! `INVALID_USER`, listing the accounts you *can* resume from, rather than
//! searching a phantom tree and reporting a misleading "no session found". (An
//! account whose tree is merely unreadable to you is real, not invalid: it
//! resolves, and the owner-only guidance below takes over.)
//!
//! A cross-user default resume lands at the session's *original* recorded
//! directory, and refuses rather than silently substituting the current one when
//! that directory is unusable — exactly as a same-user resume already does. If
//! the directory no longer exists the miss is `DIRECTORY_MISSING`; if it exists
//! but cannot be entered from your account (a sealed parent, or a missing search
//! bit) it is `DIRECTORY_UNREADABLE`, kept distinct because "gone" and "sealed"
//! are different facts. Both point you at the escape hatch that works precisely
//! when the recorded directory does not: `crap --here <id>` ignores it and forks
//! in the current directory instead.
//!
//! Scanning another account's tree runs with exactly the privileges `crap` was
//! invoked with, so a project directory it is refused — typically `0o700` and
//! owned by that account, which makes it opaque rather than merely unreadable:
//! it cannot be listed, so whether the session is inside cannot be known — is
//! skipped and *recorded* rather than fatal. One unreadable directory therefore
//! never hides a session that is readable further along. If the id then turns up
//! nowhere readable and at least one directory was skipped, the miss says so:
//! it hedges to "no *readable* session", counts the skipped directories per
//! owning account, and prints copy-paste `sudo -u <user> …` commands that locate
//! the transcript, copy it into the current directory's project folder (the
//! transcript being unreadable, its own recorded directory cannot be known), and
//! resume it with `crap --here <id>`. That is where it stops: `crap` prints the
//! escalation for the user to run and never runs one itself, which a test
//! enforces by allowlisting every program the binary may spawn.
//!
//! Because a binary cannot change its parent shell's working directory (nor see
//! shell aliases such as `clauded`), the user-facing `crap` command is a shell
//! function installed via `crap --shell-setup`. This binary resolves the session
//! id — printing the original directory to resume from, or (for `--here`, and
//! for a cross-user hit) importing the transcript into the right project folder
//! and printing what the function should run and clean up.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::exit;

use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use serde::Serialize;
use shellsetup::ShellIntegration;

/// Exit codes for the different failure conditions.
mod exit_codes {
    /// No session file matched the given id.
    pub const SESSION_NOT_FOUND: i32 = 1;
    /// The session file had no recorded working directory.
    pub const NO_CWD_IN_SESSION: i32 = 2;
    /// The recorded working directory no longer exists.
    pub const DIRECTORY_MISSING: i32 = 3;
    /// The session id was not a valid filename component.
    pub const INVALID_SESSION_ID: i32 = 4;
    /// Shell setup failed.
    pub const SHELL_SETUP_ERROR: i32 = 5;
    /// The user's home directory could not be determined.
    pub const NO_HOME_DIR: i32 = 6;
    /// The session is already running in another process.
    pub const SESSION_ALREADY_RUNNING: i32 = 7;
    /// `--here`: the project folder or the symlink/copy could not be created.
    pub const HERE_LINK_ERROR: i32 = 8;
    /// `--here`: the current working directory could not be determined.
    pub const HERE_PWD_UNAVAILABLE: i32 = 9;
    /// `--here`: the requested new session id already names a transcript.
    pub const NEW_SESSION_ID_EXISTS: i32 = 10;
    /// The recorded working directory exists but cannot be entered from this
    /// account.
    pub const DIRECTORY_UNREADABLE: i32 = 11;
    /// `--user <name>` named a sibling that does not exist, or exists but has no
    /// `.claude/projects` tree — so there is nothing for `--user` to search.
    pub const INVALID_USER: i32 = 12;
}

/// Why a located session transcript could not be resolved to an existing
/// directory. (Id validity is checked separately, before a transcript is even
/// located, so it is not represented here.)
#[derive(Debug)]
enum ResolveError {
    /// The transcript could not be read.
    SessionNotFound,
    /// The session file exists but records no working directory.
    NoCwdInSession,
    /// The recorded working directory no longer exists on disk.
    DirectoryMissing(PathBuf),
    /// The recorded working directory is still there, but this account cannot
    /// enter it — either an ancestor is opaque to us, so even asking about the
    /// directory is refused, or the directory itself lacks the search (`x`) bit
    /// that `cd` needs.
    ///
    /// Deliberately distinct from [`ResolveError::DirectoryMissing`]: we were
    /// *refused*, not told the directory is gone. Reporting "no longer exists"
    /// for a directory that is sitting right there sends the user hunting for
    /// the wrong problem, and treating it as usable would hand the shell
    /// function a `cd` that fails only after `crap` has already exited 0.
    DirectoryUnreadable(PathBuf),
}

/// Returns `true` if `id` is a canonical UUID (`8-4-4-4-12` hex digits).
///
/// Claude session ids are always UUIDs and the id only ever names a `.jsonl`
/// file under `~/.claude/projects`. Requiring this exact shape rejects typo'd
/// ids up front and, as a side effect, guarantees no path separator, traversal
/// sequence, or shell metacharacter can ride through to the filesystem lookup
/// or the shell function. Hex is matched case-insensitively.
fn is_valid_session_id(id: &str) -> bool {
    /// Hyphen positions in a canonical UUID, and its total length.
    const HYPHEN_POSITIONS: [usize; 4] = [8, 13, 18, 23];
    const UUID_LEN: usize = 36;

    if id.len() != UUID_LEN {
        return false;
    }
    id.bytes().enumerate().all(|(i, b)| {
        if HYPHEN_POSITIONS.contains(&i) {
            b == b'-'
        } else {
            b.is_ascii_hexdigit()
        }
    })
}

/// Extracts the first non-empty `cwd` value from Claude session JSONL contents.
///
/// Each line is an independent JSON object; the early bookkeeping lines often
/// carry `"cwd": null`, so the first line with a non-empty string `cwd` wins.
/// Non-JSON lines are skipped. Returns `None` if no usable `cwd` is present.
fn extract_cwd(contents: &str) -> Option<String> {
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(serde_json::Value::as_str) {
            if !cwd.is_empty() {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

/// Locates `<session_id>.jsonl` inside any immediate subdirectory of
/// `projects_dir`, returning its full path if found.
fn find_session_file(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let file_name = format!("{session_id}.jsonl");
    for entry in std::fs::read_dir(projects_dir).ok()?.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            let candidate = entry.path().join(&file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// One user's `~/.claude/projects` directory, tagged with who owns it.
///
/// Cross-user discovery searches an ordered list of these roots; the `is_self`
/// flag lets the resume logic pick "resume in place" (the current user's own
/// session) versus "copy into my tree and fork" (another user's session).
#[derive(Debug, Clone, PartialEq, Eq)]
struct UserProjects {
    /// The account name (the home directory's file name).
    user: String,
    /// That user's `.../.claude/projects` directory.
    projects_dir: PathBuf,
    /// Whether this is the invoking user's own tree.
    is_self: bool,
}

/// The current user's own `~/.claude/projects` root (`is_self = true`).
///
/// `home` is passed explicitly rather than read from the environment here, so
/// the mapping stays tempdir-testable; the one place that reads
/// `dirs::home_dir()` is `main`.
fn self_projects(home: &Path) -> UserProjects {
    let user = home
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    UserProjects {
        user,
        projects_dir: home.join(".claude").join("projects"),
        is_self: true,
    }
}

/// A sibling user's `~/.claude/projects` root, resolved as `users_parent/name`.
///
/// `is_self` is set when `name` equals `self_name` (the invoking user), so a
/// `--user` that names the current account is treated as a same-user hit. Like
/// [`self_projects`], every input is explicit so the mapping is tempdir-testable.
fn user_projects(users_parent: &Path, name: &str, self_name: &str) -> UserProjects {
    UserProjects {
        user: name.to_string(),
        projects_dir: users_parent.join(name).join(".claude").join("projects"),
        is_self: name == self_name,
    }
}

/// The canonical form of `path`, falling back to `path` verbatim when it cannot
/// be resolved.
///
/// [`std::fs::canonicalize`] resolves symlinks and `.`/`..` components, but only
/// for a path that actually exists — and the roots this compares include the
/// current user's `~/.claude/projects`, which is frequently absent (they have
/// never run Claude). A failure therefore has to mean "no better answer than the
/// path I was given" rather than "drop this root": two non-existent paths still
/// compare equal when they are literally the same path, which is exactly the
/// self-versus-self case that must dedupe.
fn canonical_or_verbatim(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Every `~/.claude/projects` root worth searching for a no-flag `crap <id>`,
/// ordered self-first for a stable, deterministic scan, with each physical tree
/// appearing exactly once.
///
/// The current user's own entry is always included and comes first, so a
/// self-first search short-circuits on a session the current user already owns
/// before ever reaching into another home — today's fast path, preserved even
/// when the current user has never run Claude (their entry is still root zero).
/// Sibling entries are included only when they actually have a `.claude/projects`
/// directory (an account that never ran Claude is not a search root), and are
/// ordered after self by account name so the result never depends on the order
/// the filesystem happens to list the parent directory in.
///
/// Roots are then deduped on their *canonical* `projects_dir` (see
/// [`canonical_or_verbatim`]) rather than on account name, because one home can
/// answer to several names: a symlinked alias (`/Users/me-alias -> /Users/me`),
/// or a `HOME` whose case differs from the on-disk directory on a
/// case-insensitive filesystem. Name comparison misses both, so the same tree
/// would be searched twice and the second copy — self included — would be
/// mislabelled as another user's, sending a hit down the foreign-user fork path
/// for a session the current user already owns. Because self is inserted first
/// and siblings are deduped against self *and* against each other, the surviving
/// entry for any tree is self when self is one of its names, and otherwise the
/// alphabetically first sibling name.
///
/// Every input is explicit (no `home_dir()` read here) so the enumeration is
/// tempdir-testable; the sole env-coupled caller in `main` derives `users_parent`
/// from `home.parent()` and `self_name` from `home.file_name()`.
fn enumerate_user_projects(users_parent: &Path, self_name: &str) -> Vec<UserProjects> {
    let mut others: Vec<UserProjects> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(users_parent) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let root = user_projects(users_parent, name, self_name);
            // Only accounts that have actually run Claude are search roots;
            // `is_dir` follows symlinks, so a symlinked home still counts.
            if root.projects_dir.is_dir() {
                others.push(root);
            }
        }
    }
    // Sort the siblings by account name so the order is independent of however
    // the filesystem enumerated the parent directory.
    others.sort_by(|a, b| a.user.cmp(&b.user));

    let mut roots = Vec::with_capacity(others.len() + 1);
    // Self goes in first and unconditionally, claiming its canonical tree, so it
    // stays root zero and is the only entry that can ever carry `is_self`.
    let me = user_projects(users_parent, self_name, self_name);
    let mut seen: HashSet<PathBuf> = HashSet::new();
    seen.insert(canonical_or_verbatim(&me.projects_dir));
    roots.push(me);
    for root in others {
        if seen.insert(canonical_or_verbatim(&root.projects_dir)) {
            roots.push(root);
        }
    }
    roots
}

/// The roots to search for `--user <name>`, or the error to print when the named
/// account is not a resumable target.
///
/// `--user` narrows the search to exactly one tree, with none of the self-first
/// fallback that rescues a no-flag miss — so a `<name>` that points at nothing
/// has nowhere else to be found and can only ever surface as a bare "session not
/// found". That is the wrong diagnosis: it sends the user to re-check the id when
/// the account they typed is the thing that is wrong. Making the account check
/// its own outcome, distinct from a session miss, is what lets `main` say so up
/// front.
enum UserRoots {
    /// `<name>` resolved to a real projects tree (possibly owner-only); search it.
    Resolved(Vec<UserProjects>),
    /// `<name>` names no account with a `.claude/projects` tree. Carries the
    /// accounts that DO have one, for the message.
    Invalid {
        /// The `--user` value that did not resolve.
        name: String,
        /// Accounts on this machine that have a `.claude/projects` tree, in
        /// [`enumerate_user_projects`] order (self first, then siblings by name).
        available: Vec<String>,
    },
}

/// Validates a `--user <name>` target against the sibling homes, returning the
/// single root to search or the accounts that *are* resumable when it names none.
///
/// A target counts as resumable exactly when `<name>/.claude/projects` is a
/// directory — the same test [`enumerate_user_projects`] uses to decide a sibling
/// is a search root, so `--user <self>` and `--user <sibling>` agree with the
/// no-flag enumeration and a same-user `--user` stays a valid in-place hit.
///
/// The classification is deliberately four-way, mirroring
/// [`session_dir_from_transcript`], because "the tree is not there" and "the tree
/// is there but sealed to me" are different realities that a plain `is_dir()`
/// (which reports `false` for a path it may not even `stat`) collapses into one
/// wrong answer:
///
/// - `Ok` + directory → a real, readable tree: resolve it.
/// - `PermissionDenied` → the tree, or an ancestor of it, is opaque to us. Only a
///   genuine account with real data can refuse us like that, so this is a *valid*
///   target: resolve it and let the normal search re-hit the refusal, record the
///   owner-only skip, and print the copy-it-first guidance. Calling it "invalid"
///   here would be a lie — the account plainly exists — and would swap the
///   actionable owner-only remedy for a dead end.
/// - `Ok` + not a directory (a stray file at that name), or any other error
///   (`NotFound` and the rest) → there is no tree to search and none we were
///   merely refused: the account is not a resumable target.
fn resolve_user_roots(users_parent: &Path, name: &str, self_name: &str) -> UserRoots {
    let root = user_projects(users_parent, name, self_name);
    match std::fs::metadata(&root.projects_dir) {
        // A real, readable projects tree: the ordinary cross-user (or same-user)
        // target.
        Ok(meta) if meta.is_dir() => UserRoots::Resolved(vec![root]),
        // Refused before we could learn anything — an ancestor is owner-only.
        // Being locked out is proof the account is real, not proof it is absent,
        // so treat it as valid and defer to the search's owner-only handling.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            UserRoots::Resolved(vec![root])
        }
        // Absent, or something that is not a directory sitting at the name: no
        // tree to search and none we were refused. Report the accounts that *are*
        // resumable, best-effort, from the same sibling scan the no-flag path uses.
        Ok(_) | Err(_) => UserRoots::Invalid {
            name: name.to_string(),
            available: enumerate_user_projects(users_parent, self_name)
                .into_iter()
                .map(|root| root.user)
                .collect(),
        },
    }
}

/// The message body for a `--user <name>` that names no resumable account,
/// carried as plain text with the caller colorizing the `Error:` prefix — like
/// every other formatter in this file — so the mapping stays unit-testable
/// without spawning a subprocess.
///
/// The headline names the bad `<name>`; the detail then either lists the accounts
/// that *do* have a projects tree (one per line under the same hanging indent the
/// other multi-account messages use) or, when none do, says so plainly rather
/// than printing an empty list under a heading that promises entries. No `sudo`
/// remedy appears here: this is "you named the wrong account", not "the account's
/// data is sealed" — that guidance belongs to the owner-only path, which a
/// `PermissionDenied` target reaches instead of this one.
fn format_invalid_user(name: &str, available: &[String]) -> String {
    /// The hanging indent that aligns a detail line under the `Error:` prefix,
    /// matching every other multi-line message in this binary.
    const INDENT: &str = "       ";
    /// Account names are indented one step further so they read as a list under
    /// the heading rather than as prose.
    const ITEM_INDENT: &str = "         ";

    let mut out =
        format!("--user '{name}' does not name an account with a Claude projects tree.\n");
    if available.is_empty() {
        // Nothing to point them at: neither our own tree nor any sibling home has
        // ever run Claude, so there is no resumable account to suggest instead.
        out.push_str(&format!(
            "{INDENT}no account on this machine has a Claude projects tree to resume from.\n"
        ));
    } else {
        out.push_str(&format!(
            "{INDENT}accounts you can resume from with --user:\n"
        ));
        for account in available {
            out.push_str(&format!("{ITEM_INDENT}{account}\n"));
        }
    }
    out
}

/// A project directory the scan could not enter, tagged with the account that
/// owns the tree it was found in.
///
/// A cross-user scan reaches into other people's homes, where a `0o700` project
/// directory is completely opaque to us: we cannot list it, and we cannot tell
/// whether the session we are hunting for is sitting inside it. That is a
/// materially different answer from "the id is not on this machine", and the
/// difference is only actionable if the scan remembers *which* directories it
/// could not see and *whose* they were — the user name is what turns a dead end
/// into a remedy the caller can name ("resume it as that account"). Carrying the
/// pair together means the miss itself contains everything guidance needs, so no
/// caller has to re-walk the tree to work out what it was denied.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SkippedDir {
    /// The account whose `~/.claude/projects` tree the directory belongs to.
    user: String,
    /// The directory itself, unreadable to the invoking user.
    dir: PathBuf,
}

/// The outcome of searching an ordered list of roots for a session id.
#[derive(Debug)]
enum FoundSession {
    /// The transcript, and which root (hence user / `is_self`) it came from.
    Found {
        /// The `<id>.jsonl` transcript that matched.
        path: PathBuf,
        /// The root it was found under, carrying the owning user and `is_self`.
        root: UserProjects,
    },
    /// The id was not found in any *readable* location under the searched roots.
    ///
    /// This is deliberately not a bare unit variant: a miss on a cross-user scan
    /// is only half an answer until you also know what the scan was not allowed
    /// to look at, so the variant carries the owner-only directories it stepped
    /// over. An empty `skipped` therefore means "the id genuinely is not here",
    /// while a non-empty one means "not in anything I could read" — and the
    /// caller can say so instead of asserting a certainty it does not have.
    NotFound {
        /// The owner-only directories skipped during the scan, in scan order.
        skipped: Vec<SkippedDir>,
    },
}

/// Searches an ordered list of roots for a session id, first match winning, and
/// records every directory it was refused.
///
/// Roots are searched in order and the search short-circuits on the first match,
/// so a self-first ordering makes a session the current user already owns always
/// win, and a hit is tagged with the root it came from (hence its owning user
/// and whether it is the current user's own tree).
///
/// The per-root inner loop is spelled out here rather than delegated to
/// [`find_session_file`] because that function answers a strictly smaller
/// question: it can only say "no `<id>.jsonl` here", collapsing "I looked and it
/// is absent" together with "I was not allowed to look". Across users those are
/// different answers. A `0o700` project directory owned by another account is
/// completely opaque without `sudo` — we cannot list it, and we cannot say
/// whether the session is inside — and `crap` must never run `sudo` itself: it
/// searches other homes with exactly the privileges it was invoked with, and
/// escalating on the user's behalf is not its call to make. Given that, the only
/// useful thing it can do with a refusal is remember it, so a miss can tell the
/// user precisely what it could not see and let them decide. Hence a
/// `PermissionDenied` probe becomes a [`SkippedDir`] tagged with the root's
/// user; every other IO error stays ignored, since a vanished or malformed entry
/// says nothing the user could act on.
///
/// A root whose `projects_dir` cannot even be listed is the same failure one
/// level up, and is recorded as a single skip naming the root itself.
fn find_session_across(roots: &[UserProjects], id: &str) -> FoundSession {
    let file_name = format!("{id}.jsonl");
    let mut skipped = Vec::new();
    for root in roots {
        let entries = match std::fs::read_dir(&root.projects_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                skipped.push(SkippedDir {
                    user: root.user.clone(),
                    dir: root.projects_dir.clone(),
                });
                continue;
            }
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let dir = entry.path();
            let candidate = dir.join(&file_name);
            match std::fs::metadata(&candidate) {
                Ok(meta) if meta.is_file() => {
                    return FoundSession::Found {
                        path: candidate,
                        root: root.clone(),
                    };
                }
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                    skipped.push(SkippedDir {
                        user: root.user.clone(),
                        dir,
                    });
                }
                // A non-file `<id>.jsonl`, or any other IO error, is a plain
                // miss: nothing here the user could be told to act on.
                Ok(_) | Err(_) => {}
            }
        }
    }
    FoundSession::NotFound { skipped }
}

/// The "where I looked" detail lines for a not-found session, indented to hang
/// under the `Error:` line that precedes them.
///
/// The first root is named in full — on the no-flag path that is always the
/// current user's own tree, and on the `--user <name>` path it is the only root
/// there is — because it is the one location the user can act on. Any remaining
/// roots collapse into a single count, deliberately *without* their account
/// names: the automatic cross-user fallback searches every sibling home that has
/// ever run Claude, so naming them would turn a mistyped id on a shared machine
/// into a roster of who else has an account there. The block is therefore never
/// more than two lines, however many accounts were searched.
///
/// Returns an empty string for an empty root list. No caller can produce one
/// (the current user is always root zero), but keeping the function total means
/// the not-found path can never print a dangling `Error:` with a stray indent.
fn format_searched_roots(roots: &[UserProjects]) -> String {
    /// The hanging indent that aligns a detail line under the `Error:` prefix.
    const INDENT: &str = "       ";

    let Some(first) = roots.first() else {
        return String::new();
    };
    let mut lines = format!("{INDENT}looked under {}\n", first.projects_dir.display());
    let others = roots.len() - 1;
    if others > 0 {
        let account = if others == 1 { "account" } else { "accounts" };
        lines.push_str(&format!(
            "{INDENT}…and {others} other {account} on this machine\n"
        ));
    }
    lines
}

/// The actionable guidance for a miss whose scan was refused entry to owner-only
/// project directories, indented to hang under the `Error:` line.
///
/// Returns an empty string when nothing was skipped, so the not-found path can
/// print it unconditionally: a genuine "that id is not on this machine" gains
/// nothing, and a caller never has to branch on the shape of the miss.
///
/// The guidance stops at *telling the user what to run*. `crap` searches other
/// homes with exactly the privileges it was invoked with, and reading a `0o700`
/// directory owned by someone else needs more than that — but escalating on the
/// user's behalf is not a tool's call to make, so `crap` never runs `sudo`
/// itself. Printing the exact commands is the most it can do and still leave the
/// decision (and the audit trail) with the person at the keyboard.
///
/// Every account with skipped directories gets a count line, in scan order, so
/// nobody has to guess how much of the machine was opaque. The remedy that
/// follows is keyed on the *first* such account — one worked example beats four
/// near-identical ones — and names that account explicitly whenever there is more
/// than one, so the reader can tell which of the counts above the commands are
/// for.
///
/// The remedy anchors on `pwd` rather than on the session's own recorded
/// directory for the reason the guidance exists at all: the transcript is
/// unreadable, so the directory recorded inside it cannot be known. The current
/// directory is the one location that is both knowable and almost certainly what
/// the user wants — they typed the id while standing where they intend to work —
/// so the copy lands in that directory's project folder and the trailing
/// `crap --here <id>` picks it straight back up.
fn format_owner_only_guidance(
    session_id: &str,
    roots: &[UserProjects],
    skipped: &[SkippedDir],
    pwd: &Path,
) -> String {
    /// The hanging indent that aligns a detail line under the `Error:` prefix.
    const INDENT: &str = "       ";
    /// Commands are indented one step further so they read as a copy-paste block
    /// rather than as prose.
    const COMMAND_INDENT: &str = "         ";

    let Some(first) = skipped.first() else {
        return String::new();
    };
    let owner = first.user.as_str();

    // Count per account, keeping first-seen (scan) order: the roots were searched
    // in a deliberate order (self first, then siblings by name), and reporting in
    // that same order keeps the message stable run to run.
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for dir in skipped {
        match counts.iter_mut().find(|(user, _)| *user == dir.user) {
            Some((_, count)) => *count += 1,
            None => counts.push((dir.user.as_str(), 1)),
        }
    }

    let mut out = String::new();
    for &(user, count) in &counts {
        let (noun, verb) = if count == 1 {
            ("project dir", "is owner-only and was skipped")
        } else {
            ("project dirs", "are owner-only and were skipped")
        };
        out.push_str(&format!(
            "{INDENT}{count} {noun} under user '{user}' {verb}.\n"
        ));
    }

    out.push_str(&format!(
        "{INDENT}if the session is in one of those, crap cannot read it — and crap never\n"
    ));
    if counts.len() > 1 {
        out.push_str(&format!(
            "{INDENT}runs sudo itself. copy it into your own tree first — for user '{owner}',\n\
             {INDENT}for example:\n"
        ));
    } else {
        out.push_str(&format!(
            "{INDENT}runs sudo itself. copy it into your own tree first, for example:\n"
        ));
    }

    // Search the owning account's whole tree, since which project folder holds
    // the session is exactly what could not be seen. The root is looked up by
    // account name; a skipped directory always came from a root, so the lookup
    // cannot really miss — but keeping it total (falling back to the skipped
    // directory itself, a real path inside that same tree) means a future caller
    // that assembles the two lists separately degrades to a narrower `find`
    // rather than panicking on a message that only exists to be helpful.
    let search_root = roots
        .iter()
        .find(|root| root.user == owner)
        .map_or(first.dir.as_path(), |root| root.projects_dir.as_path());
    // The project folder for the current directory. An unavailable `pwd` encodes
    // to the empty string, which leaves a still-runnable `mkdir -p` the user can
    // point wherever they like.
    let folder = encode_project_dir(pwd);
    out.push_str(&format!(
        "{COMMAND_INDENT}SRC=$(sudo -u {owner} find {} -name '{session_id}.jsonl')\n",
        search_root.display()
    ));
    out.push_str(&format!(
        "{COMMAND_INDENT}mkdir -p ~/.claude/projects/{folder}\n"
    ));
    out.push_str(&format!(
        "{COMMAND_INDENT}sudo -u {owner} cat \"$SRC\" > ~/.claude/projects/{folder}/{session_id}.jsonl\n"
    ));
    out.push_str(&format!("{COMMAND_INDENT}crap --here {session_id}\n"));
    out
}

/// The conversational state of a session, inferred from its transcript.
///
/// Claude Code never writes an explicit "turn finished, waiting for input"
/// marker, so the state is derived from the shape of the last *conversational*
/// turn (subagent/`isSidechain` and injected `isMeta` entries, and trailing
/// bookkeeping lines, are not turns and are ignored).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    /// The transcript records no conversational turns yet.
    Empty,
    /// Claude finished its turn and is waiting for the user: the last turn is
    /// an assistant message that ended with prose, with no pending tool call.
    WaitingForUser,
    /// Work is in flight — the assistant has an unanswered tool call, or a tool
    /// result was just delivered and the assistant has yet to respond.
    Busy,
    /// The user sent the last message and Claude has not replied yet (an active
    /// turn, or a session abandoned before the reply).
    AwaitingAssistant,
}

impl SessionState {
    /// The stable lowercase token printed by `crap --status`, suitable for
    /// scripting.
    fn as_token(self) -> &'static str {
        match self {
            SessionState::Empty => "empty",
            SessionState::WaitingForUser => "waiting-for-user",
            SessionState::Busy => "busy",
            SessionState::AwaitingAssistant => "awaiting-assistant",
        }
    }
}

/// Reports whether an assistant turn has ended (it is now the user's turn)
/// rather than leaving a tool call pending.
///
/// `stop_reason` is authoritative when present and is replicated onto every
/// JSONL line of a message, so the message's last line carries it even when
/// that line holds only a `thinking` or `text` block. When the field is absent
/// or null — which happens when a turn was interrupted mid-stream — the type of
/// the last content block is used as a fallback: a trailing `tool_use` block
/// means a tool call is still pending.
fn assistant_turn_ended(message: &serde_json::Value) -> bool {
    match message
        .get("stop_reason")
        .and_then(serde_json::Value::as_str)
    {
        Some("tool_use") => false,
        Some("end_turn" | "stop_sequence") => true,
        _ => {
            let last_block = message
                .get("content")
                .and_then(serde_json::Value::as_array)
                .and_then(|blocks| blocks.last())
                .and_then(|block| block.get("type"))
                .and_then(serde_json::Value::as_str);
            last_block != Some("tool_use")
        }
    }
}

/// Reports whether a `user` turn carries a tool result (the model is expected
/// to respond next) rather than a genuine user prompt.
fn user_turn_is_tool_result(message: &serde_json::Value) -> bool {
    message
        .get("content")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
            })
        })
}

/// Infers the [`SessionState`] from the raw contents of a session transcript.
///
/// Each line is an independent JSON object. Subagent turns (`isSidechain`) and
/// injected entries (`isMeta`) belong to other threads, and bookkeeping lines
/// (`last-prompt`, `ai-title`, `file-history-snapshot`, …) are not turns at
/// all; all are skipped. The state is decided by the last surviving
/// conversational turn — there is no explicit "waiting for input" marker in the
/// transcript to read directly.
fn classify_session_state(contents: &str) -> SessionState {
    let mut state = SessionState::Empty;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
            || value.get("isMeta").and_then(serde_json::Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        state = match value.get("type").and_then(serde_json::Value::as_str) {
            Some("assistant") => {
                if assistant_turn_ended(message) {
                    SessionState::WaitingForUser
                } else {
                    SessionState::Busy
                }
            }
            Some("user") => {
                if user_turn_is_tool_result(message) {
                    SessionState::Busy
                } else {
                    SessionState::AwaitingAssistant
                }
            }
            // Any other entry type is bookkeeping, not a turn: leave the state.
            _ => continue,
        };
    }
    state
}

/// Reads the existing directory a session ran in out of its located transcript.
///
/// The transcript's first non-empty `cwd` is taken as the working directory; the
/// call fails if the transcript cannot be read, records no `cwd`, or names a
/// directory that no longer exists or cannot be entered.
///
/// "Gone" and "refused" are answered separately because they are separate
/// realities, and the obvious `path.is_dir()` gets both of them wrong in a
/// cross-user world. A directory behind an ancestor that is opaque to this
/// account cannot even be `stat`ed, so `is_dir()` reports `false` and we would
/// tell the user a directory that is sitting right there is gone. A directory
/// that `stat`s fine but carries no search (`x`) bit reports `true`, so we would
/// print a resume, exit 0, and leave the shell function's `cd` to fail with the
/// binary that knew better already gone. `crap` must never hand the shell a
/// directory it cannot enter, so entering is *probed* rather than inferred.
///
/// The probe is what `cd` itself needs and nothing more: `0o600` (readable but
/// not searchable) is correctly unreadable, while `0o100` (searchable but not
/// listable) is correctly usable.
///
/// # Errors
///
/// Returns a [`ResolveError`] when the transcript cannot be read, records no
/// working directory, or names a directory that no longer exists
/// ([`ResolveError::DirectoryMissing`]) or cannot be entered from this account
/// ([`ResolveError::DirectoryUnreadable`]).
fn session_dir_from_transcript(transcript: &Path) -> Result<PathBuf, ResolveError> {
    let contents =
        std::fs::read_to_string(transcript).map_err(|_| ResolveError::SessionNotFound)?;
    let cwd = extract_cwd(&contents).ok_or(ResolveError::NoCwdInSession)?;
    let path = PathBuf::from(cwd);

    match std::fs::metadata(&path) {
        // Refused before we learned anything: an ancestor is opaque to us. Not
        // being allowed to look is not the same as knowing there is nothing
        // there, and only one of those two is the user's actual problem.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(ResolveError::DirectoryUnreadable(path))
        }
        // `NotFound`, and every other error too: none of them leaves a directory
        // we could hand to `cd`, and "no longer exists" is the useful summary.
        Err(_) => Err(ResolveError::DirectoryMissing(path)),
        // Something is at that path, but it is a file, a socket, …: the recorded
        // directory is not there any more even though the name is taken.
        Ok(meta) if !meta.is_dir() => Err(ResolveError::DirectoryMissing(path)),
        Ok(_) => {
            // A successful `stat` only proves the *parent* let us look; it says
            // nothing about whether we may enter. Resolving a path *inside* the
            // directory does, because that is the lookup the search (`x`) bit
            // guards — exactly the permission `cd` needs.
            //
            // This works because `Path::join(".")` appends a literal `.`
            // component (Rust does not normalize it away) and `std::fs::metadata`
            // passes the path straight to `stat(2)`, so the trailing `.` really
            // is resolved through the directory by the kernel. Do not "simplify"
            // this back to `metadata(&path)`: that is precisely the check that
            // cannot tell an enterable directory from a sealed one.
            match std::fs::metadata(path.join(".")) {
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    Err(ResolveError::DirectoryUnreadable(path))
                }
                // Either the probe succeeded, or it failed for some reason that
                // is not a refusal — in which case we were allowed to traverse,
                // which is all `cd` asks for.
                _ => Ok(path),
            }
        }
    }
}

/// Encodes a working directory into the project-folder name Claude Code uses
/// under `~/.claude/projects`.
///
/// Claude derives the folder name by replacing every character of the absolute
/// path that is not an ASCII letter or digit with `-`. So `/` and `.` both
/// become `-` (and a `/.` run becomes `--`), while existing hyphens are kept.
/// This is the lookup `claude --resume` performs against the *current*
/// directory, so reproducing it exactly is what lets `--here` drop a session
/// where Claude will find it.
///
/// The mapping is per Unicode scalar, which matches Claude for the ASCII paths
/// that real project directories use; non-ASCII characters each collapse to a
/// single `-`.
fn encode_project_dir(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// How [`prepare_import`] should make a session's transcript resolvable from a
/// target directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportMode {
    /// Symlink the target name to the source (same-user `--here`): the original
    /// is only ever read, and the link is cheap and trivially removable.
    Symlink,
    /// Copy the source into the target folder (cross-user): a self-contained
    /// snapshot owned by the current user, so nothing under another user's home
    /// is written and the import survives the source moving.
    Copy,
}

/// Makes the session `source_jsonl` resolvable by `claude --resume` from
/// `target_dir`, by placing it into `target_dir`'s project folder under
/// `dest_projects_dir` — either as a symlink to the original ([`ImportMode::Symlink`])
/// or as a self-contained copy ([`ImportMode::Copy`]).
///
/// `dest_projects_dir` is always the current user's tree, so every write lands
/// under the current user's home even when `source_jsonl` belongs to another
/// user.
///
/// Returns the path that was created (so the caller can remove it once the
/// session ends), or `None` when the session is *already* resolvable from
/// `target_dir` — because `target_dir` is its own directory, or an earlier
/// import already placed it. Anything already at the target name is left
/// untouched: a session id is a UUID, so a file there can only be this very
/// session, and clobbering it would never be correct.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] if the project folder cannot be
/// created or the symlink/copy cannot be made.
fn prepare_import(
    dest_projects_dir: &Path,
    source_jsonl: &Path,
    target_dir: &Path,
    session_id: &str,
    mode: ImportMode,
) -> std::io::Result<Option<PathBuf>> {
    let folder = dest_projects_dir.join(encode_project_dir(target_dir));
    let link_path = folder.join(format!("{session_id}.jsonl"));

    // Anything already at this name means the session resolves from here
    // already. `symlink_metadata` does not follow links, so even a dangling
    // symlink left by an earlier import counts as "present".
    if link_path.symlink_metadata().is_ok() {
        return Ok(None);
    }

    std::fs::create_dir_all(&folder)?;

    match mode {
        ImportMode::Symlink => {
            #[cfg(unix)]
            std::os::unix::fs::symlink(source_jsonl, &link_path)?;
            #[cfg(not(unix))]
            std::os::windows::fs::symlink_file(source_jsonl, &link_path)?;
        }
        ImportMode::Copy => {
            std::fs::copy(source_jsonl, &link_path)?;
        }
    }

    Ok(Some(link_path))
}

/// Why `--here` could not place a session under the current directory.
#[derive(Debug)]
enum HereResolveError {
    /// The session id was not a valid UUID.
    InvalidSessionId,
    /// No `<session_id>.jsonl` file was found under any *readable* location in
    /// the searched roots.
    ///
    /// Carries the owner-only directories the scan had to step over (see
    /// [`SkippedDir`]), so the failure the user sees can distinguish "that id is
    /// not on this machine" from "it is not anywhere I was allowed to look".
    SessionNotFound {
        /// The owner-only directories skipped while searching, in scan order.
        skipped: Vec<SkippedDir>,
    },
    /// Creating the project folder, or the symlink/copy, failed.
    Io(std::io::Error),
}

/// Validates `session_id`, locates its transcript across `roots`, and imports it
/// into `pwd`'s project folder under `dest_projects_dir` so `claude --resume`
/// will find it from there.
///
/// A hit in the current user's own tree (`is_self`) is symlinked, exactly as
/// before. A hit in another user's tree (`--here <id> --user <name>`) is
/// *copied* instead, so nothing is symlinked into another user's home and the
/// import is a self-contained snapshot owned by the current user.
/// `dest_projects_dir` is always the current user's tree, so every write lands
/// under the current user's home even when the source is foreign.
///
/// Returns the path of the symlink or copy to clean up afterwards, or `None`
/// when the session is already resolvable from `pwd` (no import needed).
///
/// # Errors
///
/// See [`HereResolveError`].
fn resolve_here_import(
    roots: &[UserProjects],
    dest_projects_dir: &Path,
    pwd: &Path,
    session_id: &str,
) -> Result<Option<PathBuf>, HereResolveError> {
    if !is_valid_session_id(session_id) {
        return Err(HereResolveError::InvalidSessionId);
    }
    let (path, root) = match find_session_across(roots, session_id) {
        FoundSession::Found { path, root } => (path, root),
        FoundSession::NotFound { skipped } => {
            return Err(HereResolveError::SessionNotFound { skipped });
        }
    };
    // Same-user hit → symlink (unchanged). Cross-user hit → copy, so nothing is
    // symlinked into another user's home and the import is a self-contained
    // snapshot owned by the current user.
    let mode = if root.is_self {
        ImportMode::Symlink
    } else {
        ImportMode::Copy
    };
    prepare_import(dest_projects_dir, &path, pwd, session_id, mode).map_err(HereResolveError::Io)
}

/// A caller-supplied `--here` new-session id that is not a valid UUID.
#[derive(Debug, PartialEq, Eq)]
struct InvalidNewSessionId;

/// Validates the optional forked-session id a caller passed as the second
/// `--here` argument.
///
/// Returns `Ok(None)` when none was supplied (so Claude mints a fresh random
/// id), `Ok(Some(id))` when it is a valid UUID, and `Err(InvalidNewSessionId)`
/// when one was supplied but is malformed. Validating up front keeps a bad id
/// from ever reaching the shell function's `claude --session-id`.
///
/// # Errors
///
/// Returns [`InvalidNewSessionId`] if `new_session_id` is `Some` but not a UUID.
fn resolve_new_session_id(
    new_session_id: Option<&str>,
) -> Result<Option<&str>, InvalidNewSessionId> {
    match new_session_id {
        None => Ok(None),
        Some(id) if is_valid_session_id(id) => Ok(Some(id)),
        Some(_) => Err(InvalidNewSessionId),
    }
}

/// Whether pinning a `--here` fork to `new_session_id` would collide with an
/// existing transcript.
///
/// `claude --session-id <id>` writes to `<id>.jsonl`, so reusing an id that
/// already names a session would let the fork overwrite an unrelated
/// conversation — the opposite of `--here`'s "leave the original untouched"
/// guarantee. `None` (no forced id, Claude mints a random one) never collides.
fn new_session_id_collides(projects_dir: &Path, new_session_id: Option<&str>) -> bool {
    match new_session_id {
        Some(id) => find_session_file(projects_dir, id).is_some(),
        None => false,
    }
}

/// Returns the `~/.claude/sessions` directory, or `None` if the home directory
/// cannot be determined.
///
/// Claude Code writes one `<pid>.json` file here per live CLI session and
/// removes it on clean exit, so it serves as a registry of running sessions.
fn claude_sessions_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".claude").join("sessions"))
}

/// A running Claude CLI session, as recorded under `~/.claude/sessions`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRecord {
    /// The process id of the running `claude` process.
    pid: u32,
    /// The session id this process is attached to.
    session_id: String,
    /// The directory the session is running in.
    cwd: String,
    /// The reported activity status (e.g. `"busy"` or `"idle"`), if present.
    status: Option<String>,
}

/// Parses a `~/.claude/sessions/<pid>.json` record.
///
/// Returns `None` if the JSON is malformed or is missing the `pid`/`sessionId`
/// fields that uniquely identify a running session.
fn parse_session_record(json: &str) -> Option<SessionRecord> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
    let session_id = value.get("sessionId")?.as_str()?.to_string();
    let cwd = value
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(SessionRecord {
        pid,
        session_id,
        cwd,
        status,
    })
}

/// Finds a currently-running session attached to `session_id`.
///
/// Scans every `<pid>.json` under `sessions_dir` and returns the first record
/// whose `session_id` matches and whose pid `is_alive` reports as still
/// running. The `is_alive` predicate is injected so this logic is testable
/// without spawning real processes.
fn find_live_session<F>(sessions_dir: &Path, session_id: &str, is_alive: F) -> Option<SessionRecord>
where
    F: Fn(u32) -> bool,
{
    for entry in std::fs::read_dir(sessions_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(record) = parse_session_record(&contents) else {
            continue;
        };
        if record.session_id == session_id && is_alive(record.pid) {
            return Some(record);
        }
    }
    None
}

/// Reports whether `pid` is a currently-running Claude CLI process.
///
/// Uses `ps -p <pid> -o command=`: a non-empty, successful result means the pid
/// exists, and requiring `claude` in the command line guards against a stale
/// session file whose pid has since been reused by an unrelated process.
fn pid_is_alive(pid: u32) -> bool {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout)
        .to_lowercase()
        .contains("claude")
}

/// Why `crap --status` could not report a session's state.
#[derive(Debug)]
enum StatusError {
    /// The session id was not a valid UUID.
    InvalidSessionId,
    /// No `<session_id>.jsonl` file was found under any *readable* location in
    /// the searched roots.
    ///
    /// Carries the owner-only directories the cross-user scan had to step over
    /// (see [`SkippedDir`]), so a status miss can hand back the same "it is not
    /// anywhere I was allowed to look" guidance the resume forms do, rather than
    /// asserting a "not on this machine" certainty an opaque directory denies it.
    SessionNotFound {
        /// The owner-only directories skipped while searching, in scan order.
        skipped: Vec<SkippedDir>,
    },
}

/// Returns the state line for a session that is open in a live `claude`
/// process — `"<status> (live, pid <pid>)"` — or `None` when no such process is
/// attached. A live process's own reported status is more authoritative than
/// anything inferred from the transcript, so callers prefer it.
fn live_state_string<F>(sessions_dir: &Path, session_id: &str, is_alive: F) -> Option<String>
where
    F: Fn(u32) -> bool,
{
    find_live_session(sessions_dir, session_id, is_alive).map(|live| {
        let status = live.status.as_deref().unwrap_or("running");
        format!("{status} (live, pid {})", live.pid)
    })
}

/// One session's status for the per-directory listing `crap --status` prints
/// when given no id.
///
/// Serializes to camelCase JSON (`sessionId`, …) for the `--json` form;
/// `started`/`last` become `null` when no timestamp was recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionStatusReport {
    /// The session id (the `.jsonl` filename stem under the project folder).
    session_id: String,
    /// The state line: a [`SessionState`] token, or `"<status> (live, pid N)"`
    /// when a `claude` process is attached.
    state: String,
    /// The earliest `timestamp` recorded in the transcript (ISO 8601 UTC), or
    /// `None` if no line carries one.
    started: Option<String>,
    /// The latest `timestamp` recorded in the transcript (ISO 8601 UTC), or
    /// `None` if no line carries one.
    last: Option<String>,
}

/// Returns the earliest and latest `timestamp` values found in a transcript.
///
/// Claude writes ISO 8601 UTC timestamps (`…Z`, fixed width) on conversational
/// and system entries, but not on bookkeeping lines. Because the format is
/// fixed-width and always UTC, lexicographic ordering matches chronological
/// ordering, so the earliest/latest are just the string min/max — no date
/// parsing, and the result is independent of line order. Returns `(None, None)`
/// when no line carries a timestamp.
fn transcript_time_span(contents: &str) -> (Option<String>, Option<String>) {
    let mut earliest: Option<String> = None;
    let mut latest: Option<String> = None;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(ts) = value.get("timestamp").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if earliest.as_deref().is_none_or(|e| ts < e) {
            earliest = Some(ts.to_string());
        }
        if latest.as_deref().is_none_or(|l| ts > l) {
            latest = Some(ts.to_string());
        }
    }
    (earliest, latest)
}

/// Prettifies an ISO 8601 UTC timestamp (`2026-05-25T18:43:05.109Z`) into a
/// human `2026-05-25 18:43:05`, dropping the sub-second fraction and zone.
///
/// Input that does not match the expected shape is returned unchanged.
fn format_timestamp(raw: &str) -> String {
    let Some((date, rest)) = raw.split_once('T') else {
        return raw.to_string();
    };
    let time = rest.split(['.', 'Z']).next().unwrap_or(rest);
    if date.is_empty() || time.is_empty() {
        return raw.to_string();
    }
    format!("{date} {time}")
}

/// Lists the status of every session whose transcript lives in `pwd`'s project
/// folder under `projects_dir`.
///
/// This backs `crap --status` with no id: it enumerates `<uuid>.jsonl` files in
/// the folder Claude would use for `pwd`, classifying each (live process status
/// taking precedence over transcript inference) and recording its time span.
/// Results are ordered ascending by last-activity, so the most recently used
/// session is last. `is_alive` is injected so liveness can be tested without
/// spawning processes.
fn resolve_dir_statuses<F>(
    projects_dir: &Path,
    sessions_dir: &Path,
    pwd: &Path,
    is_alive: F,
) -> Vec<SessionStatusReport>
where
    F: Fn(u32) -> bool + Copy,
{
    let folder = projects_dir.join(encode_project_dir(pwd));
    let mut reports = Vec::new();
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return reports;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(session_id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_valid_session_id(session_id) {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let state = live_state_string(sessions_dir, session_id, is_alive)
            .unwrap_or_else(|| classify_session_state(&contents).as_token().to_string());
        let (started, last) = transcript_time_span(&contents);
        reports.push(SessionStatusReport {
            session_id: session_id.to_string(),
            state,
            started,
            last,
        });
    }
    // Ascending by last-activity, so the most recently used session sits at the
    // bottom of the printed table; timestamp-less sessions (which sort first)
    // and ties break by id, keeping the order deterministic regardless of
    // directory iteration order.
    reports.sort_by(|a, b| {
        a.last
            .cmp(&b.last)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    reports
}

/// Resolves the full [`SessionStatusReport`] for a single session id, searching
/// the given `roots` in order.
///
/// Unlike the bare token, this also carries the transcript's time span, so the
/// JSON form of `crap --status <id>` can include start/last times. A live
/// process's status still takes precedence for the `state` field; the
/// transcript is read (best-effort) for the times either way.
///
/// Status only ever *reads*: it reports where a session left off without copying
/// or forking, so a foreign hit under another user's root is simply classified
/// in place. The current user's own `~/.claude/sessions` registry is the only one
/// consulted for liveness, so a foreign session that is live in another account's
/// process is reported from its transcript rather than as live — the registry
/// under another home is typically unreadable anyway, and status never writes.
///
/// # Errors
///
/// Returns [`StatusError::InvalidSessionId`] for a malformed id, or
/// [`StatusError::SessionNotFound`] when the id is neither live nor on disk.
fn resolve_status_report<F>(
    roots: &[UserProjects],
    sessions_dir: &Path,
    session_id: &str,
    is_alive: F,
) -> Result<SessionStatusReport, StatusError>
where
    F: Fn(u32) -> bool,
{
    if !is_valid_session_id(session_id) {
        return Err(StatusError::InvalidSessionId);
    }
    let live = live_state_string(sessions_dir, session_id, is_alive);
    // Locate the transcript across the roots (self first, then siblings, or one
    // sibling for `--user`), so a session under another user is reported in
    // place. The match is only ever read here — status copies and forks nothing.
    // A miss keeps the owner-only directories the scan stepped over, so a
    // not-found can carry the same guidance the resume forms print.
    let (contents, skipped) = match find_session_across(roots, session_id) {
        FoundSession::Found { path, .. } => (std::fs::read_to_string(&path).ok(), Vec::new()),
        FoundSession::NotFound { skipped } => (None, skipped),
    };
    let (started, last) = contents
        .as_deref()
        .map_or((None, None), transcript_time_span);
    let state = match live {
        Some(line) => line,
        None => {
            // Not live: the transcript is the only evidence, so it must exist.
            let contents = contents.ok_or(StatusError::SessionNotFound { skipped })?;
            classify_session_state(&contents).as_token().to_string()
        }
    };
    Ok(SessionStatusReport {
        session_id: session_id.to_string(),
        state,
        started,
        last,
    })
}

/// Serializes a single session's status report as pretty JSON.
fn format_status_json(report: &SessionStatusReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string())
}

/// Serializes a directory's session status reports as a pretty JSON array.
fn format_dir_statuses_json(reports: &[SessionStatusReport]) -> String {
    serde_json::to_string_pretty(reports).unwrap_or_else(|_| "[]".to_string())
}

/// Renders the per-directory `crap --status` listing for `pwd` as a table.
///
/// An empty listing reports that nothing was found; otherwise a heading line is
/// followed by a table with one row per session. A session with no recorded
/// activity shows an em-dash in its time columns.
fn format_dir_statuses(pwd: &Path, reports: &[SessionStatusReport]) -> String {
    if reports.is_empty() {
        return format!("No Claude sessions found for {}\n", pwd.display());
    }
    let count = reports.len();
    let noun = if count == 1 { "session" } else { "sessions" };
    let table = dir_statuses_table(reports);
    format!("{count} {noun} for {}\n\n{table}\n", pwd.display())
}

/// Builds the session-status table — one row per report, timestamps prettified
/// into the cells — ready for rendering.
///
/// Rendered with [`ContentArrangement::Disabled`] so each column is sized to its
/// own content and the table is laid out at its natural width, **never wrapping
/// a cell to fit the terminal**. Wrapping would chop a session UUID or timestamp
/// across lines (unreadable), and — because the dynamic arrangement reads the
/// ambient terminal width — would make the output depend on a shared,
/// uncontrolled resource, so the rendering (and any test asserting on it) would
/// silently change with the window size. Long rows simply overflow and let the
/// terminal soft-wrap.
fn dir_statuses_table(reports: &[SessionStatusReport]) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Disabled)
        .set_header(vec!["SESSION", "STATE", "STARTED", "LAST"]);
    for report in reports {
        let started = report
            .started
            .as_deref()
            .map_or_else(|| "—".to_string(), format_timestamp);
        let last = report
            .last
            .as_deref()
            .map_or_else(|| "—".to_string(), format_timestamp);
        table.add_row(vec![
            report.session_id.clone(),
            report.state.clone(),
            started,
            last,
        ]);
    }
    table
}

/// Formats the binary's success output for the shell function to read back.
///
/// The session id (a validated UUID) is emitted first, on its own line; the
/// directory comes last. Putting the variable-content directory last means a
/// path that itself contains a newline can't be mistaken for the end of the
/// session-id field — the shell function takes the first line as the session id
/// and everything after it as the directory.
fn format_output(dir: &Path, session_id: &str) -> String {
    format!("{session_id}\n{}\n", dir.display())
}

/// Leading token marking `--here` output, distinguishing it from the default
/// `<session-id>\n<dir>` resume output the shell function otherwise expects.
const HERE_SENTINEL: &str = "__CRAP_HERE__";

/// Placeholder used in the link field when no import was created — because
/// `--here`'s target directory already is the session's own folder, or, via
/// [`format_fork_at_output`], a cross-user resume needed none — so the shell
/// function can tell "nothing to clean up" apart from a real path.
const NO_LINK_SENTINEL: &str = "__CRAP_NO_LINK__";

/// Placeholder used in the forced-new-id field when `--here` was given no
/// explicit new session id, so the shell function knows to let Claude mint a
/// fresh random id (`--fork-session`) instead of pinning one (`--session-id`).
const NO_NEW_ID_SENTINEL: &str = "__CRAP_NO_NEW_ID__";

/// Leading token marking cross-user default-resume output: unlike [`HERE_SENTINEL`]
/// (which stays in the current directory), this tells the shell function to
/// `cd` into the session's original recorded directory *and then* fork, so a
/// foreign session resumes where it originally ran.
const FORK_AT_SENTINEL: &str = "__CRAP_FORK_AT__";

/// Formats `--here` output for the shell function: the [`HERE_SENTINEL`], then
/// the session id, then the caller-supplied forked-session id (or
/// [`NO_NEW_ID_SENTINEL`] when none was given), then the import to remove once
/// the session ends (or [`NO_LINK_SENTINEL`] when none was created).
///
/// The cleanup path is emitted last so that — like [`format_output`] — a path
/// containing a newline survives intact as "everything after the final field
/// separator". The forced-new-id is a validated UUID (or the sentinel), so it
/// never contains a newline and is safe in the middle.
fn format_here_output(
    session_id: &str,
    new_session_id: Option<&str>,
    link_to_cleanup: Option<&Path>,
) -> String {
    let new_id = new_session_id.unwrap_or(NO_NEW_ID_SENTINEL);
    let link = match link_to_cleanup {
        Some(path) => path.display().to_string(),
        None => NO_LINK_SENTINEL.to_string(),
    };
    format!("{HERE_SENTINEL}\n{session_id}\n{new_id}\n{link}\n")
}

/// Formats cross-user default-resume output for the shell function: the
/// [`FORK_AT_SENTINEL`], then the session id, then the forked-session id (or
/// [`NO_NEW_ID_SENTINEL`]), then the imported transcript to remove once the
/// session ends (or [`NO_LINK_SENTINEL`]), and finally the session's original
/// directory to `cd` into before forking.
///
/// The directory is emitted **last** so — like [`format_output`] — a path
/// containing a newline survives intact as "everything after the final field
/// separator". The middle fields are all newline-free: the ids are validated
/// UUIDs (or sentinels), and the link path lives under
/// `~/.claude/projects/<encoded>`, whose encoding maps every non-alphanumeric
/// character (including newline) to `-`.
fn format_fork_at_output(
    session_id: &str,
    new_session_id: Option<&str>,
    link_to_cleanup: Option<&Path>,
    dir: &Path,
) -> String {
    let new_id = new_session_id.unwrap_or(NO_NEW_ID_SENTINEL);
    let link = match link_to_cleanup {
        Some(path) => path.display().to_string(),
        None => NO_LINK_SENTINEL.to_string(),
    };
    format!(
        "{FORK_AT_SENTINEL}\n{session_id}\n{new_id}\n{link}\n{}\n",
        dir.display()
    )
}

/// The shell function installed by `crap --shell-setup`.
///
/// `crap` shadows the binary, so the function reaches the binary explicitly via
/// `command crap`, forwarding all arguments (so flags like `--force` and
/// `--here` work). `clauded` is resolved through `eval` so that an alias of that
/// name is expanded at call time (shell aliases are otherwise not expanded
/// inside function bodies); if no `clauded` exists, plain `claude` is used. If
/// the binary exits non-zero (session not found, already running, …) its message
/// is shown and the function does nothing further.
///
/// The binary speaks one of three output shapes:
///
/// * **default** — `<session-id>\n<dir>`: the function `cd`s into the original
///   directory (splitting on the first newline so a path containing newlines
///   survives intact) and resumes.
/// * **`--here`** — `__CRAP_HERE__\n<session-id>\n<new-id-or-sentinel>\n<link-or-sentinel>`:
///   the binary has already imported the session into the *current* directory's
///   project folder — a symlink for a same-user source, or a copy for a
///   cross-user `--user` source — so the function stays put, resumes with
///   `--fork-session` (a fresh session id, leaving the original transcript
///   untouched), and finally removes that import — unless the link field is
///   `__CRAP_NO_LINK__`, meaning none was created because this already is the
///   session's own directory. When the new-id field is not
///   `__CRAP_NO_NEW_ID__`, the fork is pinned to that id via `--session-id`
///   instead of a random one.
/// * **cross-user** — `__CRAP_FORK_AT__\n<session-id>\n<new-id-or-sentinel>\n<link-or-sentinel>\n<dir>`:
///   the binary has *copied* another user's transcript into our own tree, so the
///   function `cd`s into the session's original directory (the trailing `<dir>`,
///   emitted last so a newline in the path survives) and then runs the same
///   fork + cleanup sequence as `--here`.
const SHELL_CODE: &str = r#"
function crap() {
    # These flags make the binary print to stdout and exit 0 without mutating
    # the parent shell: --status queries, --help/-h/--version/-V emit
    # informational text, and --shell-setup writes the rc file (not the live
    # shell) and prints activation instructions. Run them straight through so
    # their output reaches the terminal instead of being parsed as a
    # "<session-id>\n<dir>" resume target (which would otherwise `cd` into that
    # text and mangle it). --shell-setup matters on upgrades, when this very
    # function is already loaded and would otherwise swallow its instructions.
    case " $* " in
        *" --status "*|*" --help "*|*" -h "*|*" --version "*|*" -V "*|*" --shell-setup "*)
            command crap "$@"; return $? ;;
    esac
    local __crap_out
    __crap_out=$(command crap "$@") || return $?
    if [ "${__crap_out%%$'\n'*}" = "__CRAP_HERE__" ]; then
        local __crap_rest __crap_session __crap_newid __crap_link __crap_folder __crap_n0 __crap_watcher
        __crap_rest=${__crap_out#*$'\n'}
        __crap_session=${__crap_rest%%$'\n'*}
        __crap_rest=${__crap_rest#*$'\n'}
        __crap_newid=${__crap_rest%%$'\n'*}
        __crap_link=${__crap_rest#*$'\n'}
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            # Claude only needs the import (a symlink, or a copy for a
            # cross-user source) while it reads the transcript at startup;
            # once it writes the forked session file, the import is
            # vestigial. Watch the folder and drop it the moment a new .jsonl
            # appears, rather than letting it linger for the whole session.
            __crap_folder=$(dirname -- "$__crap_link")
            __crap_n0=$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')
            (
                __crap_i=0
                while [ "$__crap_i" -lt 600 ]; do
                    if [ "$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')" -gt "$__crap_n0" ]; then
                        rm -f -- "$__crap_link"
                        exit 0
                    fi
                    __crap_i=$((__crap_i + 1))
                    sleep 0.1
                done
            ) &
            __crap_watcher=$!
            disown 2>/dev/null
        fi
        # Build the resume argv: always --fork-session, so the original
        # transcript is left untouched. When the binary supplied a forced id
        # (third field is not the sentinel), pin the fork to it with
        # --session-id instead of letting Claude mint a random one. The earlier
        # "command crap" call has already consumed the function's own arguments,
        # so reusing the positional parameters here is safe.
        set -- --resume "$__crap_session" --fork-session
        if [ "$__crap_newid" != "__CRAP_NO_NEW_ID__" ]; then
            set -- "$@" --session-id "$__crap_newid"
        fi
        if command -v clauded >/dev/null 2>&1; then
            eval 'clauded "$@"'
        else
            claude "$@"
        fi
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            kill "$__crap_watcher" 2>/dev/null
            rm -f -- "$__crap_link"
        fi
        return
    fi
    if [ "${__crap_out%%$'\n'*}" = "__CRAP_FORK_AT__" ]; then
        # Cross-user resume: the binary copied a foreign transcript into our own
        # tree and wants it forked at the session's ORIGINAL directory. The wire
        # shape adds a trailing <dir> field to the here-mode layout —
        # "__CRAP_FORK_AT__\n<session>\n<new-id>\n<link>\n<dir>" — with <dir>
        # last so a path containing newlines survives as the final field. We cd
        # there, then run the same fork + cleanup sequence as --here.
        local __crap_rest __crap_session __crap_newid __crap_link __crap_dir __crap_folder __crap_n0 __crap_watcher
        __crap_rest=${__crap_out#*$'\n'}
        __crap_session=${__crap_rest%%$'\n'*}
        __crap_rest=${__crap_rest#*$'\n'}
        __crap_newid=${__crap_rest%%$'\n'*}
        __crap_rest=${__crap_rest#*$'\n'}
        __crap_link=${__crap_rest%%$'\n'*}
        __crap_dir=${__crap_rest#*$'\n'}
        cd -- "$__crap_dir" || return 1
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            # As in --here: drop the imported copy the moment Claude writes the
            # forked session file, rather than letting it linger.
            __crap_folder=$(dirname -- "$__crap_link")
            __crap_n0=$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')
            (
                __crap_i=0
                while [ "$__crap_i" -lt 600 ]; do
                    if [ "$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')" -gt "$__crap_n0" ]; then
                        rm -f -- "$__crap_link"
                        exit 0
                    fi
                    __crap_i=$((__crap_i + 1))
                    sleep 0.1
                done
            ) &
            __crap_watcher=$!
            disown 2>/dev/null
        fi
        set -- --resume "$__crap_session" --fork-session
        if [ "$__crap_newid" != "__CRAP_NO_NEW_ID__" ]; then
            set -- "$@" --session-id "$__crap_newid"
        fi
        if command -v clauded >/dev/null 2>&1; then
            eval 'clauded "$@"'
        else
            claude "$@"
        fi
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            kill "$__crap_watcher" 2>/dev/null
            rm -f -- "$__crap_link"
        fi
        return
    fi
    local __crap_session __crap_dir
    __crap_session=${__crap_out%%$'\n'*}
    __crap_dir=${__crap_out#*$'\n'}
    cd -- "$__crap_dir" || return 1
    if command -v clauded >/dev/null 2>&1; then
        eval 'clauded --resume "$__crap_session"'
    else
        claude --resume "$__crap_session"
    fi
}
"#;

/// Installs the `crap` shell function into the user's shell config.
fn setup_shell_integration() -> Result<(), shellsetup::ShellSetupError> {
    let integration = ShellIntegration::new("crap", "Claude, Resume Anywhere Please", SHELL_CODE)
        .with_command(
            "crap",
            "Resume a Claude session from its original directory",
        );
    integration.setup()
}

/// Command-line interface for `crap`.
#[derive(Parser)]
#[command(name = "crap")]
#[command(
    about = "Claude, Resume Anywhere Please — resume a Claude session from its original directory"
)]
#[command(version = version_string!())]
struct Cli {
    /// The Claude session id to resume (the `.jsonl` filename under
    /// `~/.claude/projects`).
    ///
    /// The lookup is self-first: your own tree is searched first and, only if
    /// the id isn't there, every sibling home that has run Claude — so an id
    /// that belongs to another account is found without any flag, copied into
    /// your own tree and forked there (see `--user`).
    ///
    /// Optional with `--status`: given no id, `--status` lists every session
    /// recorded for the current directory instead of resolving one.
    #[arg(
        value_name = "SESSION_ID",
        required_unless_present_any = ["shell_setup", "status"]
    )]
    session_id: Option<String>,

    /// Id to assign the forked session created by `--here` (must be a UUID).
    ///
    /// Only valid with `--here`. When given, the resumed fork is created with
    /// this exact id (`claude --fork-session --session-id <id>`) instead of a
    /// random one — useful when a caller needs to know the new id in advance.
    /// Without it, Claude mints a fresh random id as before.
    #[arg(value_name = "NEW_SESSION_ID", requires = "here")]
    new_session_id: Option<String>,

    /// Resume even if the session appears to be running in another process.
    ///
    /// By default `crap` refuses to resume a session that is already open
    /// elsewhere, because two processes writing the same session log can
    /// corrupt it.
    ///
    /// This guard applies only to the default resume mode. `--here` forks a
    /// fresh session (it only reads the original transcript), so it is never
    /// blocked by a live original and ignores `--force`.
    #[arg(short, long)]
    force: bool,

    /// Resume the session in the *current* directory instead of its original.
    ///
    /// `crap` symlinks the session into the current directory's project folder
    /// so `claude --resume` can find it here, then resumes it as a forked
    /// (new-id) session — the original transcript is only read, never written,
    /// so this works even if the original session is still live. Use this to
    /// carry a conversation's context into a different working directory.
    ///
    /// Composes with `--user <name>`: a cross-user source is *copied* into your
    /// own tree rather than symlinked, so nothing is linked into another user's
    /// home; a same-user source is symlinked exactly as above.
    #[arg(long)]
    here: bool,

    /// Resume another user's session from a specific account.
    ///
    /// `<name>` is resolved as a sibling of your own home (`<home>/../<name>`,
    /// i.e. `/Users/<name>` on macOS or `/home/<name>` on Linux). Only that
    /// user's `~/.claude/projects` tree is searched — your own is skipped, so
    /// `--user` is also how you disambiguate an id on purpose. A readable
    /// foreign transcript is copied into your own tree and resumed as a fork (a
    /// fresh id) at its original directory; the original is only ever read, so
    /// this is safe even while that session is live elsewhere. A `--user`
    /// naming your own account is a same-user hit and resumes in place. With
    /// `--here`, the fork lands in the current directory instead of the
    /// session's original one.
    ///
    /// Not required for a cross-user resume: with no `--user` the lookup is
    /// self-first and falls back to the sibling homes on its own, so the flag
    /// is only for forcing one particular account.
    #[arg(long, value_name = "NAME", conflicts_with = "shell_setup")]
    user: Option<String>,

    /// Print the session's conversational state and exit, without resuming.
    ///
    /// With a session id, emits one scriptable token: `waiting-for-user`
    /// (Claude finished and is waiting on you), `busy` (a tool call or reply is
    /// in flight), `awaiting-assistant` (you spoke last and Claude hasn't
    /// replied), or `empty`. If the session is currently open in a live `claude`
    /// process, its own status is reported instead, as
    /// `<status> (live, pid <pid>)`.
    ///
    /// With no id, lists every session recorded for the current directory —
    /// each with its state and the times its transcript was started and last
    /// written.
    #[arg(long)]
    status: bool,

    /// Emit machine-readable JSON instead of human-formatted text.
    ///
    /// Only valid with `--status`. With a session id it prints one object;
    /// with no id it prints an array of one object per session. Timestamps are
    /// the raw ISO 8601 values from the transcript.
    #[arg(long, requires = "status")]
    json: bool,

    /// Install the `crap` shell function into your shell config, then exit.
    ///
    /// Run this once: `crap --shell-setup`. After re-sourcing your shell,
    /// `crap <session-id>` will cd into the session's directory and resume it.
    #[arg(long)]
    shell_setup: bool,
}

/// Whether a session that is already live in another process should block a
/// resume of it.
///
/// The default resume mode reuses the session id, so a second process would
/// append to the same transcript as the live one — that can corrupt it, so a
/// live session blocks the resume unless `--force` overrides it. `--here`
/// resumes with `--fork-session`, which only *reads* the original transcript
/// and writes a fresh file, so a live original can never be corrupted by it —
/// `--here` therefore never blocks (and `--force` is irrelevant to it).
fn should_block_for_live(here: bool, force: bool) -> bool {
    !here && !force
}

/// Aborts with a clear message if `session_id` is already open in another live
/// process and [`should_block_for_live`] says that should block this resume.
fn abort_if_session_live(session_id: &str, here: bool, force: bool) {
    if !should_block_for_live(here, force) {
        return;
    }
    if let Some(live) =
        claude_sessions_dir().and_then(|s| find_live_session(&s, session_id, pid_is_alive))
    {
        let status = live.status.as_deref().unwrap_or("running");
        eprintln!(
            "{} session '{session_id}' is already running (pid {}, {status})",
            "Error:".red().bold(),
            live.pid
        );
        eprintln!("       in {}", live.cwd);
        eprintln!("       resuming it again can corrupt the session log.");
        eprintln!(
            "       re-run with {} to resume anyway.",
            "--force".yellow()
        );
        exit(exit_codes::SESSION_ALREADY_RUNNING);
    }
}

/// Reports a session id that was not found under any searched root, and exits
/// with [`exit_codes::SESSION_NOT_FOUND`].
///
/// Every not-found path goes through here so the message is assembled in exactly
/// one place: the headline, the "where I looked" summary, and the owner-only
/// guidance are three facets of one answer, and splitting them across call sites
/// is how they drift. The headline hedges — "no *readable* Claude session" —
/// precisely when the scan was refused entry somewhere, because with an opaque
/// directory in the way "it is not here" is a certainty `crap` does not have; a
/// clean miss keeps the flat wording it has always had.
///
/// The working directory is resolved here rather than threaded in from the
/// callers, because it is only ever needed for the guidance. A failure is
/// harmless: it costs the `mkdir` line its destination, while the count lines,
/// the account name, and the `sudo -u <user> find` that actually locates the
/// transcript are all still there.
fn exit_session_not_found(session_id: &str, roots: &[UserProjects], skipped: &[SkippedDir]) -> ! {
    let pwd = std::env::current_dir().unwrap_or_default();
    let qualifier = if skipped.is_empty() { "" } else { "readable " };
    eprintln!(
        "{} no {qualifier}Claude session found with id '{session_id}'",
        "Error:".red().bold()
    );
    eprint!("{}", format_searched_roots(roots));
    eprint!(
        "{}",
        format_owner_only_guidance(session_id, roots, skipped, &pwd)
    );
    exit(exit_codes::SESSION_NOT_FOUND);
}

/// Handles `crap --here <id> [<new-id>] [--user <name>]`: import the session
/// into the current directory's project folder and emit the here-mode output
/// the shell function consumes, optionally pinning the forked session's id to
/// `new_session_id`.
///
/// The session is located across `roots` (the current user's own tree, or a
/// sibling's tree when `--user` was given). A same-user hit is symlinked; a
/// cross-user hit is copied into `dest_projects_dir` (always the current user's
/// tree). Either way `--here` stays in the current directory and forks, so a
/// live original is never blocked or corrupted.
fn run_here(
    roots: &[UserProjects],
    dest_projects_dir: &Path,
    session_id: &str,
    new_session_id: Option<&str>,
    force: bool,
) -> ! {
    let Ok(pwd) = std::env::current_dir() else {
        eprintln!(
            "{} could not determine the current directory",
            "Error:".red().bold()
        );
        exit(exit_codes::HERE_PWD_UNAVAILABLE);
    };

    // Validate the optional forced id before creating anything, so a bad id
    // aborts without leaving a stray import behind.
    let new_id = match resolve_new_session_id(new_session_id) {
        Ok(id) => id,
        Err(InvalidNewSessionId) => {
            eprintln!(
                "{} '{}' is not a valid session id",
                "Error:".red().bold(),
                new_session_id.unwrap_or_default()
            );
            exit(exit_codes::INVALID_SESSION_ID);
        }
    };

    // Refuse to pin the fork to an id that already names a transcript: that
    // would let `claude --session-id` overwrite an unrelated session. The fork
    // lands in our own tree, so the collision is checked there.
    if new_session_id_collides(dest_projects_dir, new_id) {
        eprintln!(
            "{} a session with id '{}' already exists",
            "Error:".red().bold(),
            new_id.unwrap_or_default()
        );
        eprintln!("       choose a fresh id so the fork does not overwrite it");
        exit(exit_codes::NEW_SESSION_ID_EXISTS);
    }

    // Guard before creating anything, so an aborted resume leaves no stray link.
    abort_if_session_live(session_id, true, force);

    match resolve_here_import(roots, dest_projects_dir, &pwd, session_id) {
        Ok(link) => {
            print!(
                "{}",
                format_here_output(session_id, new_id, link.as_deref())
            );
            exit(0);
        }
        Err(HereResolveError::InvalidSessionId) => {
            eprintln!(
                "{} '{session_id}' is not a valid session id",
                "Error:".red().bold()
            );
            exit(exit_codes::INVALID_SESSION_ID);
        }
        Err(HereResolveError::SessionNotFound { skipped }) => {
            exit_session_not_found(session_id, roots, &skipped);
        }
        Err(HereResolveError::Io(err)) => {
            eprintln!(
                "{} could not prepare this directory for '{session_id}': {err}",
                "Error:".red().bold()
            );
            exit(exit_codes::HERE_LINK_ERROR);
        }
    }
}

/// Handles `crap --status <id> [--user <name>]`: print the session's state to
/// stdout and exit.
///
/// The session is located across `roots` — the current user's own tree (self
/// first, then sibling homes on a miss), or a single sibling's tree when `--user`
/// was given — exactly like the resume forms, so a foreign session's state is
/// reported without ever copying, forking, or writing anywhere. Prints the bare
/// state token by default, or the full report as JSON when `json` is set. A miss
/// goes through the shared not-found path, so an id that turned up in nothing
/// readable while owner-only directories were skipped gets the same actionable
/// guidance the resume forms print.
fn run_status(roots: &[UserProjects], session_id: &str, json: bool) -> ! {
    let sessions_dir = claude_sessions_dir().unwrap_or_default();
    match resolve_status_report(roots, &sessions_dir, session_id, pid_is_alive) {
        Ok(report) => {
            if json {
                println!("{}", format_status_json(&report));
            } else {
                println!("{}", report.state);
            }
            exit(0);
        }
        Err(StatusError::InvalidSessionId) => {
            eprintln!(
                "{} '{session_id}' is not a valid session id",
                "Error:".red().bold()
            );
            exit(exit_codes::INVALID_SESSION_ID);
        }
        Err(StatusError::SessionNotFound { skipped }) => {
            exit_session_not_found(session_id, roots, &skipped);
        }
    }
}

/// Handles `crap --status` with no id: list every session recorded for the
/// current directory, then exit.
///
/// Prints a table by default, or a JSON array when `json` is set.
fn run_dir_status(projects_dir: &Path, json: bool) -> ! {
    let Ok(pwd) = std::env::current_dir() else {
        eprintln!(
            "{} could not determine the current directory",
            "Error:".red().bold()
        );
        exit(exit_codes::HERE_PWD_UNAVAILABLE);
    };
    let sessions_dir = claude_sessions_dir().unwrap_or_default();
    let reports = resolve_dir_statuses(projects_dir, &sessions_dir, &pwd, pid_is_alive);
    if json {
        println!("{}", format_dir_statuses_json(&reports));
    } else {
        print!("{}", format_dir_statuses(&pwd, &reports));
    }
    exit(0);
}

/// What `crap` should print, and exit with, for a session it located but could
/// not resolve to a usable working directory.
///
/// Bundling the three facets of one answer — headline, detail, exit code — is
/// what keeps them from drifting: the `--here` hint is only ever true advice
/// when the *directory* is the problem (the transcript read fine, only its cwd
/// is gone or sealed), and welding "print the hint" to "exit `DIRECTORY_MISSING`
/// / `DIRECTORY_UNREADABLE`" in a single value is what guarantees the two always
/// travel together. Carried as plain text with the caller colorizing the
/// `Error:` prefix, matching every other formatter in this file, so the mapping
/// stays unit-testable without spawning a subprocess.
struct ResolveFailure {
    /// The headline, printed after the red `Error:` prefix.
    headline: String,
    /// The indented detail lines that follow it, already newline-terminated —
    /// empty when there is nothing more to say.
    detail: String,
    /// The process exit code.
    code: i32,
}

/// Maps a [`ResolveError`] to the failure it should produce.
///
/// The two directory variants carry the `crap --here <id>` escape hatch because
/// `--here` ignores the recorded directory entirely, so it succeeds in exactly
/// the case that just failed. The two non-directory variants deliberately do
/// not: a transcript that could not be read, or that records no cwd at all, is
/// not something `--here` can route around, and dangling the hint there would
/// be a false lead. That "only the directory failures get the hint" rule lives
/// here, in one place, precisely so it cannot be half-applied.
fn describe_resolve_error(session_id: &str, err: &ResolveError) -> ResolveFailure {
    /// The hanging indent that aligns a detail line under the `Error:` prefix,
    /// matching every other multi-line message in this binary.
    const INDENT: &str = "       ";

    // Both directory failures share one detail body: the offending path, then the
    // escape hatch. Only the headline distinguishes "gone" from "sealed", so the
    // shared shape is built once and the two arms supply just the headline.
    let directory_failure = |headline: String, path: &Path, code: i32| ResolveFailure {
        headline,
        detail: format!(
            "{INDENT}{}\n\
             {INDENT}use 'crap --here {session_id}' to fork it in the current directory instead.\n",
            path.display()
        ),
        code,
    };

    match err {
        ResolveError::SessionNotFound => ResolveFailure {
            headline: format!("no Claude session found with id '{session_id}'"),
            detail: String::new(),
            code: exit_codes::SESSION_NOT_FOUND,
        },
        ResolveError::NoCwdInSession => ResolveFailure {
            headline: format!("session '{session_id}' has no recorded working directory"),
            detail: String::new(),
            code: exit_codes::NO_CWD_IN_SESSION,
        },
        ResolveError::DirectoryMissing(path) => directory_failure(
            format!("the directory for session '{session_id}' no longer exists:"),
            path,
            exit_codes::DIRECTORY_MISSING,
        ),
        ResolveError::DirectoryUnreadable(path) => directory_failure(
            format!(
                "the directory for session '{session_id}' cannot be entered from this account:"
            ),
            path,
            exit_codes::DIRECTORY_UNREADABLE,
        ),
    }
}

/// Handles the default resume (`crap <id> [--user X]`): locate the session
/// across `roots`, then resume it and exit.
///
/// A hit in the current user's own tree (`is_self`) resumes in place — `cd` to
/// the recorded directory and `claude --resume <id>` — exactly as before. A hit
/// in another user's tree is copied into the current user's tree
/// (`dest_projects_dir`, always our own) and forked at its original directory:
/// the foreign transcript is only ever read, and every write lands under the
/// current user's home. Emits the output the shell function consumes.
fn run_resume(
    roots: &[UserProjects],
    dest_projects_dir: &Path,
    session_id: &str,
    force: bool,
) -> ! {
    if !is_valid_session_id(session_id) {
        eprintln!(
            "{} '{session_id}' is not a valid session id",
            "Error:".red().bold()
        );
        exit(exit_codes::INVALID_SESSION_ID);
    }

    let (path, root) = match find_session_across(roots, session_id) {
        FoundSession::Found { path, root } => (path, root),
        FoundSession::NotFound { skipped } => {
            exit_session_not_found(session_id, roots, &skipped);
        }
    };

    let dir = match session_dir_from_transcript(&path) {
        Ok(dir) => dir,
        // One decision function owns which message, which detail, and which exit
        // code every resolve failure gets; the call site only colorizes the
        // `Error:` prefix and prints what it is handed. Keeping the mapping out of
        // this `match` is what stops the four cases — and in particular the
        // `--here` hint that only two of them carry — from drifting apart.
        Err(err) => {
            let failure = describe_resolve_error(session_id, &err);
            eprintln!("{} {}", "Error:".red().bold(), failure.headline);
            eprint!("{}", failure.detail);
            exit(failure.code);
        }
    };

    if root.is_self {
        // Same-user hit: resume the very session in place, as today.
        abort_if_session_live(session_id, false, force);
        print!("{}", format_output(&dir, session_id));
        exit(0);
    }

    // Cross-user hit: copy the foreign transcript into our own tree at the
    // original directory's project folder, then fork it there. The fork only
    // reads the copy, so a live original is never blocked and never corrupted.
    match prepare_import(dest_projects_dir, &path, &dir, session_id, ImportMode::Copy) {
        Ok(link) => {
            print!(
                "{}",
                format_fork_at_output(session_id, None, link.as_deref(), &dir)
            );
            exit(0);
        }
        Err(err) => {
            eprintln!(
                "{} could not import session '{session_id}': {err}",
                "Error:".red().bold()
            );
            exit(exit_codes::HERE_LINK_ERROR);
        }
    }
}

/// Builds the ordered search roots for a session lookup, honoring `--user`.
///
/// This is the one place that reads the env-coupled home layout —
/// `home.file_name()` (the current account name) and `home.parent()` (the users'
/// parent directory) — so [`self_projects`], [`user_projects`], and
/// [`enumerate_user_projects`] stay pure and tempdir-testable. The default
/// resume, `--here`, and `--status <id>` all resolve their roots through here, so
/// a cross-user source is reachable identically from every form.
///
/// With no `--user`, the current user's own tree is searched first and, only on a
/// miss, every sibling home that has run Claude (self-first, so an id the current
/// user already owns always wins and the common case never reaches another home).
/// With `--user <name>`, only that sibling's tree is searched — the current user's
/// is skipped, which is also how an id is disambiguated on purpose.
///
/// A `--user` naming no account with a `.claude/projects` tree (a typo, or an
/// account that never ran Claude) exits with [`exit_codes::INVALID_USER`] *here*,
/// before any lookup, so the message points at the account rather than at the id.
/// An owner-only tree is a valid target — a real account merely sealed to us — so
/// it resolves and the search's owner-only guidance handles it. A home with no
/// resolvable parent/name keeps today's single-user behavior with no fallback.
fn resolve_search_roots(home: &Path, user: Option<&str>) -> Vec<UserProjects> {
    match user {
        None => match (home.file_name().and_then(|n| n.to_str()), home.parent()) {
            (Some(self_name), Some(users_parent)) => {
                enumerate_user_projects(users_parent, self_name)
            }
            // A home with no resolvable parent/name (unusual): keep today's
            // single-user behavior exactly, with no cross-user fallback.
            _ => vec![self_projects(home)],
        },
        Some(name) => {
            let (Some(self_name), Some(users_parent)) =
                (home.file_name().and_then(|n| n.to_str()), home.parent())
            else {
                eprintln!(
                    "{} could not resolve sibling users from your home {}",
                    "Error:".red().bold(),
                    home.display()
                );
                exit(exit_codes::NO_HOME_DIR);
            };
            match resolve_user_roots(users_parent, name, self_name) {
                UserRoots::Resolved(roots) => roots,
                UserRoots::Invalid { name, available } => {
                    eprint!(
                        "{} {}",
                        "Error:".red().bold(),
                        format_invalid_user(&name, &available)
                    );
                    exit(exit_codes::INVALID_USER);
                }
            }
        }
    }
}

fn main() {
    let cli = Cli::parse();

    if cli.shell_setup {
        match setup_shell_integration() {
            Ok(()) => exit(0),
            Err(e) => {
                eprintln!("{} {e}", "Error:".red().bold());
                exit(exit_codes::SHELL_SETUP_ERROR);
            }
        }
    }

    let Some(home) = dirs::home_dir() else {
        eprintln!(
            "{} could not determine your home directory",
            "Error:".red().bold()
        );
        exit(exit_codes::NO_HOME_DIR);
    };
    let projects_dir = home.join(".claude").join("projects");

    if cli.status {
        match cli.session_id.as_deref() {
            // `--status <id>` gains the same cross-user discovery as resume: build
            // the search roots (self-first, or one sibling for `--user`) and look
            // the id up across them, read-only. The no-id form lists sessions for
            // the current directory, which is inherently the current user's, so it
            // stays self-only and ignores `--user`.
            Some(id) => {
                let roots = resolve_search_roots(&home, cli.user.as_deref());
                run_status(&roots, id, cli.json);
            }
            None => run_dir_status(&projects_dir, cli.json),
        }
    }

    // The clap `required_unless_present_any` guarantees an id is present once we
    // are past the `--shell-setup` and `--status` paths above.
    let session_id = cli
        .session_id
        .expect("session id is required without --shell-setup or --status");

    // Build the search roots (self-first, or one sibling for `--user`); a bad
    // `--user` exits here, before either `run_here` or `run_resume`, so
    // `--here --user <ghost>` rejects for free. Both `--here` and the default
    // resume search the same roots, so a cross-user source is reachable either
    // way.
    let roots = resolve_search_roots(&home, cli.user.as_deref());

    if cli.here {
        run_here(
            &roots,
            &projects_dir,
            &session_id,
            cli.new_session_id.as_deref(),
            cli.force,
        );
    }

    run_resume(&roots, &projects_dir, &session_id, cli.force);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// A representative session id used across tests.
    const SAMPLE_ID: &str = "11111111-2222-3333-4444-555555555555";

    /// Builds a single JSONL line recording `cwd`.
    fn cwd_line(cwd: &str) -> String {
        format!("{}\n", serde_json::json!({ "cwd": cwd }))
    }

    #[test]
    fn default_mode_blocks_a_live_session_unless_forced() {
        // Default resume reuses the session id, so a live original must block.
        assert!(should_block_for_live(false, false));
    }

    #[test]
    fn force_overrides_the_live_block_in_default_mode() {
        assert!(!should_block_for_live(false, true));
    }

    #[test]
    fn resolve_new_session_id_accepts_a_valid_uuid() {
        // A well-formed UUID is passed through so `--here` can pin the fork.
        assert_eq!(resolve_new_session_id(Some(ID_B)), Ok(Some(ID_B)));
    }

    #[test]
    fn resolve_new_session_id_absent_is_none() {
        // No second argument means Claude mints a fresh random id, as before.
        assert_eq!(resolve_new_session_id(None), Ok(None));
    }

    #[test]
    fn resolve_new_session_id_rejects_a_non_uuid() {
        // A malformed id must be caught before it reaches `claude --session-id`.
        assert_eq!(
            resolve_new_session_id(Some("not-a-uuid")),
            Err(InvalidNewSessionId)
        );
    }

    #[test]
    fn here_accepts_session_and_new_id_positionals() {
        let cli = Cli::try_parse_from(["crap", "--here", ID_A, ID_B]).expect("should parse");
        assert!(cli.here);
        assert_eq!(cli.session_id.as_deref(), Some(ID_A));
        assert_eq!(cli.new_session_id.as_deref(), Some(ID_B));
    }

    #[test]
    fn new_session_id_positional_requires_here() {
        // A forked id is meaningless without --here, so clap must reject it.
        assert!(Cli::try_parse_from(["crap", ID_A, ID_B]).is_err());
    }

    #[test]
    fn new_session_id_collides_when_the_id_already_exists() {
        // Pinning a fork to an id that already names a transcript would let it
        // overwrite that conversation, so it must be reported as a collision.
        let projects = tempdir().unwrap();
        let folder = projects.path().join("some-project");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(format!("{ID_B}.jsonl")), "{}\n").unwrap();
        assert!(new_session_id_collides(projects.path(), Some(ID_B)));
    }

    #[test]
    fn new_session_id_does_not_collide_when_unused_or_absent() {
        let projects = tempdir().unwrap();
        fs::create_dir_all(projects.path().join("some-project")).unwrap();
        // An id no transcript uses is free, and "no forced id" never collides.
        assert!(!new_session_id_collides(projects.path(), Some(ID_B)));
        assert!(!new_session_id_collides(projects.path(), None));
    }

    #[test]
    fn here_mode_never_blocks_a_live_session() {
        // `--here` resumes with `--fork-session`: it only reads the original
        // transcript and writes a fresh file, so a live original can never be
        // corrupted by it. A live session must therefore not block `--here`.
        assert!(!should_block_for_live(true, false));
    }

    #[test]
    fn extract_cwd_returns_first_non_null_cwd() {
        let contents = format!(
            "{}{}{}{}",
            "{\"type\":\"summary\"}\n",
            "{\"cwd\":null}\n",
            cwd_line("/Users/tim/code/foo"),
            cwd_line("/Users/tim/code/bar"),
        );
        assert_eq!(
            extract_cwd(&contents).as_deref(),
            Some("/Users/tim/code/foo")
        );
    }

    #[test]
    fn extract_cwd_skips_non_json_and_missing_or_empty() {
        let contents = "not json at all\n{\"foo\":1}\n{\"cwd\":\"\"}\n";
        assert_eq!(extract_cwd(contents), None);
    }

    #[test]
    fn extract_cwd_empty_input_is_none() {
        assert_eq!(extract_cwd(""), None);
    }

    #[test]
    fn extract_cwd_handles_multibyte_paths() {
        let contents = cwd_line("/Users/tim/コード/café");
        assert_eq!(
            extract_cwd(&contents).as_deref(),
            Some("/Users/tim/コード/café")
        );
    }

    /// Builds an `assistant` transcript line with the given content blocks and
    /// `stop_reason` (pass `serde_json::Value::Null` for an interrupted turn).
    fn assistant_line(content: serde_json::Value, stop_reason: serde_json::Value) -> String {
        format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "message": { "stop_reason": stop_reason, "content": content },
            })
        )
    }

    /// Builds a `user` transcript line carrying the given content.
    fn user_line(content: serde_json::Value) -> String {
        format!(
            "{}\n",
            serde_json::json!({ "type": "user", "message": { "content": content } })
        )
    }

    /// A single content block of the given `type`.
    fn block(kind: &str) -> serde_json::Value {
        serde_json::json!([{ "type": kind }])
    }

    #[test]
    fn classify_empty_input_is_empty() {
        assert_eq!(classify_session_state(""), SessionState::Empty);
    }

    #[test]
    fn classify_only_bookkeeping_is_empty() {
        // Lines that are not conversational turns never establish a state.
        let contents = format!(
            "{}{}{}",
            "{\"type\":\"file-history-snapshot\"}\n",
            "{\"type\":\"last-prompt\"}\n",
            "{\"type\":\"ai-title\"}\n",
        );
        assert_eq!(classify_session_state(&contents), SessionState::Empty);
    }

    #[test]
    fn classify_assistant_end_turn_waits_for_user() {
        let contents = assistant_line(block("text"), serde_json::json!("end_turn"));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_assistant_stop_sequence_waits_for_user() {
        let contents = assistant_line(block("text"), serde_json::json!("stop_sequence"));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_assistant_pending_tool_use_is_busy() {
        let contents = assistant_line(block("tool_use"), serde_json::json!("tool_use"));
        assert_eq!(classify_session_state(&contents), SessionState::Busy);
    }

    #[test]
    fn classify_uses_stop_reason_over_block_type() {
        // `stop_reason` is replicated onto every line of a message, so a line
        // holding only a `thinking` block still carries the turn's real
        // `stop_reason`. A tool_use stop means a tool call is pending → busy,
        // even though this line's block is not itself a tool_use.
        let contents = assistant_line(block("thinking"), serde_json::json!("tool_use"));
        assert_eq!(classify_session_state(&contents), SessionState::Busy);
    }

    #[test]
    fn classify_interrupted_text_turn_waits_for_user() {
        // A null stop_reason means the turn was cut off mid-stream; with no
        // pending tool call (last block is text) the user is still up next.
        let contents = assistant_line(block("text"), serde_json::Value::Null);
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_interrupted_tool_use_is_busy() {
        // Interrupted, but the last block is a tool_use awaiting a result → busy.
        let contents = assistant_line(block("tool_use"), serde_json::Value::Null);
        assert_eq!(classify_session_state(&contents), SessionState::Busy);
    }

    #[test]
    fn classify_tool_result_is_busy() {
        // A tool result was just delivered; the assistant has yet to respond.
        let contents = format!(
            "{}{}",
            assistant_line(block("tool_use"), serde_json::json!("tool_use")),
            user_line(block("tool_result")),
        );
        assert_eq!(classify_session_state(&contents), SessionState::Busy);
    }

    #[test]
    fn classify_user_prompt_string_awaits_assistant() {
        // A real user prompt (string content) with no assistant reply yet.
        let contents = user_line(serde_json::json!("please continue"));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::AwaitingAssistant
        );
    }

    #[test]
    fn classify_user_prompt_text_block_awaits_assistant() {
        let contents = user_line(serde_json::json!([{ "type": "text", "text": "hi" }]));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::AwaitingAssistant
        );
    }

    #[test]
    fn classify_ignores_meta_user_entries() {
        // An injected `isMeta` user entry (system reminder, command output) is
        // not the user speaking, so the prior assistant end-of-turn wins.
        let mut contents = assistant_line(block("text"), serde_json::json!("end_turn"));
        contents.push_str(&format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "isMeta": true,
                "message": { "content": [{ "type": "text", "text": "<reminder>" }] },
            })
        ));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_ignores_sidechain_turns() {
        // Subagent (`isSidechain`) turns are interleaved in the same file but
        // belong to a different thread, so they never set the main-thread state.
        let mut contents = assistant_line(block("text"), serde_json::json!("end_turn"));
        contents.push_str(&format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "isSidechain": true,
                "message": { "stop_reason": "tool_use", "content": [{ "type": "tool_use" }] },
            })
        ));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_ignores_trailing_bookkeeping() {
        // Bookkeeping lines are appended after the conversation; they must not
        // override the last real turn's state.
        let mut contents = assistant_line(block("text"), serde_json::json!("end_turn"));
        contents.push_str("{\"type\":\"file-history-snapshot\"}\n");
        contents.push_str("{\"type\":\"last-prompt\"}\n");
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_follows_a_realistic_sequence() {
        // user prompt → assistant tool_use → tool_result → assistant end_turn:
        // Claude has finished and is waiting for the user.
        let contents = format!(
            "{}{}{}{}",
            user_line(serde_json::json!("do the thing")),
            assistant_line(block("tool_use"), serde_json::json!("tool_use")),
            user_line(block("tool_result")),
            assistant_line(block("text"), serde_json::json!("end_turn")),
        );
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn session_state_tokens_are_stable() {
        assert_eq!(SessionState::Empty.as_token(), "empty");
        assert_eq!(SessionState::WaitingForUser.as_token(), "waiting-for-user");
        assert_eq!(SessionState::Busy.as_token(), "busy");
        assert_eq!(
            SessionState::AwaitingAssistant.as_token(),
            "awaiting-assistant"
        );
    }

    #[test]
    fn valid_session_id_accepts_uuid() {
        assert!(is_valid_session_id("4733ee2a-1ad6-4619-a01a-11840b8e1901"));
    }

    #[test]
    fn valid_session_id_rejects_traversal_and_separators() {
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id(".."));
        assert!(!is_valid_session_id("../etc/passwd"));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id("a\\b"));
    }

    #[test]
    fn valid_session_id_requires_uuid_shape() {
        // Anything that isn't a canonical 8-4-4-4-12 UUID is rejected, so a
        // typo'd id fails fast and shell metacharacters never reach the binary.
        assert!(!is_valid_session_id("not-a-uuid"));
        assert!(!is_valid_session_id("4733ee2a-1ad6-4619-a01a-11840b8e190")); // too short
        assert!(!is_valid_session_id(
            "4733ee2a-1ad6-4619-a01a-11840b8e19011"
        )); // too long
        assert!(!is_valid_session_id("4733ee2a1ad64619a01a11840b8e1901")); // no hyphens
        assert!(!is_valid_session_id("4733ee2g-1ad6-4619-a01a-11840b8e1901")); // 'g' not hex
        assert!(!is_valid_session_id(
            "4733ee2a-1ad6-4619-a01a-11840b8e1901 ; rm -rf ~"
        ));
        assert!(!is_valid_session_id("4733ee2a 1ad6 4619 a01a 11840b8e1901")); // spaces
    }

    #[test]
    fn valid_session_id_accepts_uppercase_hex() {
        // Hex is case-insensitive even though Claude writes lowercase ids.
        assert!(is_valid_session_id("4733EE2A-1AD6-4619-A01A-11840B8E1901"));
    }

    #[test]
    fn encode_project_dir_matches_claude_folder_naming() {
        // Plain path: every separator becomes a dash, the leading slash too.
        assert_eq!(
            encode_project_dir(Path::new("/Volumes/SamsungSSDs/code/claude-vibecoding")),
            "-Volumes-SamsungSSDs-code-claude-vibecoding"
        );
        // A `/.` run (hidden directory) collapses to a double dash, and the
        // hyphen already in the final component is preserved verbatim. This is
        // a real example observed under ~/.claude/projects.
        assert_eq!(
            encode_project_dir(Path::new("/Users/timmattison/.config/qbittorrent-vpn")),
            "-Users-timmattison--config-qbittorrent-vpn"
        );
        // Digits survive; only non-alphanumerics are rewritten.
        assert_eq!(
            encode_project_dir(Path::new("/a/day-3-planning")),
            "-a-day-3-planning"
        );
        // Non-ASCII characters each collapse to a single dash.
        assert_eq!(encode_project_dir(Path::new("/x/café")), "-x-caf-");
    }

    #[test]
    fn prepare_import_symlink_creates_symlink_in_target_project_folder() {
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        // A target dir whose encoded project folder does not exist yet.
        let target = Path::new("/Volumes/x/here-cwd");
        let link = prepare_import(projects, &original, target, SAMPLE_ID, ImportMode::Symlink)
            .expect("should succeed")
            .expect("a symlink should be created");

        assert_eq!(
            link,
            projects
                .join("-Volumes-x-here-cwd")
                .join(format!("{SAMPLE_ID}.jsonl"))
        );
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), original);
    }

    #[test]
    fn prepare_import_symlink_returns_none_when_already_in_session_folder() {
        let dir = tempdir().unwrap();
        let projects = dir.path();
        // encode_project_dir("/orig") == "-orig", so target resolves to the
        // folder the session already lives in: no symlink is needed.
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let result = prepare_import(
            projects,
            &original,
            Path::new("/orig"),
            SAMPLE_ID,
            ImportMode::Symlink,
        )
        .expect("ok");
        assert_eq!(result, None);
        assert!(original.is_file(), "original must be left untouched");
    }

    #[cfg(unix)]
    #[test]
    fn prepare_import_symlink_leaves_an_existing_link_in_place() {
        // A symlink dropped by an earlier `--here` already makes the session
        // resolvable here, so a repeat returns `None` (nothing to clean up) and
        // does not disturb the existing link.
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let target = Path::new("/Volumes/x/here");
        let folder = projects.join("-Volumes-x-here");
        fs::create_dir_all(&folder).unwrap();
        let link = folder.join(format!("{SAMPLE_ID}.jsonl"));
        std::os::unix::fs::symlink(&original, &link).unwrap();

        assert_eq!(
            prepare_import(projects, &original, target, SAMPLE_ID, ImportMode::Symlink)
                .expect("ok"),
            None
        );
        assert_eq!(fs::read_link(&link).unwrap(), original);
    }

    #[test]
    fn prepare_import_symlink_returns_none_when_a_real_file_is_already_present() {
        // A session id is a UUID, so a real file at the target name can only be
        // this very session living here already: resolve it in place, untouched.
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let target = Path::new("/Volumes/x/here");
        let folder = projects.join("-Volumes-x-here");
        fs::create_dir_all(&folder).unwrap();
        let real = folder.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&real, "a real session").unwrap();

        assert_eq!(
            prepare_import(projects, &original, target, SAMPLE_ID, ImportMode::Symlink)
                .expect("ok"),
            None
        );
        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            "a real session",
            "the real file must be left untouched"
        );
    }

    #[test]
    fn prepare_import_copy_snapshots_source_and_is_idempotent() {
        // Cross-user import copies the transcript into the current user's tree:
        // a self-contained regular file (not a symlink) with identical bytes,
        // and a repeat import is a no-op.
        let dir = tempdir().unwrap();
        let projects = dir.path().join("dest/.claude/projects");
        fs::create_dir_all(&projects).unwrap();
        // The source lives under a *different* tree (another user's home).
        let src_dir = dir.path().join("other/.claude/projects/-orig");
        fs::create_dir_all(&src_dir).unwrap();
        let source = src_dir.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&source, "{\"cwd\":\"/work\"}\n").unwrap();

        let target = Path::new("/Volumes/x/work");
        let copy = prepare_import(&projects, &source, target, SAMPLE_ID, ImportMode::Copy)
            .expect("should succeed")
            .expect("a copy should be created");

        assert_eq!(
            copy,
            projects
                .join("-Volumes-x-work")
                .join(format!("{SAMPLE_ID}.jsonl"))
        );
        // A real file, not a symlink pointing back into the other user's tree.
        assert!(
            !fs::symlink_metadata(&copy)
                .unwrap()
                .file_type()
                .is_symlink(),
            "cross-user import must be a copy, not a symlink"
        );
        assert_eq!(
            fs::read_to_string(&copy).unwrap(),
            "{\"cwd\":\"/work\"}\n",
            "the copy must snapshot the source bytes"
        );

        // A second import finds the file already present and is a no-op.
        assert_eq!(
            prepare_import(&projects, &source, target, SAMPLE_ID, ImportMode::Copy).expect("ok"),
            None
        );
    }

    #[test]
    fn find_session_file_locates_jsonl_in_project_subdir() {
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let proj = projects.join("-Users-tim-code-foo");
        fs::create_dir_all(&proj).unwrap();
        let file = proj.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&file, "{}\n").unwrap();

        assert_eq!(find_session_file(projects, SAMPLE_ID), Some(file));
    }

    #[test]
    fn find_session_file_returns_none_when_absent() {
        let dir = tempdir().unwrap();
        assert_eq!(find_session_file(dir.path(), SAMPLE_ID), None);
    }

    #[test]
    fn user_projects_sets_is_self_for_self_and_other() {
        let parent = Path::new("/Users");
        // A `--user` naming the current account is a same-user hit.
        let me = user_projects(parent, "timmattison", "timmattison");
        assert_eq!(me.user, "timmattison");
        assert_eq!(
            me.projects_dir,
            Path::new("/Users/timmattison/.claude/projects")
        );
        assert!(me.is_self);

        // A different account is a cross-user root.
        let other = user_projects(parent, "scyloswork", "timmattison");
        assert_eq!(other.user, "scyloswork");
        assert_eq!(
            other.projects_dir,
            Path::new("/Users/scyloswork/.claude/projects")
        );
        assert!(!other.is_self);
    }

    #[test]
    fn self_projects_marks_the_current_users_tree() {
        let home = Path::new("/Users/timmattison");
        let mine = self_projects(home);
        assert_eq!(mine.user, "timmattison");
        assert_eq!(
            mine.projects_dir,
            Path::new("/Users/timmattison/.claude/projects")
        );
        assert!(mine.is_self);
    }

    #[test]
    fn find_session_across_finds_id_in_a_single_root() {
        let dir = tempdir().unwrap();
        let projects = dir.path().join("home/.claude/projects");
        let proj = projects.join("-some-proj");
        fs::create_dir_all(&proj).unwrap();
        let file = proj.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&file, "{}\n").unwrap();

        let root = UserProjects {
            user: "me".to_string(),
            projects_dir: projects,
            is_self: true,
        };
        match find_session_across(std::slice::from_ref(&root), SAMPLE_ID) {
            FoundSession::Found { path, root: found } => {
                assert_eq!(path, file);
                assert_eq!(found.user, "me");
                assert!(found.is_self);
            }
            FoundSession::NotFound { .. } => panic!("expected the id to be found"),
        }
    }

    #[test]
    fn find_session_across_reports_not_found_when_absent() {
        let dir = tempdir().unwrap();
        let projects = dir.path().join("home/.claude/projects");
        fs::create_dir_all(&projects).unwrap();
        let root = UserProjects {
            user: "me".to_string(),
            projects_dir: projects,
            is_self: true,
        };
        assert!(matches!(
            find_session_across(&[root], SAMPLE_ID),
            FoundSession::NotFound { .. }
        ));
    }

    #[test]
    fn find_session_across_prefers_self_when_id_exists_in_two_roots() {
        // The same id lives under both the current user's tree and a foreign
        // one. With the roots ordered self-first, the search must short-circuit
        // on the self copy and never resolve to the foreign transcript — the
        // guarantee that a UUID present in two trees always resumes as our own.
        let tmp = tempdir().unwrap();
        let self_projects = tmp.path().join("me/.claude/projects");
        let other_projects = tmp.path().join("other/.claude/projects");
        let self_proj = self_projects.join("-proj");
        let other_proj = other_projects.join("-proj");
        fs::create_dir_all(&self_proj).unwrap();
        fs::create_dir_all(&other_proj).unwrap();
        let self_file = self_proj.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&self_file, "{}\n").unwrap();
        fs::write(other_proj.join(format!("{SAMPLE_ID}.jsonl")), "{}\n").unwrap();

        let roots = vec![
            UserProjects {
                user: "me".to_string(),
                projects_dir: self_projects,
                is_self: true,
            },
            UserProjects {
                user: "other".to_string(),
                projects_dir: other_projects,
                is_self: false,
            },
        ];
        match find_session_across(&roots, SAMPLE_ID) {
            FoundSession::Found { path, root } => {
                assert_eq!(path, self_file, "must resolve to the self copy");
                assert!(root.is_self);
                assert_eq!(root.user, "me");
            }
            FoundSession::NotFound { .. } => panic!("expected the id to be found in self"),
        }
    }

    /// Makes `dir` unreadable (`0o000`) and reports whether the invoking user is
    /// genuinely locked out of it.
    ///
    /// Returns `false` when the caller can still see inside. Root ignores the
    /// permission bits entirely, so under `sudo` — or in a container that runs
    /// every process as uid 0 — an owner-only directory is perfectly readable
    /// and the behavior these tests describe is simply not observable; a test
    /// that asserted it anyway would fail for a reason that has nothing to do
    /// with the code. Establishing that without adding a `libc`/`nix` dependency
    /// just to call `geteuid` means asking the filesystem the same question the
    /// scan will ask: probe a path inside the locked directory and see whether
    /// the answer really is `PermissionDenied`.
    #[cfg(unix)]
    fn lock_dir(dir: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).unwrap();
        std::fs::metadata(dir.join("probe")).err().map(|e| e.kind())
            == Some(std::io::ErrorKind::PermissionDenied)
    }

    /// Restores `dir` to a normal readable mode after [`lock_dir`].
    ///
    /// Every caller must do this *before* running any assertion that can panic:
    /// `TempDir`'s `Drop` is a recursive delete, and a `0o000` directory defeats
    /// it — so an unrestored lock leaks the tempdir and buries the real
    /// assertion failure under a cleanup error.
    #[cfg(unix)]
    fn unlock_dir(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn find_session_across_records_an_owner_only_subdir_as_skipped() {
        // A `0o700` project dir in someone else's home is opaque to us: the scan
        // cannot rule the session out, so stepping over it silently would turn
        // "I was not allowed to look" into a confident "it does not exist".
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("other/.claude/projects");
        let locked = projects.join("-locked");
        fs::create_dir_all(&locked).unwrap();
        if !lock_dir(&locked) {
            unlock_dir(&locked);
            return;
        }

        let root = UserProjects {
            user: "other".to_string(),
            projects_dir: projects,
            is_self: false,
        };
        // Capture first, unlock second, assert last — a panic before the unlock
        // would strand an undeletable directory inside the tempdir.
        let found = find_session_across(std::slice::from_ref(&root), SAMPLE_ID);
        unlock_dir(&locked);

        match found {
            FoundSession::NotFound { skipped } => assert_eq!(
                skipped,
                vec![SkippedDir {
                    user: "other".to_string(),
                    dir: locked,
                }],
                "the unreadable dir must be recorded against the user who owns it"
            ),
            FoundSession::Found { .. } => panic!("the id exists nowhere; it cannot be found"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn find_session_across_still_matches_a_readable_dir_when_another_is_owner_only() {
        // Recording a skip must never cost us a hit: one unreadable neighbour in
        // the same tree cannot be allowed to make a session that is right there,
        // readable, unfindable.
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("other/.claude/projects");
        let readable = projects.join("-readable");
        let locked = projects.join("-locked");
        fs::create_dir_all(&readable).unwrap();
        fs::create_dir_all(&locked).unwrap();
        let file = readable.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&file, "{}\n").unwrap();
        if !lock_dir(&locked) {
            unlock_dir(&locked);
            return;
        }

        let root = UserProjects {
            user: "other".to_string(),
            projects_dir: projects,
            is_self: false,
        };
        let found = find_session_across(std::slice::from_ref(&root), SAMPLE_ID);
        unlock_dir(&locked);

        match found {
            FoundSession::Found { path, root: found } => {
                assert_eq!(path, file);
                assert_eq!(found.user, "other");
            }
            FoundSession::NotFound { .. } => {
                panic!("an unreadable neighbour must not hide a readable session")
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn find_session_across_records_an_unreadable_projects_root_as_skipped() {
        // The whole `~/.claude/projects` tree can be the thing we cannot open,
        // and it is the same class of miss as a single opaque project dir: one
        // skip, naming the root itself, so the user learns the tree exists and
        // is closed rather than being told the id is nowhere on the machine.
        let tmp = tempdir().unwrap();
        let projects = tmp.path().join("other/.claude/projects");
        fs::create_dir_all(&projects).unwrap();
        if !lock_dir(&projects) {
            unlock_dir(&projects);
            return;
        }

        let root = UserProjects {
            user: "other".to_string(),
            projects_dir: projects.clone(),
            is_self: false,
        };
        let found = find_session_across(std::slice::from_ref(&root), SAMPLE_ID);
        unlock_dir(&projects);

        match found {
            FoundSession::NotFound { skipped } => assert_eq!(
                skipped,
                vec![SkippedDir {
                    user: "other".to_string(),
                    dir: projects,
                }],
                "an unlistable root is one skip, named by the root itself"
            ),
            FoundSession::Found { .. } => panic!("nothing is readable here; nothing can be found"),
        }
    }

    #[test]
    fn enumerate_user_projects_lists_self_first_then_siblings_with_projects() {
        // Layout under a fake `/Users` parent:
        //   me    -> has .claude/projects (the current user)
        //   alice -> has .claude/projects (a search root)
        //   bob   -> has .claude/projects (a search root)
        //   carol -> no .claude/projects  (never ran Claude -> excluded)
        //   notes.txt -> a regular file   (not a home at all -> excluded)
        let tmp = tempdir().unwrap();
        let parent = tmp.path();
        for user in ["me", "alice", "bob"] {
            fs::create_dir_all(parent.join(user).join(".claude").join("projects")).unwrap();
        }
        fs::create_dir_all(parent.join("carol")).unwrap();
        fs::write(parent.join("notes.txt"), "x").unwrap();

        let roots = enumerate_user_projects(parent, "me");
        let users: Vec<&str> = roots.iter().map(|r| r.user.as_str()).collect();
        // Self is first; the other roots follow sorted by name; carol (no
        // projects dir) and the regular file are excluded.
        assert_eq!(users, ["me", "alice", "bob"]);
        // Only the self entry is marked is_self, and it points at the current
        // user's own projects dir.
        assert!(roots[0].is_self);
        assert!(roots[1..].iter().all(|r| !r.is_self));
        assert_eq!(
            roots[0].projects_dir,
            parent.join("me").join(".claude").join("projects")
        );
    }

    #[test]
    fn enumerate_user_projects_always_includes_self_even_without_a_projects_dir() {
        // The current user is always root zero, even if they have not run Claude
        // yet, so today's self-first fast path is preserved verbatim. A sibling
        // that has run Claude is still discovered.
        let tmp = tempdir().unwrap();
        let parent = tmp.path();
        fs::create_dir_all(parent.join("me")).unwrap(); // no .claude/projects
        fs::create_dir_all(parent.join("alice").join(".claude").join("projects")).unwrap();

        let roots = enumerate_user_projects(parent, "me");
        assert_eq!(roots[0].user, "me");
        assert!(roots[0].is_self);
        assert_eq!(roots.iter().filter(|r| r.is_self).count(), 1);
        assert!(roots.iter().any(|r| r.user == "alice" && !r.is_self));
    }

    #[cfg(unix)]
    #[test]
    fn enumerate_user_projects_drops_an_aliased_home_pointing_back_at_self() {
        // A symlinked home alias (`me-alias -> me`) is the same physical tree as
        // the current user's own home, so it must not be searched a second time
        // — and certainly not tagged as another user's tree.
        let tmp = tempdir().unwrap();
        let parent = tmp.path();
        fs::create_dir_all(parent.join("me").join(".claude").join("projects")).unwrap();
        std::os::unix::fs::symlink(parent.join("me"), parent.join("me-alias")).unwrap();

        let roots = enumerate_user_projects(parent, "me");
        let users: Vec<&str> = roots.iter().map(|r| r.user.as_str()).collect();
        // Only the real self root survives: the alias resolves to the same
        // canonical projects dir and is deduped away.
        assert_eq!(users, ["me"]);
        assert!(roots[0].is_self);
        assert_eq!(roots.iter().filter(|r| r.is_self).count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn enumerate_user_projects_drops_sibling_aliases_of_one_another() {
        // Two names for one sibling home (`zoe -> alice`) are one search root,
        // not two. The alphabetically first name wins, so the survivor is stable
        // no matter how the filesystem enumerated the parent.
        let tmp = tempdir().unwrap();
        let parent = tmp.path();
        fs::create_dir_all(parent.join("me")).unwrap(); // no .claude/projects
        fs::create_dir_all(parent.join("alice").join(".claude").join("projects")).unwrap();
        std::os::unix::fs::symlink(parent.join("alice"), parent.join("zoe")).unwrap();

        let roots = enumerate_user_projects(parent, "me");
        let users: Vec<&str> = roots.iter().map(|r| r.user.as_str()).collect();
        assert_eq!(users, ["me", "alice"]);
        assert!(roots[0].is_self);
        assert!(roots[1..].iter().all(|r| !r.is_self));
    }

    #[test]
    fn resolve_user_roots_resolves_a_real_sibling_to_a_single_root() {
        // A sibling with a real projects tree resolves to exactly one root — that
        // sibling's, tagged as another user's — because `--user` searches only the
        // named tree and never the current user's.
        let tmp = tempdir().unwrap();
        let parent = tmp.path();
        let other_projects = parent.join("other").join(".claude").join("projects");
        fs::create_dir_all(&other_projects).unwrap();

        match resolve_user_roots(parent, "other", "me") {
            UserRoots::Resolved(roots) => {
                assert_eq!(roots.len(), 1, "--user searches only the named tree");
                assert_eq!(roots[0].user, "other");
                assert_eq!(roots[0].projects_dir, other_projects);
                assert!(!roots[0].is_self);
            }
            UserRoots::Invalid { .. } => panic!("a real sibling tree must resolve"),
        }
    }

    #[test]
    fn resolve_user_roots_resolves_the_current_users_own_name_as_self() {
        // `--user <self>` names the current account: a valid same-user target that
        // resolves in place with `is_self` set — the pure counterpart of the
        // `user_flag_self_resumes_in_place` integration test.
        let tmp = tempdir().unwrap();
        let parent = tmp.path();
        fs::create_dir_all(parent.join("me").join(".claude").join("projects")).unwrap();

        match resolve_user_roots(parent, "me", "me") {
            UserRoots::Resolved(roots) => {
                assert_eq!(roots.len(), 1);
                assert_eq!(roots[0].user, "me");
                assert!(roots[0].is_self, "--user <self> is a same-user hit");
            }
            UserRoots::Invalid { .. } => panic!("the current user's own tree must resolve"),
        }
    }

    #[test]
    fn resolve_user_roots_invalid_lists_available_accounts_for_a_ghost() {
        // A name with no home at all under the parent names no projects tree: the
        // result must be Invalid and carry the accounts that DO have one — self
        // (always) plus any sibling with a `.claude/projects` dir — in
        // enumerate_user_projects order (self first, then siblings by name).
        let tmp = tempdir().unwrap();
        let parent = tmp.path();
        fs::create_dir_all(parent.join("me").join(".claude").join("projects")).unwrap();
        fs::create_dir_all(parent.join("alice").join(".claude").join("projects")).unwrap();

        match resolve_user_roots(parent, "ghost", "me") {
            UserRoots::Invalid { name, available } => {
                assert_eq!(
                    name, "ghost",
                    "carries the bad --user value for the message"
                );
                assert_eq!(available, vec!["me".to_string(), "alice".to_string()]);
            }
            UserRoots::Resolved(_) => panic!("a ghost account has no tree to resolve"),
        }
    }

    #[test]
    fn resolve_user_roots_invalid_when_a_real_home_never_ran_claude() {
        // The account exists but has no `.claude/projects` tree, so there is
        // nothing for `--user` to search: invalid, exactly like a ghost account.
        let tmp = tempdir().unwrap();
        let parent = tmp.path();
        fs::create_dir_all(parent.join("me").join(".claude").join("projects")).unwrap();
        fs::create_dir_all(parent.join("bob")).unwrap(); // a home, but no projects tree

        assert!(matches!(
            resolve_user_roots(parent, "bob", "me"),
            UserRoots::Invalid { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_user_roots_treats_an_owner_only_tree_as_a_valid_target() {
        // The account is real and has run Claude, but its `.claude` is opaque to
        // us, so we cannot even `stat` the projects dir. That is "locked out", not
        // "absent": it must resolve (letting the normal search's owner-only
        // guidance handle the refusal), never a misleading INVALID_USER.
        let tmp = tempdir().unwrap();
        let parent = tmp.path();
        let claude = parent.join("other").join(".claude");
        fs::create_dir_all(claude.join("projects")).unwrap();
        // Sealing `.claude` makes `stat`ing `.claude/projects` fail with
        // PermissionDenied — the exact refusal resolve_user_roots must treat as a
        // valid, real account rather than an absent one.
        if !lock_dir(&claude) {
            unlock_dir(&claude);
            return;
        }
        // Capture first, unlock second, assert last — a panic before the unlock
        // would strand an undeletable directory inside the tempdir.
        let resolved = resolve_user_roots(parent, "other", "me");
        unlock_dir(&claude);

        match resolved {
            UserRoots::Resolved(roots) => {
                assert_eq!(roots.len(), 1);
                assert_eq!(roots[0].user, "other");
                assert!(!roots[0].is_self);
            }
            UserRoots::Invalid { .. } => {
                panic!("an owner-only tree is a real account, not an invalid user")
            }
        }
    }

    #[test]
    fn format_invalid_user_names_the_bad_account_and_lists_the_available_ones() {
        let msg = format_invalid_user("ghost", &["me".to_string(), "alice".to_string()]);
        assert!(msg.contains("ghost"), "must name the bad account: {msg}");
        // Each available account appears on its own line under the list heading.
        assert!(
            msg.lines().any(|l| l.trim() == "me"),
            "must list 'me' on its own line: {msg}"
        );
        assert!(
            msg.lines().any(|l| l.trim() == "alice"),
            "must list 'alice' on its own line: {msg}"
        );
        // Never a sudo remedy: this is the wrong-account case, not a sealed tree.
        assert!(
            !msg.contains("sudo"),
            "no sudo remedy on the invalid-user path: {msg}"
        );
    }

    #[test]
    fn format_invalid_user_empty_available_reads_sensibly() {
        // No account on the machine has a projects tree: there is nothing to list,
        // so say so plainly rather than printing a heading over an empty list.
        let msg = format_invalid_user("ghost", &[]);
        assert!(msg.contains("ghost"), "still names the bad account: {msg}");
        assert!(
            msg.contains("no account on this machine"),
            "must say plainly that nothing is resumable: {msg}"
        );
        assert!(
            !msg.contains("accounts you can resume from"),
            "no list heading when there is nothing to list: {msg}"
        );
    }

    /// A search root for the not-found message tests. Only `projects_dir` and
    /// how many roots there are can affect the message, so the paths are plain
    /// literals rather than tempdirs.
    fn search_root(user: &str, is_self: bool) -> UserProjects {
        UserProjects {
            user: user.to_string(),
            projects_dir: Path::new("/Users").join(user).join(".claude/projects"),
            is_self,
        }
    }

    #[test]
    fn format_searched_roots_names_the_only_root_searched() {
        // A single root is both today's self-only case and the `--user <name>`
        // case: one plain `looked under` line, and no summary to append.
        let roots = [search_root("me", true)];
        assert_eq!(
            format_searched_roots(&roots),
            format!("       looked under {}\n", roots[0].projects_dir.display())
        );
    }

    #[test]
    fn format_searched_roots_summarizes_a_single_extra_account() {
        // Two roots: the first is named, the second is counted — and the count
        // is singular.
        let roots = [search_root("me", true), search_root("alice", false)];
        assert_eq!(
            format_searched_roots(&roots),
            format!(
                "       looked under {}\n       …and 1 other account on this machine\n",
                roots[0].projects_dir.display()
            )
        );
    }

    #[test]
    fn format_searched_roots_summarizes_extra_accounts_without_naming_them() {
        // The auto-fallback can search every home on a shared machine, so the
        // remainder is a plural count only: two detail lines no matter how many
        // accounts exist, and no sibling account name is ever disclosed.
        let roots = [
            search_root("me", true),
            search_root("alice", false),
            search_root("bob", false),
            search_root("carol", false),
        ];
        let out = format_searched_roots(&roots);
        assert_eq!(
            out,
            format!(
                "       looked under {}\n       …and 3 other accounts on this machine\n",
                roots[0].projects_dir.display()
            )
        );
        assert_eq!(out.lines().count(), 2, "at most two detail lines: {out}");
        for name in ["alice", "bob", "carol"] {
            assert!(!out.contains(name), "must not name {name}: {out}");
        }
    }

    #[test]
    fn format_searched_roots_is_empty_without_any_roots() {
        // Unreachable in practice (the current user is always root zero), but a
        // total function keeps a stray indent off the not-found output.
        assert_eq!(format_searched_roots(&[]), "");
    }

    /// A project directory under `user`'s tree that the scan was refused. Like
    /// [`search_root`], plain literals: only the account name, the count, and the
    /// owning root's path can affect the guidance.
    fn skipped_dir(user: &str, folder: &str) -> SkippedDir {
        SkippedDir {
            user: user.to_string(),
            dir: Path::new("/Users")
                .join(user)
                .join(".claude/projects")
                .join(folder),
        }
    }

    #[test]
    fn format_owner_only_guidance_is_empty_when_nothing_was_skipped() {
        // A clean miss — everything was readable and the id simply is not here —
        // earns no guidance, so the not-found path can print the block
        // unconditionally instead of branching on the shape of the miss.
        let roots = [search_root("me", true)];
        assert_eq!(
            format_owner_only_guidance(SAMPLE_ID, &roots, &[], Path::new("/work")),
            ""
        );
    }

    #[test]
    fn format_owner_only_guidance_counts_a_single_dir_in_the_singular() {
        // One skipped directory reads as one, not as "1 project dirs ... were".
        let roots = [search_root("me", true), search_root("alice", false)];
        let skipped = [skipped_dir("alice", "-proj")];
        let out = format_owner_only_guidance(SAMPLE_ID, &roots, &skipped, Path::new("/work"));
        assert!(
            out.contains(
                "       1 project dir under user 'alice' is owner-only and was skipped.\n"
            ),
            "singular count line: {out}"
        );
        assert!(!out.contains("project dirs"), "no plural noun: {out}");
    }

    #[test]
    fn format_owner_only_guidance_counts_several_dirs_in_the_plural() {
        // Three skipped directories under one account: one line, pluralized.
        let roots = [search_root("me", true), search_root("alice", false)];
        let skipped = [
            skipped_dir("alice", "-one"),
            skipped_dir("alice", "-two"),
            skipped_dir("alice", "-three"),
        ];
        let out = format_owner_only_guidance(SAMPLE_ID, &roots, &skipped, Path::new("/work"));
        assert!(
            out.contains(
                "       3 project dirs under user 'alice' are owner-only and were skipped.\n"
            ),
            "plural count line: {out}"
        );
        assert_eq!(
            out.lines()
                .filter(|l| l.contains("owner-only and were skipped"))
                .count(),
            1,
            "one count line per account, not per directory: {out}"
        );
    }

    #[test]
    fn format_owner_only_guidance_counts_each_account_in_scan_order() {
        // Two accounts were opaque, and `alice`'s directories bracket `bob`'s in
        // the scan. Each account gets exactly one count line, in first-seen
        // order, and the single worked remedy is keyed on the first of them —
        // naming that account explicitly, so the reader knows which count the
        // commands belong to.
        let roots = [
            search_root("me", true),
            search_root("alice", false),
            search_root("bob", false),
        ];
        let skipped = [
            skipped_dir("alice", "-one"),
            skipped_dir("bob", "-x"),
            skipped_dir("alice", "-two"),
        ];
        let out = format_owner_only_guidance(SAMPLE_ID, &roots, &skipped, Path::new("/work"));
        let alice = out
            .find("2 project dirs under user 'alice' are owner-only and were skipped.")
            .expect("a count line for alice");
        let bob = out
            .find("1 project dir under user 'bob' is owner-only and was skipped.")
            .expect("a count line for bob");
        assert!(alice < bob, "accounts are listed in scan order: {out}");
        assert!(
            out.contains("for user 'alice',"),
            "the remedy must say which account it is for: {out}"
        );
        // Exactly one remedy, and every command in it is `alice`'s.
        assert_eq!(
            out.lines().filter(|l| l.contains("sudo -u ")).count(),
            2,
            "one remedy: a `find` and a `cat`: {out}"
        );
        assert!(!out.contains("sudo -u bob"), "not keyed on bob too: {out}");
    }

    #[test]
    fn format_owner_only_guidance_remedy_reads_that_tree_and_writes_this_directory() {
        // The `find` searches the owning account's whole tree — which project
        // folder holds the session is exactly what could not be seen — and the
        // copy lands in OUR project folder for the current directory, which is
        // what the trailing `crap --here` then resumes.
        let roots = [search_root("me", true), search_root("alice", false)];
        let skipped = [skipped_dir("alice", "-proj")];
        let out =
            format_owner_only_guidance(SAMPLE_ID, &roots, &skipped, Path::new("/Volumes/code/foo"));
        assert!(
            out.contains(&format!(
                "SRC=$(sudo -u alice find /Users/alice/.claude/projects -name '{SAMPLE_ID}.jsonl')"
            )),
            "the find must cover alice's whole tree: {out}"
        );
        assert!(
            out.contains("mkdir -p ~/.claude/projects/-Volumes-code-foo\n"),
            "the destination is this directory's project folder: {out}"
        );
        assert!(
            out.contains(&format!(
                "sudo -u alice cat \"$SRC\" > ~/.claude/projects/-Volumes-code-foo/{SAMPLE_ID}.jsonl"
            )),
            "the copy must land in our own tree: {out}"
        );
        assert!(
            out.contains(&format!("crap --here {SAMPLE_ID}\n")),
            "the remedy must end by resuming the copy: {out}"
        );
    }

    #[test]
    fn format_owner_only_guidance_falls_back_when_the_owner_has_no_root() {
        // Every skipped directory came from a root, so the lookup cannot really
        // miss — but the function stays total: with no matching root it narrows
        // the `find` to the skipped directory itself rather than panicking on a
        // message whose only job is to be helpful.
        let roots = [search_root("me", true)];
        let skipped = [skipped_dir("ghost", "-proj")];
        let out = format_owner_only_guidance(SAMPLE_ID, &roots, &skipped, Path::new("/work"));
        assert!(
            out.contains("SRC=$(sudo -u ghost find /Users/ghost/.claude/projects/-proj -name '"),
            "the find falls back to the directory that was refused: {out}"
        );
    }

    #[test]
    fn session_dir_from_transcript_errors_when_unreadable() {
        let dir = tempdir().unwrap();
        // A transcript that cannot be read resolves to SessionNotFound.
        let missing = dir.path().join(format!("{SAMPLE_ID}.jsonl"));
        assert!(matches!(
            session_dir_from_transcript(&missing),
            Err(ResolveError::SessionNotFound)
        ));
    }

    #[test]
    fn session_dir_from_transcript_errors_when_no_cwd() {
        let dir = tempdir().unwrap();
        let file = dir.path().join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&file, "{\"cwd\":null}\n").unwrap();

        assert!(matches!(
            session_dir_from_transcript(&file),
            Err(ResolveError::NoCwdInSession)
        ));
    }

    #[test]
    fn session_dir_from_transcript_errors_when_directory_missing() {
        let dir = tempdir().unwrap();
        let file = dir.path().join(format!("{SAMPLE_ID}.jsonl"));
        let missing = dir.path().join("gone");
        fs::write(&file, cwd_line(missing.to_str().unwrap())).unwrap();

        match session_dir_from_transcript(&file) {
            Err(ResolveError::DirectoryMissing(path)) => assert_eq!(path, missing),
            other => panic!("expected DirectoryMissing, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn session_dir_from_transcript_errors_when_directory_unreadable() {
        // A recorded cwd that still exists but cannot be entered is neither a
        // missing directory nor a usable one. Resolving it as usable is the
        // damaging case: `crap` would print a resume and exit 0, and the shell
        // function's `cd` would fail afterwards, with the binary that knew
        // better already gone.
        let tmp = tempdir().unwrap();
        let file = tmp.path().join(format!("{SAMPLE_ID}.jsonl"));
        let locked = tmp.path().join("locked-cwd");
        fs::create_dir_all(&locked).unwrap();
        fs::write(&file, cwd_line(locked.to_str().unwrap())).unwrap();
        if !lock_dir(&locked) {
            unlock_dir(&locked);
            return;
        }

        // Capture first, unlock second, assert last — a panic before the unlock
        // would strand an undeletable directory inside the tempdir.
        let resolved = session_dir_from_transcript(&file);
        unlock_dir(&locked);

        match resolved {
            Err(ResolveError::DirectoryUnreadable(path)) => assert_eq!(path, locked),
            other => panic!("expected DirectoryUnreadable, got {other:?}"),
        }
    }

    #[test]
    fn describe_resolve_error_session_not_found_is_plain_with_no_hint() {
        // A transcript that could not be read at all is not a problem `--here`
        // can route around — it would fork the same unreadable session — so the
        // hint would be a false lead. Exit code and headline only.
        let failure = describe_resolve_error(SAMPLE_ID, &ResolveError::SessionNotFound);
        assert_eq!(failure.code, exit_codes::SESSION_NOT_FOUND);
        assert!(
            failure.headline.contains(SAMPLE_ID),
            "headline names the id: {}",
            failure.headline
        );
        assert!(
            failure.detail.is_empty(),
            "no directory failed, so nothing points at --here: {:?}",
            failure.detail
        );
    }

    #[test]
    fn describe_resolve_error_no_cwd_is_plain_with_no_hint() {
        // A session that recorded no working directory has no directory to fork
        // *to*, so `--here` (which forks in the current directory) is not the
        // answer either; withholding the hint keeps it honest.
        let failure = describe_resolve_error(SAMPLE_ID, &ResolveError::NoCwdInSession);
        assert_eq!(failure.code, exit_codes::NO_CWD_IN_SESSION);
        assert!(
            !failure.detail.contains("crap --here"),
            "a transcript with no cwd earns no --here hint: {:?}",
            failure.detail
        );
    }

    #[test]
    fn describe_resolve_error_missing_directory_names_it_and_points_at_here() {
        // A gone directory is a dead end escaped only via --here, so the failure
        // must name the path, carry the hint keyed to this id, and select the
        // exit code that means "gone".
        let missing = PathBuf::from("/was/here/once");
        let failure =
            describe_resolve_error(SAMPLE_ID, &ResolveError::DirectoryMissing(missing.clone()));
        assert_eq!(failure.code, exit_codes::DIRECTORY_MISSING);
        assert!(
            failure.detail.contains(missing.to_str().unwrap()),
            "must name the gone directory: {:?}",
            failure.detail
        );
        assert!(
            failure.detail.contains(&format!("crap --here {SAMPLE_ID}")),
            "must hand back the escape hatch with this id: {:?}",
            failure.detail
        );
        assert!(
            failure.detail.contains("in the current directory instead"),
            "must say what --here does differently: {:?}",
            failure.detail
        );
    }

    #[test]
    fn describe_resolve_error_unreadable_directory_names_it_and_points_at_here() {
        // The harder-to-diagnose directory failure — sitting right there but
        // sealed — is the one most in need of the hint, and selects the
        // DIRECTORY_UNREADABLE code that distinguishes it from "gone". This is
        // the pure test #308's acceptance criteria call for.
        let sealed = PathBuf::from("/locked/out");
        let failure = describe_resolve_error(
            SAMPLE_ID,
            &ResolveError::DirectoryUnreadable(sealed.clone()),
        );
        assert_eq!(failure.code, exit_codes::DIRECTORY_UNREADABLE);
        assert!(
            failure.detail.contains(sealed.to_str().unwrap()),
            "must name the directory it cannot enter: {:?}",
            failure.detail
        );
        assert!(
            failure.detail.contains(&format!("crap --here {SAMPLE_ID}")),
            "must hand back the escape hatch with this id: {:?}",
            failure.detail
        );
        assert!(
            failure.detail.contains("in the current directory instead"),
            "must say what --here does differently: {:?}",
            failure.detail
        );
    }

    #[test]
    fn session_dir_from_transcript_returns_existing_directory() {
        let dir = tempdir().unwrap();
        let file = dir.path().join(format!("{SAMPLE_ID}.jsonl"));
        let cwd = dir.path().join("real-cwd");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(&file, cwd_line(cwd.to_str().unwrap())).unwrap();

        assert_eq!(session_dir_from_transcript(&file).unwrap(), cwd);
    }

    /// A real session record as written by Claude Code.
    const SESSION_JSON: &str = r#"{"pid":17041,"sessionId":"3eafa9f8-9d1f-43cf-b417-eb9efcb8ed4d","cwd":"/Volumes/code/crap","startedAt":1779730239473,"version":"2.1.150","kind":"interactive","entrypoint":"cli","status":"busy","updatedAt":1779730460209}"#;

    #[test]
    fn parse_session_record_extracts_fields() {
        let rec = parse_session_record(SESSION_JSON).expect("should parse");
        assert_eq!(rec.pid, 17041);
        assert_eq!(rec.session_id, "3eafa9f8-9d1f-43cf-b417-eb9efcb8ed4d");
        assert_eq!(rec.cwd, "/Volumes/code/crap");
        assert_eq!(rec.status.as_deref(), Some("busy"));
    }

    #[test]
    fn parse_session_record_rejects_malformed_or_incomplete() {
        assert_eq!(parse_session_record("not json"), None);
        assert_eq!(parse_session_record("{\"pid\":1}"), None); // no sessionId
        assert_eq!(parse_session_record("{\"sessionId\":\"x\"}"), None); // no pid
    }

    #[test]
    fn find_live_session_matches_only_alive_pid_for_id() {
        let dir = tempdir().unwrap();
        let target = "3eafa9f8-9d1f-43cf-b417-eb9efcb8ed4d";

        // A different session, alive — must be ignored.
        fs::write(
            dir.path().join("100.json"),
            serde_json::json!({"pid":100,"sessionId":"other","cwd":"/x"}).to_string(),
        )
        .unwrap();
        // The target session — written with pid 17041.
        fs::write(dir.path().join("17041.json"), SESSION_JSON).unwrap();

        // Nothing alive -> no live session.
        assert_eq!(find_live_session(dir.path(), target, |_| false), None);

        // Only the target pid alive -> found.
        let found = find_live_session(dir.path(), target, |pid| pid == 17041)
            .expect("target session should be found");
        assert_eq!(found.pid, 17041);
        assert_eq!(found.cwd, "/Volumes/code/crap");
    }

    #[test]
    fn find_live_session_ignores_stale_record_for_target() {
        let dir = tempdir().unwrap();
        let target = "3eafa9f8-9d1f-43cf-b417-eb9efcb8ed4d";
        fs::write(dir.path().join("17041.json"), SESSION_JSON).unwrap();

        // The matching record exists but its pid is dead (process exited
        // uncleanly leaving the file behind) -> not a live session.
        assert_eq!(find_live_session(dir.path(), target, |_| false), None);
    }

    #[test]
    fn find_live_session_missing_dir_is_none() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-sessions-here");
        assert_eq!(find_live_session(&missing, "anything", |_| true), None);
    }

    /// The session id recorded in [`SESSION_JSON`].
    const LIVE_ID: &str = "3eafa9f8-9d1f-43cf-b417-eb9efcb8ed4d";

    /// Writes a transcript whose single assistant turn ended cleanly (so the
    /// classifier reports `waiting-for-user`) into a project subfolder.
    fn write_waiting_transcript(projects: &Path, session_id: &str) {
        let proj = projects.join("proj");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join(format!("{session_id}.jsonl")),
            assistant_line(block("text"), serde_json::json!("end_turn")),
        )
        .unwrap();
    }

    #[test]
    fn resolve_status_report_prefers_live_session_over_transcript() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        // A transcript that would classify as waiting-for-user...
        write_waiting_transcript(projects.path(), LIVE_ID);
        // ...but the session is live, so the live status wins for `state`.
        fs::write(sessions.path().join("17041.json"), SESSION_JSON).unwrap();

        let report = resolve_status_report(
            &self_root(projects.path()),
            sessions.path(),
            LIVE_ID,
            |pid| pid == 17041,
        )
        .unwrap();
        assert_eq!(report.state, "busy (live, pid 17041)");
    }

    #[test]
    fn output_emits_session_id_before_directory() {
        // The session id (a validated UUID) goes on the first line; the
        // directory comes last so any newline inside a path can't be mistaken
        // for the end of the session-id field.
        let weird_dir = Path::new("/Users/tim/od\nd\u{2009}dir");
        let out = format_output(weird_dir, SAMPLE_ID);

        assert_eq!(out.lines().next(), Some(SAMPLE_ID));
        // Everything after the first newline is the directory, intact.
        let rest = out.split_once('\n').map(|(_, rest)| rest).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), weird_dir.to_str().unwrap());
    }

    /// A single self-owned search root over `projects`, for the `--here` import
    /// tests. `is_self` is true, so imports through it symlink (same-user).
    fn self_root(projects: &Path) -> Vec<UserProjects> {
        vec![UserProjects {
            user: "me".to_string(),
            projects_dir: projects.to_path_buf(),
            is_self: true,
        }]
    }

    #[test]
    fn resolve_here_import_rejects_invalid_id() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_here_import(
                &self_root(dir.path()),
                dir.path(),
                Path::new("/x"),
                "../escape"
            ),
            Err(HereResolveError::InvalidSessionId)
        ));
    }

    #[test]
    fn resolve_here_import_errors_when_session_missing() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_here_import(
                &self_root(dir.path()),
                dir.path(),
                Path::new("/x"),
                SAMPLE_ID
            ),
            Err(HereResolveError::SessionNotFound { .. })
        ));
    }

    #[test]
    fn resolve_here_import_symlinks_a_same_user_source() {
        // Same-user `--here` must still symlink, exactly as before: the import
        // is a link back to the original in our own tree (regression guard).
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let link = resolve_here_import(
            &self_root(projects),
            projects,
            Path::new("/Volumes/x/here"),
            SAMPLE_ID,
        )
        .expect("ok")
        .expect("a symlink should be created");
        assert_eq!(
            link,
            projects
                .join("-Volumes-x-here")
                .join(format!("{SAMPLE_ID}.jsonl"))
        );
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "same-user `--here` must symlink"
        );
        assert_eq!(fs::read_link(&link).unwrap(), original);
    }

    #[test]
    fn resolve_here_import_copies_a_cross_user_source() {
        // `--here <id> --user <name>`: the source lives under another user's
        // tree, so it must be COPIED into our own tree (a real file), never
        // symlinked into their home. The copy lands at pwd's encoded folder
        // under our own dest tree and snapshots the foreign bytes.
        let dir = tempdir().unwrap();
        let dest = dir.path().join("me/.claude/projects");
        fs::create_dir_all(&dest).unwrap();
        // A foreign root: another user's `.claude/projects` tree.
        let foreign = dir.path().join("them/.claude/projects");
        let foreign_proj = foreign.join("-orig");
        fs::create_dir_all(&foreign_proj).unwrap();
        let source = foreign_proj.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&source, "{\"cwd\":\"/work\"}\n").unwrap();

        let roots = vec![UserProjects {
            user: "them".to_string(),
            projects_dir: foreign,
            is_self: false,
        }];
        let pwd = Path::new("/Volumes/x/here");
        let link = resolve_here_import(&roots, &dest, pwd, SAMPLE_ID)
            .expect("ok")
            .expect("a copy should be created");

        // The import lands in OUR tree, at pwd's encoded project folder.
        assert_eq!(
            link,
            dest.join("-Volumes-x-here")
                .join(format!("{SAMPLE_ID}.jsonl"))
        );
        // It is a real copy, not a symlink pointing back into the foreign home.
        assert!(
            !fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "cross-user `--here` must copy, not symlink into another user's home"
        );
        assert_eq!(
            fs::read_to_string(&link).unwrap(),
            "{\"cwd\":\"/work\"}\n",
            "the copy must snapshot the foreign transcript bytes"
        );
        // The foreign original is left untouched.
        assert!(source.is_file());
    }

    #[test]
    fn resolve_here_import_returns_none_when_session_already_here() {
        // The session already lives in the current directory's folder, so no
        // import is needed and there is nothing to clean up afterwards.
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let folder = projects.join("-Volumes-x-here");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(format!("{SAMPLE_ID}.jsonl")), "{}\n").unwrap();

        assert_eq!(
            resolve_here_import(
                &self_root(projects),
                projects,
                Path::new("/Volumes/x/here"),
                SAMPLE_ID
            )
            .expect("ok"),
            None
        );
    }

    #[test]
    fn shell_code_detects_here_sentinel_from_the_binary() {
        // The shell function branches on the exact sentinel the binary emits.
        assert!(SHELL_CODE.contains(HERE_SENTINEL));
    }

    #[test]
    fn shell_code_forks_and_cleans_up_in_here_mode() {
        // here-mode forks a fresh session instead of appending to the original
        // transcript...
        assert!(SHELL_CODE.contains("--fork-session"));
        // ...and removes the temporary symlink afterwards, unless there was
        // none to remove.
        assert!(SHELL_CODE.contains(r#"rm -f -- "$__crap_link""#));
        assert!(SHELL_CODE.contains(NO_LINK_SENTINEL));
    }

    #[test]
    fn shell_code_removes_here_symlink_early_via_background_watcher() {
        // A backgrounded watcher polls the project folder and removes the
        // symlink as soon as a new (forked) session file appears — Claude no
        // longer needs the symlink once it has read the transcript — instead of
        // letting it linger for the whole session.
        assert!(SHELL_CODE.contains(r#"find "$__crap_folder""#));
        assert!(SHELL_CODE.contains(r#"-gt "$__crap_n0""#));
        assert!(SHELL_CODE.contains(") &"));
        assert!(SHELL_CODE.contains("sleep 0.1"));
        // The watcher is stopped once claude exits, and the post-exit `rm`
        // remains as a safety net in case the fork file never appeared.
        assert!(SHELL_CODE.contains(r#"kill "$__crap_watcher""#));
    }

    #[test]
    fn here_output_carries_sentinel_session_and_link() {
        let link = Path::new("/Users/tim/.claude/projects/-x/abc.jsonl");
        let out = format_here_output(SAMPLE_ID, None, Some(link));

        let mut lines = out.lines();
        assert_eq!(lines.next(), Some(HERE_SENTINEL));
        assert_eq!(lines.next(), Some(SAMPLE_ID));
        // No forced id was given, so the third field is the sentinel.
        assert_eq!(lines.next(), Some(NO_NEW_ID_SENTINEL));
        // Everything after the third newline is the link path, intact.
        let rest = out.splitn(4, '\n').nth(3).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), link.to_str().unwrap());
    }

    #[test]
    fn here_output_uses_no_link_sentinel_when_nothing_to_clean() {
        let out = format_here_output(SAMPLE_ID, None, None);

        assert_eq!(out.lines().next(), Some(HERE_SENTINEL));
        let link_field = out.splitn(4, '\n').nth(3).unwrap();
        assert_eq!(link_field.trim_end_matches('\n'), NO_LINK_SENTINEL);
    }

    #[test]
    fn here_output_carries_forced_new_session_id() {
        // When the caller supplies a forked-session id, it rides as the third
        // field so the shell function can pass it to `claude --session-id`.
        let link = Path::new("/Users/tim/.claude/projects/-x/abc.jsonl");
        let out = format_here_output(SAMPLE_ID, Some(ID_B), Some(link));

        let mut lines = out.lines();
        assert_eq!(lines.next(), Some(HERE_SENTINEL));
        assert_eq!(lines.next(), Some(SAMPLE_ID));
        assert_eq!(lines.next(), Some(ID_B));
        // The link still lives last, after the forced-id field.
        let rest = out.splitn(4, '\n').nth(3).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), link.to_str().unwrap());
    }

    #[test]
    fn here_output_uses_no_new_id_sentinel_when_absent() {
        // Without a caller-supplied id, the third field is the sentinel so the
        // shell function lets Claude mint a fresh random id.
        let out = format_here_output(SAMPLE_ID, None, None);
        assert_eq!(out.lines().nth(2), Some(NO_NEW_ID_SENTINEL));
    }

    #[test]
    fn here_output_preserves_newline_in_link_path() {
        // The link lives last in the output, so a newline inside the path can't
        // be mistaken for a field boundary.
        let link = Path::new("/Users/tim/od\ndd/abc.jsonl");
        let out = format_here_output(SAMPLE_ID, None, Some(link));

        let rest = out.splitn(4, '\n').nth(3).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), link.to_str().unwrap());
    }

    #[test]
    fn fork_at_output_emits_dir_last_with_sentinels_in_slots() {
        // Cross-user default resume: sentinel, session, new-id slot, link slot,
        // then the original directory last.
        let link = Path::new("/Users/tim/.claude/projects/-work/abc.jsonl");
        let dir = Path::new("/Volumes/x/work");
        let out = format_fork_at_output(SAMPLE_ID, None, Some(link), dir);

        let mut lines = out.lines();
        assert_eq!(lines.next(), Some(FORK_AT_SENTINEL));
        assert_eq!(lines.next(), Some(SAMPLE_ID));
        // No forced id: the new-id slot is the sentinel.
        assert_eq!(lines.next(), Some(NO_NEW_ID_SENTINEL));
        assert_eq!(lines.next(), Some(link.to_str().unwrap()));
        // Everything after the fourth newline is the directory, intact.
        let rest = out.splitn(5, '\n').nth(4).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), dir.to_str().unwrap());
    }

    #[test]
    fn fork_at_output_uses_no_link_sentinel_when_nothing_to_clean() {
        // When the import was a no-op (already resolvable), the link slot is the
        // sentinel so the shell knows there is nothing to remove.
        let dir = Path::new("/Volumes/x/work");
        let out = format_fork_at_output(SAMPLE_ID, None, None, dir);
        assert_eq!(out.lines().nth(3), Some(NO_LINK_SENTINEL));
    }

    #[test]
    fn fork_at_output_carries_forced_new_id() {
        // A caller-supplied forked id rides in the new-id slot for the shell's
        // `--session-id`, while the directory still comes last.
        let link = Path::new("/Users/tim/.claude/projects/-work/abc.jsonl");
        let dir = Path::new("/Volumes/x/work");
        let out = format_fork_at_output(SAMPLE_ID, Some(ID_B), Some(link), dir);
        assert_eq!(out.lines().nth(2), Some(ID_B));
        let rest = out.splitn(5, '\n').nth(4).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), dir.to_str().unwrap());
    }

    #[test]
    fn fork_at_output_preserves_newline_in_dir() {
        // The directory lives last, so a newline inside it can't be mistaken for
        // a field boundary — the invariant the whole layout is designed around.
        let link = Path::new("/Users/tim/.claude/projects/-work/abc.jsonl");
        let dir = Path::new("/Volumes/x/od\ndd");
        let out = format_fork_at_output(SAMPLE_ID, None, Some(link), dir);

        let rest = out.splitn(5, '\n').nth(4).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), dir.to_str().unwrap());
    }

    #[test]
    fn shell_code_guards_cd_against_dash_prefixed_dirs() {
        // `cd -- "$dir"` stops option parsing, so a directory whose name begins
        // with '-' is treated as a path rather than a flag.
        assert!(SHELL_CODE.contains("cd -- \"$__crap_dir\""));
    }

    #[test]
    fn shell_code_defines_function_and_dispatches_to_claude() {
        assert!(SHELL_CODE.contains("function crap()"));
        // Forwards all args so --force reaches the binary.
        assert!(SHELL_CODE.contains("command crap \"$@\""));
        // Splits the output on the first newline: session id leads, directory
        // is the remainder (so a path with embedded newlines stays whole).
        assert!(SHELL_CODE.contains("__crap_session=${__crap_out%%$'\\n'*}"));
        assert!(SHELL_CODE.contains("__crap_dir=${__crap_out#*$'\\n'}"));
        assert!(SHELL_CODE.contains("clauded --resume"));
        assert!(SHELL_CODE.contains("claude --resume"));
    }

    #[test]
    fn shell_code_passes_informational_flags_through_untouched() {
        // `--status` queries, --help/-h/--version/-V print informational text,
        // and --shell-setup writes the rc file (not the live shell); none mutate
        // the parent shell, and each must reach the terminal rather than being
        // parsed as a "<session-id>\n<dir>" resume target.
        assert!(SHELL_CODE.contains(
            r#"*" --status "*|*" --help "*|*" -h "*|*" --version "*|*" -V "*|*" --shell-setup "*)"#
        ));
        assert!(SHELL_CODE.contains(r#"command crap "$@"; return $?"#));
    }

    /// Sources `SHELL_CODE` in a real `bash`, with a fake `crap` binary (and
    /// fake `claude`/`clauded`) ahead of it on `PATH`, then runs `crap <args>`.
    ///
    /// The fake binary mimics clap: informational flags print a recognizable
    /// marker to stdout and exit 0; anything else emits a `<session>\n<dir>`
    /// resume target. Returns the captured stdout plus whether the `claude`
    /// stub was invoked (it drops a marker file when called).
    ///
    /// Each call gets its own `tempfile::TempDir` (an `O_EXCL` random name) so
    /// concurrent runs of this test never share a directory. A pid+nanos name is
    /// NOT enough: two threads in the same test process can sample the clock in
    /// the same tick and collide, letting one run clobber the other's fakes.
    #[cfg(unix)]
    fn run_shell_function(args: &str) -> (String, bool) {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path();

        let claude_marker = dir.join("claude_called");

        // Fake `crap`: informational flags print a marker and exit 0, exactly
        // as clap does; the default path prints a resume target.
        let fake_crap = "#!/bin/sh\n\
            case \" $* \" in\n\
            \x20 *\" --help \"*|*\" -h \"*) printf 'CRAP_HELP_MARKER\\nUsage: crap\\nmore\\n'; exit 0 ;;\n\
            \x20 *\" --version \"*|*\" -V \"*) printf 'CRAP_VERSION_MARKER 0.1.0\\n'; exit 0 ;;\n\
            \x20 *\" --shell-setup \"*) printf 'CRAP_SETUP_MARKER\\nTo activate, run:\\n  source ~/.zshrc\\n'; exit 0 ;;\n\
            esac\n\
            printf 'session-xyz\\n/tmp/crap-resume-dir\\n'\n";

        // Fake `claude`/`clauded`: record that a resume was attempted.
        let fake_claude = format!("#!/bin/sh\n: > {:?}\n", claude_marker);

        for (name, body) in [
            ("crap", fake_crap.to_string()),
            ("claude", fake_claude.clone()),
            ("clauded", fake_claude),
        ] {
            let path = dir.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let base_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{base_path}", dir.display());
        let script = format!("{SHELL_CODE}\ncrap {args}\n");

        let output = Command::new("bash")
            .env("PATH", new_path)
            .arg("-c")
            .arg(&script)
            .output()
            .expect("bash should be available");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let claude_called = claude_marker.exists();
        // `temp` drops at end of scope, removing the directory.
        (stdout, claude_called)
    }

    #[cfg(unix)]
    #[test]
    fn shell_function_passes_informational_flags_through() {
        // These flags make the binary print to stdout and exit 0 without
        // mutating the parent shell. Without a pass-through, the function
        // captures that text and tries to `cd` into it as a resume directory.
        // Each must reach the terminal verbatim and never trigger a resume.
        // `--shell-setup` is included because, on an upgrade, the already-loaded
        // function would otherwise swallow its activation instructions.
        for (args, marker) in [
            ("--help", "CRAP_HELP_MARKER"),
            ("-h", "CRAP_HELP_MARKER"),
            ("--version", "CRAP_VERSION_MARKER"),
            ("-V", "CRAP_VERSION_MARKER"),
            ("--shell-setup", "CRAP_SETUP_MARKER"),
        ] {
            let (stdout, claude_called) = run_shell_function(args);
            assert!(
                stdout.contains(marker),
                "`crap {args}` should print the binary's output verbatim, got: {stdout:?}"
            );
            assert!(!claude_called, "`crap {args}` must not attempt a resume");
        }
    }

    /// Sources `SHELL_CODE` in a real `bash` with a fake `crap` that emits
    /// here-mode output carrying `new_id_field` as its third field (and
    /// `__CRAP_NO_LINK__`, so the symlink watcher is skipped), plus fake
    /// `claude`/`clauded` that record the exact arguments they were resumed
    /// with. Returns those recorded arguments, one per line.
    ///
    /// When `provide_clauded` is true the preferred `clauded` is on `PATH` and
    /// records; otherwise only plain `claude` is available. Each call gets its
    /// own `tempfile::TempDir` (an `O_EXCL` random name) so concurrent runs
    /// never share a directory. A pid+nanos name is NOT enough: two threads in
    /// the same test process can sample the clock in the same tick and collide,
    /// letting one run read args the other recorded.
    #[cfg(unix)]
    fn run_here_shell_function(new_id_field: &str, provide_clauded: bool) -> String {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        // The session id the fake binary reports as the resumed original.
        const HERE_SESSION: &str = "33333333-4444-5555-6666-777777777777";

        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path();

        let args_file = dir.join("claude_args");

        // Fake `crap`: emit a here-output whose third field is `new_id_field`.
        let fake_crap = format!(
            "#!/bin/sh\nprintf '{HERE_SENTINEL}\\n%s\\n%s\\n%s\\n' '{HERE_SESSION}' '{new_id_field}' '{NO_LINK_SENTINEL}'\n"
        );

        // Fake `claude`/`clauded`: record the exact argument list, one per line.
        let fake_claude = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {:?}\n", args_file);

        let mut tools = vec![("crap", fake_crap), ("claude", fake_claude.clone())];
        if provide_clauded {
            tools.push(("clauded", fake_claude));
        }
        for (name, body) in tools {
            let path = dir.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let base_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{base_path}", dir.display());
        let script = format!("{SHELL_CODE}\ncrap --here {HERE_SESSION}\n");

        let output = Command::new("bash")
            .env("PATH", new_path)
            .arg("-c")
            .arg(&script)
            .output()
            .expect("bash should be available");
        assert!(
            output.status.success(),
            "here-mode shell function failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let recorded = fs::read_to_string(&args_file).unwrap_or_default();
        // `temp` drops at end of scope, removing the directory.
        recorded
    }

    /// A well-formed forked-session id for the here-mode dispatch tests.
    const FORCED_NEW_ID: &str = "99999999-8888-7777-6666-555555555555";

    #[cfg(unix)]
    #[test]
    fn shell_function_pins_forced_new_id_via_session_id() {
        // When the binary supplies a forced id, the resume must fork *and* pin
        // the fork to that id with `--session-id`, on both the `clauded` and the
        // plain `claude` dispatch paths.
        for provide_clauded in [true, false] {
            let recorded = run_here_shell_function(FORCED_NEW_ID, provide_clauded);
            let args: Vec<&str> = recorded.lines().collect();
            assert!(
                args.contains(&"--fork-session"),
                "must still fork (clauded={provide_clauded}); got {args:?}"
            );
            let pos = args
                .iter()
                .position(|a| *a == "--session-id")
                .unwrap_or_else(|| {
                    panic!("--session-id missing (clauded={provide_clauded}): {args:?}")
                });
            assert_eq!(
                args.get(pos + 1).copied(),
                Some(FORCED_NEW_ID),
                "the forced id must follow --session-id (clauded={provide_clauded})"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn shell_function_omits_session_id_without_a_forced_new_id() {
        // The sentinel third field means "let Claude mint a random id": the
        // resume forks but must not pass --session-id.
        let recorded = run_here_shell_function(NO_NEW_ID_SENTINEL, true);
        let args: Vec<&str> = recorded.lines().collect();
        assert!(
            args.contains(&"--fork-session"),
            "must still fork: {args:?}"
        );
        assert!(
            !args.contains(&"--session-id"),
            "no forced id => no --session-id: {args:?}"
        );
    }

    #[test]
    fn shell_code_parses_new_id_field_and_pins_session_id() {
        // here-mode reads the third field and, unless it is the sentinel, pins
        // the fork's id via `claude --session-id`.
        assert!(SHELL_CODE.contains(NO_NEW_ID_SENTINEL));
        assert!(SHELL_CODE.contains("--session-id"));
    }

    #[test]
    fn shell_code_detects_fork_at_sentinel() {
        // The shell function branches on the exact cross-user sentinel the
        // binary emits for a resume at the session's original directory.
        assert!(SHELL_CODE.contains(FORK_AT_SENTINEL));
    }

    /// Sources `SHELL_CODE` in a real `bash` with a fake `crap` that emits
    /// cross-user `__CRAP_FORK_AT__` output naming a real `orig-cwd` directory
    /// (and `__CRAP_NO_LINK__`, so no watcher/cleanup runs), plus a fake
    /// `claude`/`clauded` that records both the arguments it was resumed with
    /// and the working directory it ran in. Returns `(recorded_args,
    /// resumed_in_dir, orig_dir)`, where both directories are canonicalized
    /// **before** the temp dir is dropped so the caller can compare them without
    /// touching a filesystem path that no longer exists.
    ///
    /// Each call gets its own `tempfile::TempDir` (an `O_EXCL` random name) so
    /// concurrent runs never share a directory.
    #[cfg(unix)]
    fn run_fork_at_shell_function() -> (String, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        // The session id the fake binary reports as the foreign original.
        const FORK_SESSION: &str = "33333333-4444-5555-6666-777777777777";

        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path();

        let args_file = dir.join("claude_args");
        let pwd_file = dir.join("claude_pwd");
        // The session's original recorded directory: the fork must land here.
        let orig = dir.join("orig-cwd");
        fs::create_dir_all(&orig).unwrap();

        // Fake `crap`: emit a fork-at output naming `orig` as the last field.
        let fake_crap = format!(
            "#!/bin/sh\nprintf '{FORK_AT_SENTINEL}\\n%s\\n%s\\n%s\\n%s\\n' '{FORK_SESSION}' '{NO_NEW_ID_SENTINEL}' '{NO_LINK_SENTINEL}' '{}'\n",
            orig.display()
        );
        // Fake `claude`/`clauded`: record the resume argv and the cwd it ran in.
        let fake_claude = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {:?}\npwd > {:?}\n",
            args_file, pwd_file
        );

        for (name, body) in [
            ("crap", fake_crap),
            ("claude", fake_claude.clone()),
            ("clauded", fake_claude),
        ] {
            let path = dir.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let base_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{base_path}", dir.display());
        let script = format!("{SHELL_CODE}\ncrap {FORK_SESSION} --user someone\n");

        let output = Command::new("bash")
            .env("PATH", new_path)
            .arg("-c")
            .arg(&script)
            .output()
            .expect("bash should be available");

        let args = fs::read_to_string(&args_file).unwrap_or_default();
        let pwd = fs::read_to_string(&pwd_file).unwrap_or_default();
        assert!(
            output.status.success(),
            "fork-at shell function failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Canonicalize both directories while `temp` is still alive — it drops
        // (removing the tree) at the end of this scope, so the caller must not
        // depend on either path still existing on disk.
        let resumed_in = std::fs::canonicalize(pwd.trim())
            .expect("claude should have recorded the directory it forked in");
        let orig = std::fs::canonicalize(&orig).unwrap();
        (args, resumed_in, orig)
    }

    #[cfg(unix)]
    #[test]
    fn shell_function_forks_at_original_dir_for_cross_user() {
        // Cross-user default resume: the function must `cd` into the session's
        // original directory and then fork it (`--resume <id> --fork-session`),
        // leaving the foreign transcript untouched.
        let (args, resumed_in, orig) = run_fork_at_shell_function();
        let args: Vec<&str> = args.lines().collect();
        assert!(
            args.contains(&"--resume") && args.contains(&"--fork-session"),
            "must fork-resume the original id; got {args:?}"
        );
        assert_eq!(
            resumed_in, orig,
            "the fork must run in the session's original directory"
        );
    }

    // Two distinct session ids for the per-directory listing tests.
    const ID_A: &str = "aaaaaaaa-1111-2222-3333-444444444444";
    const ID_B: &str = "bbbbbbbb-1111-2222-3333-444444444444";

    /// Builds a transcript line of `kind` carrying a top-level `timestamp`.
    fn timestamped_line(kind: &str, timestamp: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({ "type": kind, "timestamp": timestamp, "message": {} })
        )
    }

    #[test]
    fn transcript_time_span_returns_earliest_and_latest() {
        let contents = format!(
            "{}{}{}",
            timestamped_line("user", "2026-05-25T18:43:05.109Z"),
            timestamped_line("assistant", "2026-05-25T19:00:00.000Z"),
            timestamped_line("assistant", "2026-05-25T20:17:39.732Z"),
        );
        assert_eq!(
            transcript_time_span(&contents),
            (
                Some("2026-05-25T18:43:05.109Z".to_string()),
                Some("2026-05-25T20:17:39.732Z".to_string())
            )
        );
    }

    #[test]
    fn transcript_time_span_ignores_lines_without_timestamps() {
        // Bookkeeping lines carry no timestamp and must not affect the span.
        let contents = format!(
            "{}{}{}",
            "{\"type\":\"last-prompt\"}\n",
            timestamped_line("user", "2026-05-25T18:43:05.109Z"),
            "{\"type\":\"ai-title\"}\n",
        );
        assert_eq!(
            transcript_time_span(&contents),
            (
                Some("2026-05-25T18:43:05.109Z".to_string()),
                Some("2026-05-25T18:43:05.109Z".to_string())
            )
        );
    }

    #[test]
    fn transcript_time_span_is_order_independent() {
        // A line written later may bear an earlier instant; min/max still hold.
        let contents = format!(
            "{}{}",
            timestamped_line("assistant", "2026-05-25T20:00:00.000Z"),
            timestamped_line("user", "2026-05-25T08:00:00.000Z"),
        );
        assert_eq!(
            transcript_time_span(&contents),
            (
                Some("2026-05-25T08:00:00.000Z".to_string()),
                Some("2026-05-25T20:00:00.000Z".to_string())
            )
        );
    }

    #[test]
    fn transcript_time_span_none_when_no_timestamps() {
        assert_eq!(
            transcript_time_span("{\"type\":\"last-prompt\"}\n"),
            (None, None)
        );
    }

    #[test]
    fn format_timestamp_prettifies_iso8601() {
        assert_eq!(
            format_timestamp("2026-05-25T18:43:05.109Z"),
            "2026-05-25 18:43:05"
        );
    }

    #[test]
    fn format_timestamp_handles_missing_subseconds() {
        assert_eq!(
            format_timestamp("2026-05-25T18:43:05Z"),
            "2026-05-25 18:43:05"
        );
    }

    #[test]
    fn format_timestamp_passes_through_unexpected_input() {
        assert_eq!(format_timestamp("not-a-timestamp"), "not-a-timestamp");
    }

    /// Writes a single-turn transcript (classifies as `waiting-for-user`) with
    /// one `timestamp` into `folder`.
    fn write_session_in(folder: &Path, session_id: &str, timestamp: &str) {
        fs::create_dir_all(folder).unwrap();
        let line = format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "timestamp": timestamp,
                "message": { "stop_reason": "end_turn", "content": [{ "type": "text" }] },
            })
        );
        fs::write(folder.join(format!("{session_id}.jsonl")), line).unwrap();
    }

    #[test]
    fn resolve_dir_statuses_lists_all_sessions_in_pwd_folder() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        let pwd = Path::new("/Volumes/x/proj");
        let folder = projects.path().join(encode_project_dir(pwd));
        write_session_in(&folder, ID_A, "2026-05-25T10:00:00.000Z");
        write_session_in(&folder, ID_B, "2026-05-25T11:00:00.000Z");

        let reports = resolve_dir_statuses(projects.path(), sessions.path(), pwd, |_| false);
        assert_eq!(reports.len(), 2);
        // Ascending by last-activity: the most recently used session is last,
        // so it lands at the bottom of the printed table.
        assert_eq!(reports[0].session_id, ID_A);
        assert_eq!(reports[1].session_id, ID_B);
        assert!(reports.iter().all(|r| r.state == "waiting-for-user"));
        assert_eq!(
            reports[1].started.as_deref(),
            Some("2026-05-25T11:00:00.000Z")
        );
        assert_eq!(reports[1].last.as_deref(), Some("2026-05-25T11:00:00.000Z"));
    }

    #[test]
    fn resolve_dir_statuses_ignores_non_session_files() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        let pwd = Path::new("/Volumes/x/proj");
        let folder = projects.path().join(encode_project_dir(pwd));
        write_session_in(&folder, ID_A, "2026-05-25T10:00:00.000Z");
        fs::write(folder.join("notes.txt"), "hi").unwrap();
        fs::write(folder.join("not-a-uuid.jsonl"), "{}\n").unwrap();

        let reports = resolve_dir_statuses(projects.path(), sessions.path(), pwd, |_| false);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].session_id, ID_A);
    }

    #[test]
    fn resolve_dir_statuses_empty_when_folder_absent() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        let reports = resolve_dir_statuses(
            projects.path(),
            sessions.path(),
            Path::new("/no/such/dir"),
            |_| false,
        );
        assert!(reports.is_empty());
    }

    #[test]
    fn resolve_dir_statuses_marks_live_session() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        // The cwd's project folder holds the live session's transcript, but the
        // live process's own status wins over transcript inference.
        let pwd = Path::new("/Volumes/code/crap");
        let folder = projects.path().join(encode_project_dir(pwd));
        write_session_in(&folder, LIVE_ID, "2026-05-25T10:00:00.000Z");
        fs::write(sessions.path().join("17041.json"), SESSION_JSON).unwrap();

        let reports =
            resolve_dir_statuses(projects.path(), sessions.path(), pwd, |pid| pid == 17041);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].state, "busy (live, pid 17041)");
    }

    #[test]
    fn format_dir_statuses_reports_none_found_for_empty() {
        let out = format_dir_statuses(Path::new("/Volumes/x/proj"), &[]);
        assert!(out.contains("No Claude sessions found"));
        assert!(out.contains("/Volumes/x/proj"));
    }

    #[test]
    fn format_dir_statuses_tabulates_each_session_with_times() {
        let reports = vec![SessionStatusReport {
            session_id: ID_A.to_string(),
            state: "waiting-for-user".to_string(),
            started: Some("2026-05-25T10:00:00.000Z".to_string()),
            last: Some("2026-05-25T12:30:45.000Z".to_string()),
        }];
        let out = format_dir_statuses(Path::new("/Volumes/x/proj"), &reports);
        // A heading line, then a table with column headers.
        assert!(out.contains("1 session for /Volumes/x/proj"));
        assert!(out.contains("SESSION"));
        assert!(out.contains("STATE"));
        assert!(out.contains("STARTED"));
        assert!(out.contains("LAST"));
        assert!(out.contains(ID_A));
        assert!(out.contains("waiting-for-user"));
        // Times are prettified in the cells (no raw `T`/`Z`, no `started`/`last`
        // labels — those are column headers now).
        assert!(out.contains("2026-05-25 10:00:00"));
        assert!(out.contains("2026-05-25 12:30:45"));
    }

    #[test]
    fn dir_statuses_table_never_wraps_cells_to_fit_a_narrow_terminal() {
        // The status table must render at its natural content width regardless of
        // the terminal: a session UUID or timestamp chopped across lines is
        // unreadable, and tying the layout to the ambient terminal width makes the
        // output (and every test that asserts on it) depend on a shared,
        // uncontrolled resource — a flaky-test trap. Force an absurdly narrow
        // width and demand every cell still appears whole on one line.
        let reports = vec![SessionStatusReport {
            session_id: ID_A.to_string(),
            state: "waiting-for-user".to_string(),
            started: Some("2026-05-25T10:00:00.000Z".to_string()),
            last: Some("2026-05-25T12:30:45.000Z".to_string()),
        }];
        let mut table = dir_statuses_table(&reports);
        table.set_width(20);
        let rendered = table.to_string();
        assert!(
            rendered.contains(ID_A),
            "UUID must stay intact at narrow width, got:\n{rendered}"
        );
        assert!(rendered.contains("waiting-for-user"));
        assert!(rendered.contains("2026-05-25 10:00:00"));
        assert!(rendered.contains("2026-05-25 12:30:45"));
    }

    #[test]
    fn format_dir_statuses_uses_plural_and_dash_for_missing_times() {
        let reports = vec![
            SessionStatusReport {
                session_id: ID_A.to_string(),
                state: "empty".to_string(),
                started: None,
                last: None,
            },
            SessionStatusReport {
                session_id: ID_B.to_string(),
                state: "busy".to_string(),
                started: Some("2026-05-25T10:00:00.000Z".to_string()),
                last: Some("2026-05-25T10:05:00.000Z".to_string()),
            },
        ];
        let out = format_dir_statuses(Path::new("/x"), &reports);
        assert!(out.contains("2 sessions for /x"));
        // The session with no recorded activity shows an em dash placeholder.
        assert!(out.contains("—"));
    }

    #[test]
    fn format_dir_statuses_json_emits_array_with_raw_timestamps() {
        let reports = vec![SessionStatusReport {
            session_id: ID_A.to_string(),
            state: "busy (live, pid 17041)".to_string(),
            started: Some("2026-05-25T10:00:00.000Z".to_string()),
            last: Some("2026-05-25T12:30:45.000Z".to_string()),
        }];
        let out = format_dir_statuses_json(&reports);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        let arr = parsed.as_array().expect("a JSON array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["sessionId"], ID_A);
        assert_eq!(arr[0]["state"], "busy (live, pid 17041)");
        // Raw ISO 8601 is preserved for machine consumers, not prettified.
        assert_eq!(arr[0]["started"], "2026-05-25T10:00:00.000Z");
        assert_eq!(arr[0]["last"], "2026-05-25T12:30:45.000Z");
    }

    #[test]
    fn format_dir_statuses_json_empty_is_empty_array() {
        let parsed: serde_json::Value =
            serde_json::from_str(&format_dir_statuses_json(&[])).expect("valid JSON");
        assert_eq!(parsed.as_array().expect("a JSON array").len(), 0);
    }

    #[test]
    fn format_status_json_emits_single_object() {
        let report = SessionStatusReport {
            session_id: ID_A.to_string(),
            state: "waiting-for-user".to_string(),
            started: Some("2026-05-25T10:00:00.000Z".to_string()),
            last: None,
        };
        let parsed: serde_json::Value =
            serde_json::from_str(&format_status_json(&report)).expect("valid JSON");
        assert_eq!(parsed["sessionId"], ID_A);
        assert_eq!(parsed["state"], "waiting-for-user");
        assert_eq!(parsed["started"], "2026-05-25T10:00:00.000Z");
        assert!(parsed["last"].is_null());
    }

    #[test]
    fn resolve_status_report_carries_state_and_time_span() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        let proj = projects.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        let line = format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "timestamp": "2026-05-25T10:00:00.000Z",
                "message": { "stop_reason": "end_turn", "content": [{ "type": "text" }] },
            })
        );
        fs::write(proj.join(format!("{SAMPLE_ID}.jsonl")), line).unwrap();

        let report = resolve_status_report(
            &self_root(projects.path()),
            sessions.path(),
            SAMPLE_ID,
            |_| false,
        )
        .unwrap();
        assert_eq!(report.session_id, SAMPLE_ID);
        assert_eq!(report.state, "waiting-for-user");
        assert_eq!(report.started.as_deref(), Some("2026-05-25T10:00:00.000Z"));
        assert_eq!(report.last.as_deref(), Some("2026-05-25T10:00:00.000Z"));
    }

    #[test]
    fn resolve_status_report_uses_live_state_for_the_state_field() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        fs::write(sessions.path().join("17041.json"), SESSION_JSON).unwrap();

        let report = resolve_status_report(
            &self_root(projects.path()),
            sessions.path(),
            LIVE_ID,
            |pid| pid == 17041,
        )
        .unwrap();
        assert_eq!(report.state, "busy (live, pid 17041)");
    }

    #[test]
    fn resolve_status_report_rejects_invalid_id() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_status_report(&self_root(dir.path()), dir.path(), "../escape", |_| true),
            Err(StatusError::InvalidSessionId)
        ));
    }

    #[test]
    fn resolve_status_report_errors_when_session_missing() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_status_report(&self_root(dir.path()), dir.path(), SAMPLE_ID, |_| false),
            Err(StatusError::SessionNotFound { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_status_report_carries_skipped_owner_only_dirs_on_a_miss() {
        // A cross-user status miss that stepped over an owner-only project dir
        // must carry that skip, so run_status can print the same copy-it-first
        // guidance the resume forms do. A bare "not found" would assert a
        // certainty an opaque directory denies the scan.
        let self_projects = tempdir().unwrap();
        let sibling_projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        let locked = sibling_projects.path().join("-locked");
        fs::create_dir_all(&locked).unwrap();
        if !lock_dir(&locked) {
            unlock_dir(&locked);
            return;
        }

        let roots = vec![
            UserProjects {
                user: "me".to_string(),
                projects_dir: self_projects.path().to_path_buf(),
                is_self: true,
            },
            UserProjects {
                user: "other".to_string(),
                projects_dir: sibling_projects.path().to_path_buf(),
                is_self: false,
            },
        ];
        // Capture first, unlock second, assert last: a panic before the unlock
        // would strand a 0o000 directory that defeats the tempdir's cleanup.
        let result = resolve_status_report(&roots, sessions.path(), SAMPLE_ID, |_| false);
        unlock_dir(&locked);

        match result {
            Err(StatusError::SessionNotFound { skipped }) => assert_eq!(
                skipped,
                vec![SkippedDir {
                    user: "other".to_string(),
                    dir: locked,
                }],
                "the miss must record the owner-only dir against the user who owns it"
            ),
            other => panic!("expected SessionNotFound with a skip, got {other:?}"),
        }
    }

    #[test]
    fn resolve_status_report_finds_a_session_in_a_sibling_root() {
        // The id lives only under a *sibling* root; the current user's own tree
        // (root zero) is empty. `--status <id>` must fall through to the sibling
        // and report its state — reading it in place, never copying or forking.
        let self_projects = tempdir().unwrap();
        let sibling_projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        write_waiting_transcript(sibling_projects.path(), SAMPLE_ID);

        let roots = vec![
            UserProjects {
                user: "me".to_string(),
                projects_dir: self_projects.path().to_path_buf(),
                is_self: true,
            },
            UserProjects {
                user: "other".to_string(),
                projects_dir: sibling_projects.path().to_path_buf(),
                is_self: false,
            },
        ];

        let report = resolve_status_report(&roots, sessions.path(), SAMPLE_ID, |_| false).unwrap();
        assert_eq!(report.session_id, SAMPLE_ID);
        assert_eq!(report.state, "waiting-for-user");
    }

    #[test]
    fn cli_json_requires_status() {
        use clap::Parser;
        // --json without --status is rejected.
        assert!(Cli::try_parse_from(["crap", "--json", SAMPLE_ID]).is_err());
    }

    #[test]
    fn cli_status_json_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["crap", "--status", "--json"]).expect("should parse");
        assert!(cli.status && cli.json);
    }

    #[test]
    fn cli_allows_status_without_session_id() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["crap", "--status"]).expect("--status with no id should parse");
        assert!(cli.status);
        assert!(cli.session_id.is_none());
    }

    #[test]
    fn cli_still_requires_session_id_without_flags() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["crap"]).is_err());
    }

    #[test]
    fn cli_user_parses_and_carries_value() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["crap", SAMPLE_ID, "--user", "scyloswork"]).expect("should parse");
        assert_eq!(cli.session_id.as_deref(), Some(SAMPLE_ID));
        assert_eq!(cli.user.as_deref(), Some("scyloswork"));
    }

    #[test]
    fn cli_user_rejected_with_shell_setup() {
        use clap::Parser;
        // Cross-user discovery makes no sense while installing the shell
        // function, so the two flags must be mutually exclusive.
        assert!(Cli::try_parse_from(["crap", "--shell-setup", "--user", "x"]).is_err());
    }

    #[test]
    fn cli_bare_user_is_a_parse_error() {
        use clap::Parser;
        // `--user` requires a NAME; a bare flag is rejected.
        assert!(Cli::try_parse_from(["crap", SAMPLE_ID, "--user"]).is_err());
    }

    /// Extracts every program literal handed to a process spawn in `source` —
    /// the string inside each `Command::new(…)` call, however the type is
    /// qualified at the call site.
    ///
    /// The needle is assembled at runtime rather than written as one literal so
    /// that this function's own text cannot satisfy the scan. Spelled out, the
    /// guard would find itself in every file it reads, and a matcher that
    /// reports its own source is indistinguishable from one that reports
    /// nothing.
    ///
    /// Splitting on the needle and reading characters up to the closing quote
    /// keeps this free of byte offsets entirely — the workspace denies
    /// `clippy::string_slice`, and slicing a source file (which may hold
    /// multi-byte characters anywhere) is exactly the panic that lint exists to
    /// prevent.
    fn spawned_programs(source: &str) -> Vec<String> {
        let needle = ["Command", "::new(\""].concat();
        source
            .split(&needle)
            // The first chunk is whatever preceded the first spawn, so it never
            // begins with a program name.
            .skip(1)
            .map(|rest| rest.chars().take_while(|c| *c != '"').collect())
            .collect()
    }

    /// The privilege-escalation binaries `crap` must never run.
    ///
    /// Assembled at runtime for the same reason as the spawn needle: the
    /// guard's vocabulary stays out of the text the guard reads, so no check
    /// here can ever be satisfied — or defeated — by this test's own source.
    fn escalation_binaries() -> Vec<String> {
        vec![
            ["su", "do"].concat(),
            ["s", "u"].concat(),
            ["do", "as"].concat(),
            ["pk", "exec"].concat(),
            ["run", "0"].concat(),
        ]
    }

    /// Every escalation binary named as a bare word in `shell`.
    ///
    /// Words, not substrings: `--resume` and `sure` both contain `su`, and a
    /// naive `contains` would report the shell function's own `claude --resume`
    /// as an escalation. Splitting on everything that cannot be part of a
    /// command name also catches path-qualified forms, since `/usr/bin/sudo`
    /// tokenises to `usr`, `bin`, `sudo`.
    fn shell_escalations(shell: &str, escalators: &[String]) -> Vec<String> {
        shell
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|word| escalators.iter().any(|e| e == word))
            .map(String::from)
            .collect()
    }

    /// `crap`'s whole answer to a session it cannot read — another account's
    /// `0o700` project directory — is *detect and guide, never escalate*. It
    /// names the owning account and prints the `sudo -u <account> …` commands
    /// that would recover the transcript, for the user to read, weigh, and run
    /// themselves. It never runs one. That line is what keeps the person at the
    /// keyboard the one who authorizes the privilege, and keeps the audit trail
    /// pointing at them rather than at a tool that escalated quietly on their
    /// behalf.
    ///
    /// Nothing in the type system holds that line, and the source now carries
    /// the word `sudo` in guidance text, so a future edit could slide from
    /// *printing* a command to *running* one without anyone noticing. This test
    /// is the enforcement point. Every program the binary spawns must appear in
    /// `ALLOWED` below — `ps`, the liveness probe in `pid_is_alive`, and `bash`,
    /// which the shell-integration tests use to source the real function — so
    /// adding a spawn is a deliberate act that has to edit this list, in a diff
    /// a reviewer will see. None of them may be an escalation binary. The shell
    /// function `--shell-setup` writes into the user's rc file is held to the
    /// same rule: an escalation there would be `crap` escalating just as surely
    /// as spawning one, only with the user's own shell doing it.
    ///
    /// Both matchers are mutation-tested against synthetic violations before
    /// they are pointed at the real thing, because a guard that has never been
    /// shown to fire is indistinguishable from one that is broken.
    #[test]
    fn crap_never_escalates_privilege_itself() {
        let escalators = escalation_binaries();
        let escalator = escalators[0].clone();

        // Mutation test 1: a source snippet that really does spawn an
        // escalation binary must be extracted and classified as one.
        let needle = ["Command", "::new(\""].concat();
        let violating_source = format!("fn oops() {{ let _ = {needle}{escalator}\").status(); }}");
        let caught = spawned_programs(&violating_source);
        assert_eq!(
            caught,
            vec![escalator.clone()],
            "the spawn matcher must extract the program from a known violation"
        );
        assert!(
            escalators.contains(&caught[0]),
            "a known violation must classify as an escalation binary"
        );

        // Mutation test 2: the same for shell text, which is scanned by words.
        let violating_shell = format!("function crap() {{ {escalator} -u someone claude; }}");
        assert_eq!(
            shell_escalations(&violating_shell, &escalators),
            vec![escalator],
            "the shell matcher must flag a known violation"
        );

        // The real source. `include_str!` is relative to this file, so this is
        // the very text being compiled.
        let programs = spawned_programs(include_str!("main.rs"));
        assert!(
            !programs.is_empty(),
            "the scan found no spawns at all — `crap` does spawn `ps`, so the matcher is broken"
        );

        const ALLOWED: [&str; 2] = ["ps", "bash"];
        for program in &programs {
            assert!(
                ALLOWED.contains(&program.as_str()),
                "`crap` spawns `{program}`, which is not in the allowlist {ALLOWED:?}; \
                 every new spawn must be added here deliberately"
            );
            assert!(
                !escalators.contains(program),
                "`crap` spawns `{program}`, a privilege-escalation binary; \
                 `crap` detects and guides, it never escalates itself"
            );
        }

        // The shell function installed by `--shell-setup` runs in the user's
        // own shell, so it is held to the same rule.
        assert!(
            shell_escalations(SHELL_CODE, &escalators).is_empty(),
            "the shell function installed by --shell-setup names an escalation binary: {:?}",
            shell_escalations(SHELL_CODE, &escalators)
        );
    }
}
