//! The gzip-aware JSON load and the atomic JSON save that a recording and an
//! edit project share.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::NleError;

/// Load a value from a JSON file, auto-detecting gzip compression via magic bytes.
///
/// `noun` names the kind of file (`"recording"` or `"project"`) in the messages.
///
/// # Errors
/// Returns `NleError::LoadError` when the file cannot be opened or read,
/// and `NleError::InvalidFormat` when the file is empty or does not parse
/// as a `T`.
pub(crate) fn load<T: DeserializeOwned>(path: &Path, noun: &str) -> Result<T, NleError> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        NleError::LoadError(format!(
            "Failed to open {noun} file {}: {e}",
            path.display()
        ))
    })?;

    if file
        .metadata()
        .map_err(|e| NleError::LoadError(e.to_string()))?
        .len()
        == 0
    {
        return Err(NleError::InvalidFormat(format!("The {noun} file is empty")));
    }

    // Check for gzip magic bytes (0x1f 0x8b) instead of relying on extension
    let mut magic = [0_u8; 2];
    let is_gzip = file.read_exact(&mut magic).is_ok() && magic == [0x1f, 0x8b];

    // Seek back to the beginning for the actual read
    file.seek(SeekFrom::Start(0))
        .map_err(|e| NleError::LoadError(e.to_string()))?;
    let reader = std::io::BufReader::new(file);

    if is_gzip {
        let decoder = flate2::read::GzDecoder::new(reader);
        serde_json::from_reader(decoder)
            .map_err(|e| NleError::InvalidFormat(format!("Failed to parse compressed {noun}: {e}")))
    } else {
        serde_json::from_reader(reader)
            .map_err(|e| NleError::InvalidFormat(format!("Failed to parse {noun}: {e}")))
    }
}

/// Save a value as JSON to a file, optionally using gzip compression.
///
/// `noun` names the kind of file (`"recording"` or `"project"`) in the messages.
///
/// The data goes to a temp file in the directory of `path`. The temp file
/// is synced, given the mode of the old file, and renamed over `path`, so
/// a save that fails leaves the old file as it was. A save to a new path
/// gives the mode that `File::create` gives.
///
/// # Errors
/// Returns `NleError::SaveError` when the value cannot be serialized,
/// compressed, or written.
pub(crate) fn save<T: Serialize>(
    value: &T,
    path: &Path,
    compress: bool,
    noun: &str,
) -> Result<(), NleError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| NleError::SaveError(format!("Failed to serialize {noun}: {e}")))?;

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = create_temp_file(dir).map_err(|e| {
        NleError::SaveError(format!(
            "Failed to create temp file for {}: {e}",
            path.display()
        ))
    })?;

    if compress {
        let mut encoder =
            flate2::write::GzEncoder::new(temp.as_file(), flate2::Compression::default());
        encoder.write_all(json.as_bytes()).map_err(|e| {
            NleError::SaveError(format!("Failed to write gzip file {}: {e}", path.display()))
        })?;
        encoder.finish().map_err(|e| {
            NleError::SaveError(format!(
                "Failed to finish gzip file {}: {e}",
                path.display()
            ))
        })?;
    } else {
        temp.write_all(json.as_bytes()).map_err(|e| {
            NleError::SaveError(format!("Failed to write file {}: {e}", path.display()))
        })?;
    }

    temp.as_file()
        .sync_all()
        .map_err(|e| NleError::SaveError(format!("Failed to sync file {}: {e}", path.display())))?;

    if let Ok(old) = std::fs::metadata(path) {
        temp.as_file()
            .set_permissions(old.permissions())
            .map_err(|e| {
                NleError::SaveError(format!(
                    "Failed to copy permissions to {}: {e}",
                    path.display()
                ))
            })?;
    }

    temp.persist(path).map_err(|e| {
        NleError::SaveError(format!(
            "Failed to replace file {}: {}",
            path.display(),
            e.error
        ))
    })?;

    Ok(())
}

/// Create the temp file of a save in `dir`.
///
/// On Unix the temp file asks open(2) for mode 0o666, and the umask reduces
/// that mode, as it does for `File::create`. The default of `tempfile` is
/// 0o600, which gives an owner-only file on the first save to a path.
fn create_temp_file(dir: &Path) -> std::io::Result<tempfile::NamedTempFile> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o666))
            .tempfile_in(dir)
    }
    #[cfg(not(unix))]
    {
        tempfile::NamedTempFile::new_in(dir)
    }
}
