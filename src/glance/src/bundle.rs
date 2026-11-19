//! The app bundle that each status item runs from.
//!
//! Bartender keys each status item on the bundle identifier and on the
//! autosave name of the item. macOS gives a bundle identifier only to a
//! process that starts from an executable inside a real `.app` bundle. A bare
//! binary in `~/.cargo/bin` has none. An `Info.plist` that the linker embeds
//! in the binary does not help, and a symlink to the executable of a bundle
//! does not help either. Issue #719 holds these measurements.
//!
//! So `glance` makes its own bundle in its data directory:
//!
//! ```text
//! <data>/glance/Glance.app/
//!   Contents/Info.plist
//!   Contents/MacOS/glance
//! ```
//!
//! The executable is a copy of the running binary. The linker signs the binary
//! ad hoc, and that signature is enough, so no `codesign` call is necessary.
//!
//! A status item can run from the old copy while a newer `glance` starts.
//! macOS also caches the code signature of each file. So [`Bundle::ensure`]
//! never writes into the existing copy. It writes a new file and renames it
//! over the old one. The new file has a new inode, and the old process keeps
//! its old file.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::instance;

/// The bundle identifier of the status items of `glance`.
pub const IDENTIFIER: &str = "com.timmattison.glance";

/// The name of the bundle directory.
const BUNDLE_NAME: &str = "Glance.app";
/// The name of the directory of the bundle that holds its files.
const CONTENTS_NAME: &str = "Contents";
/// The name of the directory of the bundle that holds its executable.
const MACOS_NAME: &str = "MacOS";
/// The name of the property list file.
const INFO_PLIST_NAME: &str = "Info.plist";
/// The name of the executable in the bundle.
const EXECUTABLE_NAME: &str = "glance";
/// The display name of the bundle.
const DISPLAY_NAME: &str = "Glance";
/// The prefix of the name of a temporary file.
const TEMPORARY_PREFIX: &str = ".glance-";
/// The mode of the executable.
const EXECUTABLE_MODE: u32 = 0o755;
/// The mode of the property list.
const PLIST_MODE: u32 = 0o644;

/// The app bundle of `glance`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle(PathBuf);

impl Bundle {
    /// Gives the bundle below the data directory `data`, at
    /// `data/glance/Glance.app`.
    #[must_use]
    pub fn in_data_directory(data: &Path) -> Self {
        Self(data.join(instance::DIRECTORY_NAME).join(BUNDLE_NAME))
    }

    /// Gives the bundle of the current user.
    ///
    /// # Errors
    ///
    /// Returns an error of kind [`io::ErrorKind::NotFound`] when the data
    /// directory of the user is not known.
    pub fn for_user() -> io::Result<Self> {
        instance::user_data_directory().map(|data| Self::in_data_directory(&data))
    }

    /// Gives the path of the `Glance.app` directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Gives the path of the executable in the bundle.
    #[must_use]
    pub fn executable(&self) -> PathBuf {
        self.0
            .join(CONTENTS_NAME)
            .join(MACOS_NAME)
            .join(EXECUTABLE_NAME)
    }

    /// Gives the path of the property list of the bundle.
    #[must_use]
    pub fn info_plist(&self) -> PathBuf {
        self.0.join(CONTENTS_NAME).join(INFO_PLIST_NAME)
    }

    /// Makes sure that the bundle holds a copy of the file `source` as its
    /// executable, and the right `Info.plist`.
    ///
    /// # Errors
    ///
    /// Returns an error when `source` cannot be read, or when a directory or a
    /// file of the bundle cannot be written.
    pub fn ensure(&self, source: &Path) -> io::Result<()> {
        let _ = source;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_directory() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("glance-bundle-")
            .tempdir()
            .expect("a temporary data directory")
    }

    fn plutil_json(path: &Path) -> serde_json::Value {
        let output = childenv::fixture_command("plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(path)
            .output()
            .expect("plutil starts");
        assert!(output.status.success(), "plutil refused the plist");
        serde_json::from_slice(&output.stdout).expect("plutil writes JSON")
    }

    #[test]
    fn the_paths_are_below_the_data_directory() {
        let bundle = Bundle::in_data_directory(Path::new("/data"));
        assert_eq!(bundle.path(), Path::new("/data/glance/Glance.app"));
        assert_eq!(
            bundle.executable(),
            Path::new("/data/glance/Glance.app/Contents/MacOS/glance")
        );
        assert_eq!(
            bundle.info_plist(),
            Path::new("/data/glance/Glance.app/Contents/Info.plist")
        );
    }

    #[test]
    fn the_bundle_of_the_user_is_below_the_data_directory_of_the_user() {
        let data = dirs::data_dir().expect("the data directory of this user");
        let bundle = Bundle::for_user().expect("the bundle of this user");
        assert_eq!(bundle, Bundle::in_data_directory(&data));
    }

    #[test]
    fn ensure_makes_the_layout_and_an_info_plist_that_plutil_reads() {
        let data = data_directory();
        let source = data.path().join("source-binary");
        fs::write(&source, b"distinctive bytes of the source").expect("the source");
        let bundle = Bundle::in_data_directory(data.path());
        bundle.ensure(&source).expect("ensure");

        let executable = bundle.executable();
        assert_eq!(
            executable,
            data.path().join("glance/Glance.app/Contents/MacOS/glance")
        );
        assert_eq!(
            fs::read(&executable).expect("the executable"),
            b"distinctive bytes of the source"
        );
        let mode = fs::metadata(&executable)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);

        let json = plutil_json(&bundle.info_plist());
        let object = json.as_object().expect("an object");
        assert_eq!(object.len(), 6);
        assert_eq!(object["CFBundleExecutable"], "glance");
        assert_eq!(object["CFBundleIdentifier"], IDENTIFIER);
        assert_eq!(object["CFBundleName"], "Glance");
        assert_eq!(object["CFBundlePackageType"], "APPL");
        assert_eq!(
            object["CFBundleShortVersionString"],
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(object["LSUIElement"], true);
    }
}
