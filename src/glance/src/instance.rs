//! The one status item of each target: the name of the lock that marks it.
//!
//! A [`Target`] is the Mac whose values a status item shows: this Mac, or the
//! destination of `--host`. Each target has its own lock file, so one status
//! item of each target can run, and a stop of one target leaves the others
//! alone. The lock itself, and the calls to the kernel that detach the status
//! item from the terminal and that stop it, are in
//! [`soleinstance::instance`].

use std::io;
use std::path::Path;

use soleinstance::instance::LockFile;

use crate::connection::Destination;

/// The name of the directory of `glance` in the data directory of the user.
const DIRECTORY_NAME: &str = "glance";

/// The name of the lock file of the local status item.
const LOCAL_FILE_NAME: &str = "local.lock";

/// The start of the name of the lock file of a host.
const HOST_FILE_PREFIX: &str = "host-";

/// The end of the name of each lock file.
const LOCK_FILE_SUFFIX: &str = ".lock";

/// The Mac whose values a status item shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// This Mac.
    Local,
    /// The second Mac at this `--host` destination, as the user typed it.
    Host(Destination),
}

impl Target {
    /// Gives the name of the lock file of this target.
    ///
    /// The local target gives `local.lock`. A host gives `host-`, the
    /// destination, and `.lock`. Each byte of the destination that is not a
    /// lowercase ASCII letter, a digit, `.`, `-`, `_`, or `@` becomes `%` and
    /// two uppercase hex digits. Thus the name holds no `/`, and two different
    /// destinations never give the same name. The default file system of macOS
    /// folds case, so a raw uppercase letter could name the same file as its
    /// lowercase form.
    #[must_use]
    pub fn file_name(&self) -> String {
        match self {
            Target::Local => LOCAL_FILE_NAME.to_owned(),
            Target::Host(destination) => {
                let mut name = HOST_FILE_PREFIX.to_owned();
                for byte in destination.as_str().bytes() {
                    if byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_@".contains(&byte)
                    {
                        name.push(char::from(byte));
                    } else {
                        name.push_str(&format!("%{byte:02X}"));
                    }
                }
                name.push_str(LOCK_FILE_SUFFIX);
                name
            }
        }
    }
}

/// Gives the lock file of `target` below the data directory `data`:
/// `glance/` and the [file name](Target::file_name) of the target.
#[must_use]
pub fn lock_file(data: &Path, target: &Target) -> LockFile {
    LockFile::in_data_directory(data, DIRECTORY_NAME, &target.file_name())
}

/// Gives the lock file of `target` for the current user. On macOS the lock
/// file of this Mac is `~/Library/Application Support/glance/local.lock`.
///
/// # Errors
///
/// Returns an error of kind [`io::ErrorKind::NotFound`] when the data
/// directory of the user is not known.
pub fn lock_file_for_user(target: &Target) -> io::Result<LockFile> {
    LockFile::for_user(DIRECTORY_NAME, &target.file_name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A data directory for one test.
    fn data_directory() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("glance-data-")
            .tempdir()
            .expect("make a data directory")
    }

    /// The target of the host at `destination`.
    fn host(destination: &str) -> Target {
        Target::Host(destination.parse().expect("a destination"))
    }

    #[test]
    fn the_local_lock_file_is_in_the_glance_directory_of_the_data_directory() {
        let lock = lock_file(Path::new("/data"), &Target::Local);
        assert_eq!(lock.path(), Path::new("/data/glance/local.lock"));
    }

    #[test]
    fn the_lock_file_of_a_host_is_beside_the_local_lock_file() {
        let lock = lock_file(Path::new("/data"), &host("m1-ts"));
        assert_eq!(lock.path(), Path::new("/data/glance/host-m1-ts.lock"));
    }

    #[test]
    fn the_lock_file_of_the_user_is_below_the_data_directory_of_the_user() {
        let data = dirs::data_dir().expect("the data directory of this user");
        let target = host("tim@m1-ts");
        let lock = lock_file_for_user(&target).expect("the lock file of this user");
        assert_eq!(lock, lock_file(&data, &target));
    }

    #[test]
    fn a_host_name_keeps_lowercase_letters_digits_and_the_marks_of_a_host() {
        assert_eq!(
            host("tim@m4-pro_mac.local").file_name(),
            "host-tim@m4-pro_mac.local.lock"
        );
        assert_eq!(
            host("tim@M4-Pro_Mac.local").file_name(),
            "host-tim@%4D4-%50ro_%4Dac.local.lock"
        );
    }

    #[test]
    fn a_host_name_escapes_each_other_byte() {
        assert_eq!(
            host("ssh://h:22").file_name(),
            "host-ssh%3A%2F%2Fh%3A22.lock"
        );
        assert_eq!(host("a b").file_name(), "host-a%20b.lock");
        assert_eq!(host("café").file_name(), "host-caf%C3%A9.lock");
        assert_eq!(host("日本").file_name(), "host-%E6%97%A5%E6%9C%AC.lock");
        assert_eq!(host("🎉").file_name(), "host-%F0%9F%8E%89.lock");
    }

    #[test]
    fn a_host_name_never_holds_a_slash() {
        let name = host("../../etc/passwd").file_name();
        assert!(!name.contains('/'), "{name}");
        assert_eq!(name, "host-..%2F..%2Fetc%2Fpasswd.lock");
    }

    #[test]
    fn two_destinations_that_differ_only_in_case_give_names_that_differ_after_case_folding() {
        for (a, b) in [("Alpha", "alpha"), ("Tim@m1", "tim@m1"), ("ALPHA", "alpha")] {
            // A case-folding file system compares the names in lowercase.
            assert_ne!(
                host(a).file_name().to_ascii_lowercase(),
                host(b).file_name().to_ascii_lowercase(),
                "{a} and {b}"
            );
        }
    }

    #[test]
    fn two_destinations_that_differ_only_in_case_make_two_lock_files() {
        let data = data_directory();
        for destination in ["Alpha", "alpha"] {
            lock_file(data.path(), &host(destination))
                .prepare()
                .expect("prepare the lock file");
        }
        // On a case-folding volume (the macOS default), the old names give one file.
        let files = fs::read_dir(data.path().join(DIRECTORY_NAME))
            .expect("read the glance directory")
            .count();
        assert_eq!(files, 2);
    }

    #[test]
    fn two_different_destinations_give_two_different_names() {
        assert_ne!(host("a/b").file_name(), host("a%2Fb").file_name());
        assert_ne!(host("local").file_name(), Target::Local.file_name());
    }
}
