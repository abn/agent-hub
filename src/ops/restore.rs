//! Restore a data directory from a verified backup, offline.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

use super::manifest::Manifest;
use super::{HUB_DB, copy_file, dir_nonempty, join_rel, open_hub_store, sha256_file};

/// What a restore did.
#[derive(Debug)]
pub struct RestoreReport {
    pub files: usize,
    pub bytes: u64,
}

/// Replace `data_dir` with the contents of the backup in `from`.
///
/// Every member is verified against the manifest before anything is written;
/// a hub holding the store, and a non-empty destination without `--force`, are
/// both refused. The copy lands in a staging directory beside the destination
/// and is moved into place, so a failure leaves the original untouched.
pub async fn restore(from: &Path, data_dir: &Path, force: bool) -> Result<RestoreReport> {
    restore_observed(from, data_dir, force, || {}).await
}

/// As [`restore`], calling `before_swap` once both locks are held and the new
/// tree is staged, just before it is moved into place. A test starts a hub
/// there to watch it wait out the swap.
#[doc(hidden)]
pub async fn restore_observed(
    from: &Path,
    data_dir: &Path,
    force: bool,
    before_swap: impl FnOnce(),
) -> Result<RestoreReport> {
    let manifest = Manifest::read(from)?;

    // Verify every source file before touching the destination: a checksum
    // mismatch must not cost the operator the data they still have.
    let mut bytes = 0u64;
    for entry in &manifest.files {
        let source = join_rel(from, &entry.path)?;
        let metadata = std::fs::metadata(&source).map_err(|err| {
            Error::NotFound(format!("the backup is missing {}: {err}", entry.path))
        })?;
        if metadata.len() != entry.size {
            return Err(Error::InvalidArgument(format!(
                "{}: size {} does not match the manifest's {}",
                entry.path,
                metadata.len(),
                entry.size
            )));
        }
        let sum = sha256_file(&source)?;
        if sum != entry.sha256 {
            return Err(Error::InvalidArgument(format!(
                "{}: sha256 does not match the manifest",
                entry.path
            )));
        }
        bytes += entry.size;
    }

    let parent = data_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    // Take the live directory's hub.lock, the same lock a hub takes before it
    // opens the engine, and keep it to the end: through the swap and the
    // cleanup. A hub that is serving holds it, so the restore refuses. A hub
    // that starts now waits on it, and once the swap is done finds the path
    // naming the new tree's lock, which this restore also holds, so it opens
    // the restored store only after the restore lets go of both. Any error
    // taking it is a refusal.
    //
    // A directory that does not exist yet is created and locked like any
    // other, so a hub starting at the same moment cannot open an empty store
    // that the restore then replaces. The directory this restore made is
    // removed again if the restore does not finish.
    //
    // On Windows the lock file never moves: see the Windows `move_into_place`.
    let mut created = CreatedDir {
        path: data_dir,
        armed: false,
    };
    match std::fs::create_dir(data_dir) {
        Ok(()) => created.armed = true,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err.into()),
    }
    let live_lock = match lock_store_file(&data_dir.join(HUB_LOCK), Duration::ZERO)? {
        Some(file) => file,
        None => return Err(super::locked()),
    };

    // A serving hub holds the engine lock over the directory being replaced.
    let hub_db = data_dir.join(HUB_DB);
    if hub_db.is_file() {
        // The handle is dropped immediately; the refusal is what matters.
        let _held = open_hub_store(&hub_db).await?;
    }

    if !force && holds_data(data_dir)? {
        return Err(Error::Conflict(format!(
            "{} is not empty; pass --force to replace it",
            data_dir.display()
        )));
    }

    // Build the whole tree beside the destination so the move is one step,
    // and both live on the same filesystem even for a mount point.
    let token = ulid::Ulid::generate();
    let stage = parent.join(format!(".agent-hub-restore-{token}"));
    let staged_lock = match fill_stage(from, &manifest, &stage).and_then(|()| lock_stage(&stage)) {
        Ok(file) => file,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(err);
        }
    };
    before_swap();
    if let Err(err) = move_into_place(&stage, data_dir, parent, &token) {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(err);
    }
    drop(staged_lock);
    drop(live_lock);
    created.armed = false;

    Ok(RestoreReport {
        files: manifest.files.len(),
        bytes,
    })
}

/// Lock the staged tree's own hub.lock, so a hub that finds it at the data
/// directory's path once the trees are swapped waits for this restore.
#[cfg(not(windows))]
fn lock_stage(stage: &Path) -> Result<Option<File>> {
    match lock_store_file(&stage.join(HUB_LOCK), Duration::ZERO)? {
        Some(file) => Ok(Some(file)),
        None => Err(super::locked()),
    }
}

/// Windows moves the staged entries in beside the live lock instead of
/// swapping the trees, so the staged tree has no lock to take.
#[cfg(windows)]
fn lock_stage(_stage: &Path) -> Result<Option<File>> {
    Ok(None)
}

/// A data directory this restore created in order to lock it. Dropped armed,
/// it takes the directory away again, after the lock on it is released.
struct CreatedDir<'a> {
    path: &'a Path,
    armed: bool,
}

impl Drop for CreatedDir<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(self.path.join(HUB_LOCK));
            let _ = std::fs::remove_dir(self.path);
        }
    }
}

/// The lock file a process that opens the store holds in the data directory.
/// It stays empty: the OS lock on it is the whole of its job.
pub const HUB_LOCK: &str = "hub.lock";

/// The record of the process holding [`HUB_LOCK`]: its pid, address and mode.
pub const HUB_RECORD: &str = "hub.pid.json";

/// Whether a top-level name in the data directory is the lock or its record.
/// Neither is data: a backup never copies them and a restore never counts them.
pub fn is_process_file(name: &OsStr) -> bool {
    name == HUB_LOCK || name == HUB_RECORD
}

/// How often a wait for a held lock tries again.
const RETRY_EVERY: Duration = Duration::from_millis(50);

/// How many times a lock may turn out to be on a replaced file before the
/// caller gives up. A restore replaces it once.
const MAX_REPLACED: usize = 8;

/// Take the exclusive lock on the lock file at `path`, creating it, and make
/// sure the locked file is still the one at `path`.
///
/// A restore swaps the directory the lock file lives in. A process that was
/// waiting on the old file gets its lock only once the restore lets go, and by
/// then the path names the new tree's file: holding the old one would let it
/// open a store another process can open too. So the lock is checked against
/// the path after it is granted, and taken again on the new file when they
/// differ.
///
/// Retries a held lock until `wait` has passed. `Ok(None)` means another
/// process held it for the whole wait.
pub fn lock_store_file(path: &Path, wait: Duration) -> std::io::Result<Option<File>> {
    let deadline = Instant::now() + wait;
    let mut replaced = 0;
    loop {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    std::thread::sleep(RETRY_EVERY);
                }
                Err(TryLockError::Error(err)) => return Err(err),
            }
        }
        if still_at(&file, path)? {
            return Ok(Some(file));
        }
        replaced += 1;
        if replaced >= MAX_REPLACED {
            return Err(std::io::Error::other(format!(
                "{} was replaced {MAX_REPLACED} times while it was being locked",
                path.display()
            )));
        }
    }
}

/// Whether `file` is the file `path` names now.
#[cfg(unix)]
fn still_at(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let held = file.metadata()?;
    match std::fs::metadata(path) {
        Ok(now) => Ok(now.dev() == held.dev() && now.ino() == held.ino()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// Whether `file` is the file `path` names now: the same volume and file id.
#[cfg(windows)]
fn still_at(file: &File, path: &Path) -> std::io::Result<bool> {
    let held = file_id(file)?;
    match File::open(path) {
        Ok(now) => Ok(file_id(&now)? == held),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// The volume serial and 128-bit file id of an open file. An error here fails
/// the lock closed: a file whose identity cannot be read is not locked.
#[cfg(windows)]
fn file_id(file: &File) -> std::io::Result<(u64, [u8; 16])> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };
    // SAFETY: the handle is valid for the borrow of `file`, and the call only
    // writes the structure it is given, whose size is passed.
    let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((info.VolumeSerialNumber, info.FileId.Identifier))
}

#[cfg(not(any(unix, windows)))]
fn still_at(_file: &File, _path: &Path) -> std::io::Result<bool> {
    Ok(true)
}

/// Whether the directory holds anything but the lock file and its record.
fn holds_data(data_dir: &Path) -> Result<bool> {
    if !dir_nonempty(data_dir)? {
        return Ok(false);
    }
    for entry in std::fs::read_dir(data_dir)? {
        if !is_process_file(&entry?.file_name()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Copy every manifest member into the staging tree.
fn fill_stage(from: &Path, manifest: &Manifest, stage: &Path) -> Result<()> {
    crate::config::private_dir(stage)?;
    for entry in &manifest.files {
        let source = join_rel(from, &entry.path)?;
        let dest = join_rel(stage, &entry.path)?;
        copy_file(&source, &dest)?;
    }
    Ok(())
}

/// Exchange two paths in one step where the OS can: `a` names what `b` did and
/// the other way round, and neither is ever missing. `Ok(false)` means this
/// platform or filesystem has no such call.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn exchange(a: &Path, b: &Path) -> std::io::Result<bool> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;
    match renameat_with(CWD, a, CWD, b, RenameFlags::EXCHANGE) {
        Ok(()) => Ok(true),
        Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// No atomic exchange here: the caller moves the trees one after the other.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn exchange(_a: &Path, _b: &Path) -> std::io::Result<bool> {
    Ok(false)
}

/// Move the staging tree onto the destination, which always exists by now: it
/// is the live tree, or the empty directory this restore made to lock.
#[cfg(not(windows))]
fn move_into_place(stage: &Path, data_dir: &Path, parent: &Path, token: &ulid::Ulid) -> Result<()> {
    // Where there is an atomic exchange, the destination names the old tree
    // one instant and the new one the next, with no instant in which it is
    // missing for a hub to create a fresh store in. The old tree is then at
    // the staging path.
    if exchange(stage, data_dir)? {
        // The replaced tree is the operator's to remove, and failing to remove
        // it is not a failed restore.
        if let Err(err) = std::fs::remove_dir_all(stage) {
            tracing::warn!(
                path = %stage.display(),
                error = %err,
                "the restored data directory is in place, but the replaced one could not be removed"
            );
        }
        return Ok(());
    }

    // No exchange: move the current tree aside first. The two renames leave a
    // moment with no destination, which only the locks held by the caller
    // cover. If the second rename fails the first is undone.
    let aside: PathBuf = parent.join(format!(".agent-hub-replaced-{token}"));
    std::fs::rename(data_dir, &aside)?;
    if let Err(err) = std::fs::rename(stage, data_dir) {
        let _ = std::fs::rename(&aside, data_dir);
        return Err(err.into());
    }
    if let Err(err) = std::fs::remove_dir_all(&aside) {
        tracing::warn!(
            path = %aside.display(),
            error = %err,
            "the restored data directory is in place, but the replaced one could not be removed"
        );
    }
    Ok(())
}

/// Move the staged entries into the destination around its hub.lock.
///
/// Windows refuses to rename a directory while any file under it is open
/// (MS-FSA 2.1.5.15.11, whatever the share mode), and the live hub.lock is
/// open here, by this restore and by any hub waiting on it. So the lock file
/// stays where it is, held throughout, and the entries around it move: the old
/// ones aside, then the staged ones in. A waiting hub gets the same lock file
/// once the restore lets go and opens the restored store. A failure moves
/// everything back. A crash part way leaves the rest of the old entries in
/// `.agent-hub-replaced-<token>` and the rest of the new ones in the stage.
#[cfg(windows)]
fn move_into_place(stage: &Path, data_dir: &Path, parent: &Path, token: &ulid::Ulid) -> Result<()> {
    let aside: PathBuf = parent.join(format!(".agent-hub-replaced-{token}"));
    std::fs::create_dir(&aside)?;
    let old = match move_entries(data_dir, &aside) {
        Ok(names) => names,
        Err(err) => {
            let _ = std::fs::remove_dir(&aside);
            return Err(err.into());
        }
    };
    if let Err(err) = move_entries(stage, data_dir) {
        if move_back(&aside, data_dir, &old).is_ok() {
            let _ = std::fs::remove_dir(&aside);
        } else {
            tracing::warn!(
                path = %aside.display(),
                "the restore failed and part of the replaced data directory is still here"
            );
        }
        return Err(err.into());
    }
    for leftover in [&aside, stage] {
        if let Err(err) = std::fs::remove_dir_all(leftover) {
            tracing::warn!(
                path = %leftover.display(),
                error = %err,
                "the restored data directory is in place, but a leftover could not be removed"
            );
        }
    }
    Ok(())
}

/// Move every entry of `from` but its hub.lock into `to`, returning the names
/// moved. On a failure the moved entries go back before the error returns.
#[cfg(windows)]
fn move_entries(from: &Path, to: &Path) -> std::io::Result<Vec<std::ffi::OsString>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(from)? {
        let name = entry?.file_name();
        if name != HUB_LOCK {
            names.push(name);
        }
    }
    let mut moved = Vec::new();
    for name in names {
        if let Err(err) = std::fs::rename(from.join(&name), to.join(&name)) {
            let _ = move_back(to, from, &moved);
            return Err(err);
        }
        moved.push(name);
    }
    Ok(moved)
}

/// Move the named entries from `from` back into `to`, trying every one.
#[cfg(windows)]
fn move_back(from: &Path, to: &Path, names: &[std::ffi::OsString]) -> std::io::Result<()> {
    let mut first = Ok(());
    for name in names {
        if let Err(err) = std::fs::rename(from.join(name), to.join(name))
            && first.is_ok()
        {
            first = Err(err);
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("tmp")
            .join(format!("restore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create scratch");
        root
    }

    #[test]
    fn a_directory_made_to_be_locked_goes_again_unless_the_restore_finished() {
        let root = scratch("created");
        for (armed, kept) in [(true, false), (false, true)] {
            let dir = root.join(format!("data-{armed}"));
            std::fs::create_dir(&dir).expect("create");
            let lock = lock_store_file(&dir.join(HUB_LOCK), Duration::ZERO)
                .expect("lock")
                .expect("free");
            drop(CreatedDir { path: &dir, armed });
            drop(lock);
            assert_eq!(dir.exists(), kept, "armed={armed}");
        }
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_exchange_trades_two_trees_in_one_step() {
        let root = scratch("exchange");
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir(&a).expect("a");
        std::fs::create_dir(&b).expect("b");
        std::fs::write(a.join("which"), "a").expect("write");
        std::fs::write(b.join("which"), "b").expect("write");
        assert!(exchange(&a, &b).expect("exchange"), "the OS has the call");
        assert_eq!(std::fs::read_to_string(a.join("which")).expect("read"), "b");
        assert_eq!(std::fs::read_to_string(b.join("which")).expect("read"), "a");
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[cfg(windows)]
    #[test]
    fn the_entries_move_around_a_lock_that_stays_put_and_held() {
        let root = scratch("in-place");
        let (data, stage) = (root.join("data"), root.join("stage"));
        std::fs::create_dir(&data).expect("data");
        std::fs::create_dir(&stage).expect("stage");
        let lock_path = data.join(HUB_LOCK);
        let held = lock_store_file(&lock_path, Duration::ZERO)
            .expect("lock")
            .expect("free");
        // A second open handle, as a hub waiting on the lock holds.
        let waiting = File::open(&lock_path).expect("open the lock again");
        std::fs::write(data.join("old"), "old").expect("write");
        std::fs::create_dir(data.join("sessions")).expect("a directory");
        std::fs::write(stage.join("new"), "new").expect("write");

        move_into_place(&stage, &data, &root, &ulid::Ulid::generate()).expect("move");

        assert!(
            still_at(&held, &lock_path).expect("compare"),
            "same lock file"
        );
        // Windows file locks are not re-entrant, so the hold is checked from
        // another handle, as a waiting hub sees it.
        assert!(
            matches!(waiting.try_lock_shared(), Err(TryLockError::WouldBlock)),
            "still held by the restore's handle"
        );
        let mut names: Vec<_> = std::fs::read_dir(&data)
            .expect("list")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        names.sort();
        assert_eq!(names, [HUB_LOCK, "new"]);
        assert!(!stage.exists(), "the stage is gone");
        drop((waiting, held));
        std::fs::remove_dir_all(&root).expect("clean up");
    }
}
