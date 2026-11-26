//! A file system that puts faults between the walk and the real file system.
//!
//! A rule sees each call before the real call. When the rule gives a fault,
//! the call gives that fault and does not reach the real file system. This is
//! what the kernel does with an interrupted call: the call does nothing and
//! gives `EINTR`, thus the same call can be done again.

#![allow(
    dead_code,
    reason = "each test binary includes this module and uses a different part of it"
)]

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use ff::fs::{EntryKind, FileSystem, Link, RawEntry};

/// The call that a rule sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Call {
    /// `open_dir`, with the path of the directory.
    Open,
    /// `device`, with the path of the open directory.
    Device,
    /// `read_entry`, with the path of the open directory.
    Read,
    /// `is_directory`, with the path of the entry.
    IsDirectory,
}

/// What a rule gives in place of the real call.
#[derive(Debug)]
pub enum Fault {
    /// The call gives this error.
    Error(io::Error),
    /// A `device` call gives this device id.
    Device(u64),
}

/// Makes an error of the raw `errno` value `code`.
pub fn errno(code: i32) -> Fault {
    Fault::Error(io::Error::from_raw_os_error(code))
}

/// A file system that asks `rule` before each real call.
pub struct Faulty<F, R> {
    inner: F,
    rule: R,
}

impl<F, R> Faulty<F, R>
where
    F: FileSystem,
    R: Fn(Call, &Path) -> Option<Fault> + Sync,
{
    /// Wraps `inner`, and asks `rule` before each call.
    pub fn new(inner: F, rule: R) -> Self {
        Self { inner, rule }
    }
}

/// A directory that [`Faulty`] opened. It keeps its path for the rule.
pub struct FaultyDir<D> {
    inner: D,
    path: PathBuf,
}

impl<F, R> FileSystem for Faulty<F, R>
where
    F: FileSystem,
    R: Fn(Call, &Path) -> Option<Fault> + Sync,
{
    type Dir = FaultyDir<F::Dir>;

    fn open_dir(&self, path: &Path, link: Link) -> io::Result<Self::Dir> {
        if let Some(Fault::Error(error)) = (self.rule)(Call::Open, path) {
            return Err(error);
        }
        let inner = self.inner.open_dir(path, link)?;
        Ok(FaultyDir {
            inner,
            path: path.to_path_buf(),
        })
    }

    fn device(&self, dir: &Self::Dir) -> io::Result<u64> {
        match (self.rule)(Call::Device, &dir.path) {
            Some(Fault::Error(error)) => Err(error),
            Some(Fault::Device(device)) => Ok(device),
            None => self.inner.device(&dir.inner),
        }
    }

    fn read_entry(&self, dir: &mut Self::Dir) -> io::Result<Option<RawEntry>> {
        if let Some(Fault::Error(error)) = (self.rule)(Call::Read, &dir.path) {
            return Err(error);
        }
        self.inner.read_entry(&mut dir.inner)
    }

    fn is_directory(&self, dir: &Self::Dir, name: &OsStr) -> io::Result<bool> {
        if let Some(Fault::Error(error)) = (self.rule)(Call::IsDirectory, &dir.path.join(name)) {
            return Err(error);
        }
        self.inner.is_directory(&dir.inner, name)
    }
}

/// A file system whose directory entries give no type, as on some network
/// file systems. The walk must then ask for the type of each entry.
pub struct UnknownTypes<F>(pub F);

impl<F: FileSystem> FileSystem for UnknownTypes<F> {
    type Dir = F::Dir;

    fn open_dir(&self, path: &Path, link: Link) -> io::Result<Self::Dir> {
        self.0.open_dir(path, link)
    }

    fn device(&self, dir: &Self::Dir) -> io::Result<u64> {
        self.0.device(dir)
    }

    fn read_entry(&self, dir: &mut Self::Dir) -> io::Result<Option<RawEntry>> {
        Ok(self.0.read_entry(dir)?.map(|entry| RawEntry {
            kind: EntryKind::Unknown,
            ..entry
        }))
    }

    fn is_directory(&self, dir: &Self::Dir, name: &OsStr) -> io::Result<bool> {
        self.0.is_directory(dir, name)
    }
}
