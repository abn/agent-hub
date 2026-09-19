//! A directory of a test's own.
//!
//! Tests run in parallel, as threads inside one binary and as several binaries
//! at once, so a directory name built from the clock alone can be handed to two
//! of them. The name here carries the process id and a counter every thread of
//! the process shares, and the directory is created with a call that refuses a
//! name already on disk, so no test is ever given a directory that another
//! test, or an earlier run that was killed, has written into.

use std::io::ErrorKind;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A fresh directory, removed when the value is dropped.
///
/// Hold it for as long as anything uses the directory. A struct that also
/// holds a child process working there declares the child first, so the child
/// is stopped and reaped before its directory goes.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create a directory named after `tag`, which says whose it is when one
    /// is left behind by a run that was killed.
    pub fn new(tag: &str) -> Self {
        let root = std::env::temp_dir();
        loop {
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!("agent-hub-{tag}-{}-{unique}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self { path },
                // A killed run with this process id left it. It is not ours to
                // reuse or to remove: take the next name.
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(err) => panic!("create temp dir {}: {err}", path.display()),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The directory reads as its path: `&dir` goes wherever a `&Path` does.
impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.path) {
            Ok(()) => {}
            // The test removed it itself, which some do on purpose.
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            // A failure to remove means something still writes here, which is
            // a harness bug worth a red test. While the test is already
            // failing, its own panic is the one to read, so this only notes.
            Err(err) if std::thread::panicking() => {
                eprintln!("temp dir {} was not removed: {err}", self.path.display());
            }
            Err(err) => panic!("remove temp dir {}: {err}", self.path.display()),
        }
    }
}
