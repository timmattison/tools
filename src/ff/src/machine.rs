//! The facts of this machine that a search of the whole machine needs.

use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::plan::{plan_machine, Firmlinks, MachinePlan, Mount};

/// The file that maps each firmlink of macOS to its directory on the data
/// volume. The plan reads it at each run, because each release of macOS can
/// change it.
pub const FIRMLINKS_FILE: &str = "/usr/share/firmlinks";

/// The count of the volumes that the buffer of `getfsstat` holds more than
/// the count that the first call gave. A volume can be mounted between the
/// two calls.
const SPARE_ENTRIES: usize = 8;

/// The count of the reads of the mount table before [`mount_table`] keeps a
/// full buffer. A buffer that the call fills to its end can miss a volume
/// that was mounted after the count.
const MOUNT_TABLE_TRIES: usize = 10;

/// Reads the mount table of this machine through `getfsstat`.
///
/// It asks with `MNT_NOWAIT`, thus a network volume that does not answer does
/// not stop the read. It does an interrupted call again.
///
/// # Errors
///
/// Returns the error of `getfsstat`.
pub fn mount_table() -> io::Result<Vec<Mount>> {
    let mut entries = Vec::new();
    for _ in 0..MOUNT_TABLE_TRIES {
        let count = interrupted_again(|| {
            // SAFETY: a null buffer of size 0 asks only for the count.
            unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) }
        })?;
        let capacity = count + SPARE_ENTRIES;
        let bytes = libc::c_int::try_from(capacity * std::mem::size_of::<libc::statfs>())
            .map_err(|_| io::Error::other("the mount table is too large to read"))?;
        entries = Vec::<libc::statfs>::with_capacity(capacity);
        let filled = interrupted_again(|| {
            // SAFETY: the buffer has room for `capacity` structs, and `bytes`
            // is the size of that room.
            unsafe { libc::getfsstat(entries.as_mut_ptr(), bytes, libc::MNT_NOWAIT) }
        })?;
        // SAFETY: `getfsstat` wrote `filled` structs, and `filled` is not
        // more than the room it was given.
        unsafe {
            entries.set_len(filled.min(capacity));
        }
        if filled < capacity {
            break;
        }
    }
    Ok(entries
        .iter()
        .map(|entry| Mount {
            mount_point: PathBuf::from(OsStr::from_bytes(&c_bytes(&entry.f_mntonname))),
            fs_type: String::from_utf8_lossy(&c_bytes(&entry.f_fstypename)).into_owned(),
        })
        .collect())
}

/// Does `call` again while it fails with `EINTR`, and gives its count.
fn interrupted_again(mut call: impl FnMut() -> libc::c_int) -> io::Result<usize> {
    loop {
        let result = call();
        if let Ok(count) = usize::try_from(result) {
            return Ok(count);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// The bytes of a C string in a fixed array, up to its NUL.
fn c_bytes(chars: &[libc::c_char]) -> Vec<u8> {
    chars
        .iter()
        .take_while(|&&char| char != 0)
        .map(|char| u8::from_ne_bytes(char.to_ne_bytes()))
        .collect()
}

/// Plans a search of this machine, from its mount table and from
/// [`FIRMLINKS_FILE`].
///
/// A machine with no firmlink map has no firmlinks, and the plan then walks
/// the data volume whole.
///
/// # Errors
///
/// Returns the error of `getfsstat`, or an error other than "not found" from
/// the read of the firmlink map.
pub fn plan_this_machine() -> io::Result<MachinePlan> {
    let mounts = mount_table()?;
    let firmlinks = match std::fs::read_to_string(FIRMLINKS_FILE) {
        Ok(text) => Firmlinks::parse(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Firmlinks::default(),
        Err(error) => return Err(error),
    };
    Ok(plan_machine(Path::new("/"), &mounts, &firmlinks))
}
