//! Filesystem abstraction for the subset of operations miseo actually needs.

pub use camino::{Utf8Path as Path, Utf8PathBuf as PathBuf};

use crate::error::Error;

// TODO: we made this a trait so we can have a stateful test impl, but so far
// we haven't needed it yet. If it's just to abstract between platforms it can
// probably just be a module.

pub trait Fs {
    /// Return whether `path` is executable for the current user/context (`test -x`).
    fn is_executable(&self, path: &Path) -> Result<bool, Error>;

    /// List direct child paths in `dir`.
    fn ls(&self, dir: &Path) -> Result<Vec<PathBuf>, Error>;

    /// Ensure `path` exists as a directory (create parents if needed).
    fn mkdir_p(&self, path: &Path) -> Result<(), Error>;

    /// Read symlink target for `path`, returning `None` if not a symlink.
    fn readlink(&self, path: &Path) -> Result<Option<PathBuf>, Error>;

    /// Atomically replace a file/symlink at `link_path` with a symlink to `target`
    /// (rename within the same parent directory).
    ///
    /// If `link_path` exists as a directory, this returns an error.
    fn ln_s(&self, target: &Path, link_path: &Path) -> Result<(), Error>;

    /// Remove a file/symlink at `path` if present.
    fn rm(&self, path: &Path) -> Result<(), Error>;

    /// Remove a directory tree at `path` if present.
    fn rm_rf(&self, path: &Path) -> Result<(), Error>;

    /// Write UTF-8 file content to `path`, replacing existing file content.
    fn write_file(&self, path: &Path, content: &str) -> Result<(), Error>;

    /// Return whether any filesystem entry exists at `path`.
    #[cfg(test)]
    fn exists(&self, path: &Path) -> Result<bool, Error>;

    /// Read UTF-8 file content from `path`.
    #[cfg(test)]
    fn read_file(&self, path: &Path) -> Result<String, Error>;

    /// Write UTF-8 file content to `path` and mark it executable.
    #[cfg(test)]
    fn write_executable_file(&self, path: &Path, content: &str) -> Result<(), Error>;

    /// Write an executable shim that activates the tool-local mise env then executes `target`.
    fn write_mise_env_shim(
        &self,
        project_dir: &Path,
        target: &Path,
        path: &Path,
    ) -> Result<(), Error>;
}

#[cfg(unix)]
mod unix;

#[cfg(windows)]
mod windows;

#[cfg(all(not(unix), not(windows)))]
compile_error!("miseo only supports unix and windows targets");

#[cfg(unix)]
static DEFAULT_FS: unix::UnixFs = unix::UnixFs;

#[cfg(windows)]
static DEFAULT_FS: windows::WindowsFs = windows::WindowsFs;

pub fn new() -> &'static impl Fs {
    &DEFAULT_FS
}
