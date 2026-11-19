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

    /// Tells if `program` is the executable of this bundle.
    ///
    /// The function compares the real paths of both files, so a path through
    /// a symbolic link counts. A file that does not exist is not the
    /// executable of the bundle. A foreground run uses this function to avoid
    /// a second `exec` of the same file.
    #[must_use]
    pub fn runs(&self, program: &Path) -> bool {
        match (
            fs::canonicalize(program),
            fs::canonicalize(self.executable()),
        ) {
            (Ok(program), Ok(executable)) => program == executable,
            _ => false,
        }
    }

    /// Makes sure that the bundle holds a copy of the file `source` as its
    /// executable, and the right `Info.plist`.
    ///
    /// # Errors
    ///
    /// Returns an error when `source` cannot be read, or when a directory or a
    /// file of the bundle cannot be written.
    pub fn ensure(&self, source: &Path) -> io::Result<()> {
        let executable = self.executable();
        let bytes = fs::read(source)?;
        if !holds(&executable, &bytes) {
            replace_file(&executable, &bytes, EXECUTABLE_MODE)?;
        }
        let plist = self.info_plist();
        let text = info_plist_text();
        if !holds(&plist, text.as_bytes()) {
            replace_file(&plist, text.as_bytes(), PLIST_MODE)?;
        }
        Ok(())
    }
}

/// Tells if the file `path` exists and holds exactly `bytes`. A file of
/// another length differs, so the function does not read it.
fn holds(path: &Path, bytes: &[u8]) -> bool {
    let same_length = fs::metadata(path)
        .is_ok_and(|metadata| usize::try_from(metadata.len()).is_ok_and(|n| n == bytes.len()));
    same_length && fs::read(path).is_ok_and(|current| current == bytes)
}

/// Writes `bytes` to a new temporary file next to `target`, sets its mode, and
/// renames it over `target`. The new file has a new inode. It makes the
/// directory of `target` when it is missing.
///
/// # Errors
///
/// Returns an error when `target` has no directory, or when a file cannot be
/// written.
fn replace_file(target: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let directory = target.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "the target has no directory")
    })?;
    fs::create_dir_all(directory)?;
    let mut file = tempfile::Builder::new()
        .prefix(TEMPORARY_PREFIX)
        .tempfile_in(directory)?;
    io::Write::write_all(&mut file, bytes)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    file.persist(target).map_err(|error| error.error)?;
    Ok(())
}

/// Gives the text of the `Info.plist` of the bundle.
fn info_plist_text() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key>
	<string>{EXECUTABLE_NAME}</string>
	<key>CFBundleIdentifier</key>
	<string>{IDENTIFIER}</string>
	<key>CFBundleName</key>
	<string>{DISPLAY_NAME}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>LSUIElement</key>
	<true/>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

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

    #[test]
    fn a_second_call_with_the_same_source_writes_nothing() {
        let data = data_directory();
        let source = data.path().join("source-binary");
        fs::write(&source, b"distinctive bytes of the source").expect("the source");
        let bundle = Bundle::in_data_directory(data.path());
        bundle.ensure(&source).expect("first ensure");
        let executable = fs::metadata(bundle.executable()).expect("executable").ino();
        let plist = fs::metadata(bundle.info_plist()).expect("plist").ino();
        bundle.ensure(&source).expect("second ensure");
        assert_eq!(
            fs::metadata(bundle.executable()).expect("executable").ino(),
            executable
        );
        assert_eq!(
            fs::metadata(bundle.info_plist()).expect("plist").ino(),
            plist
        );
    }

    #[test]
    fn a_call_with_a_different_source_replaces_the_executable() {
        let data = data_directory();
        let source_a = data.path().join("source-a");
        let source_b = data.path().join("source-b");
        fs::write(&source_a, b"bytes of source A").expect("source A");
        fs::write(&source_b, b"other, longer bytes of source B").expect("source B");
        let bundle = Bundle::in_data_directory(data.path());
        bundle.ensure(&source_a).expect("ensure with A");
        let before = fs::metadata(bundle.executable()).expect("executable").ino();
        bundle.ensure(&source_b).expect("ensure with B");
        let after = fs::metadata(bundle.executable()).expect("executable");
        assert_eq!(
            fs::read(bundle.executable()).expect("the executable"),
            b"other, longer bytes of source B"
        );
        assert_ne!(after.ino(), before);
        assert_eq!(after.permissions().mode() & 0o777, 0o755);
    }

    #[test]
    fn a_changed_info_plist_is_written_again() {
        let data = data_directory();
        let source = data.path().join("source-binary");
        fs::write(&source, b"distinctive bytes of the source").expect("the source");
        let bundle = Bundle::in_data_directory(data.path());
        bundle.ensure(&source).expect("first ensure");
        let right = fs::read_to_string(bundle.info_plist()).expect("plist");
        fs::write(bundle.info_plist(), "changed by hand").expect("overwrite");
        bundle.ensure(&source).expect("second ensure");
        assert_eq!(
            fs::read_to_string(bundle.info_plist()).expect("plist"),
            right
        );
        assert_eq!(
            plutil_json(&bundle.info_plist())["CFBundleIdentifier"],
            IDENTIFIER
        );
    }

    #[test]
    fn the_executable_of_the_bundle_runs_from_the_bundle() {
        let data = data_directory();
        let source = data.path().join("source-binary");
        fs::write(&source, b"distinctive bytes of the source").expect("the source");
        let bundle = Bundle::in_data_directory(data.path());
        bundle.ensure(&source).expect("ensure");
        assert!(bundle.runs(&bundle.executable()));
    }

    #[test]
    fn the_source_of_the_bundle_does_not_run_from_the_bundle() {
        let data = data_directory();
        let source = data.path().join("source-binary");
        fs::write(&source, b"distinctive bytes of the source").expect("the source");
        let bundle = Bundle::in_data_directory(data.path());
        bundle.ensure(&source).expect("ensure");
        assert!(!bundle.runs(&source));
    }

    #[test]
    fn a_missing_bundle_runs_nothing() {
        let data = data_directory();
        let source = data.path().join("source-binary");
        fs::write(&source, b"distinctive bytes of the source").expect("the source");
        let bundle = Bundle::in_data_directory(data.path());
        assert!(!bundle.runs(&source));
        assert!(!bundle.runs(&bundle.executable()));
    }

    #[test]
    fn a_path_through_a_symbolic_link_runs_from_the_bundle() {
        let data = data_directory();
        let source = data.path().join("source-binary");
        fs::write(&source, b"distinctive bytes of the source").expect("the source");
        let bundle = Bundle::in_data_directory(data.path());
        bundle.ensure(&source).expect("ensure");
        let link = data.path().join("link-to-data");
        std::os::unix::fs::symlink(data.path(), &link).expect("the link");
        let through = link.join("glance/Glance.app/Contents/MacOS/glance");
        assert!(through.is_file());
        assert!(bundle.runs(&through));
    }
}
