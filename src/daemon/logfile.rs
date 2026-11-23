//! The hub's log file: a size-rotated sink for the tracing output, and the
//! readers `agent-hub logs` uses.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The size at which the log rolls over.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// How many rolled files are kept beside the live one.
pub const KEEP: usize = 3;

/// An append-only file that rolls over when it would pass its size cap.
///
/// `hub.log` becomes `hub.log.1`, `hub.log.1` becomes `hub.log.2`, and the file
/// past `keep` is removed. A line is never split across two files.
pub struct RotatingFile {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    file: File,
    size: u64,
}

impl RotatingFile {
    /// Open the log for appending, creating its directory and the file (0600).
    pub fn open(path: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
        let file = open_append(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            max_bytes,
            keep,
            file,
            size,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        // This handle is the only writer of the live file: the process's own
        // stdio goes to a separate startup file, so nothing else keeps a
        // descriptor on the file being moved.
        for index in (1..self.keep).rev() {
            let from = numbered(&self.path, index);
            if from.exists() {
                let to = numbered(&self.path, index + 1);
                // Windows will not rename over a file that exists.
                let _ = std::fs::remove_file(&to);
                std::fs::rename(&from, to)?;
            }
        }
        if self.keep == 0 {
            std::fs::remove_file(&self.path)?;
        } else {
            let first = numbered(&self.path, 1);
            let _ = std::fs::remove_file(&first);
            std::fs::rename(&self.path, first)?;
        }
        self.file = open_append(&self.path)?;
        self.size = 0;
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size > 0 && self.size + buf.len() as u64 > self.max_bytes {
            // A failed rotation must not lose the line: keep appending.
            if let Err(err) = self.rotate() {
                eprintln!("agent-hub: could not rotate {}: {err}", self.path.display());
            }
        }
        let written = self.file.write(buf)?;
        self.size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// The shared handle tracing writes through.
#[derive(Clone)]
pub struct LogSink(Arc<Mutex<RotatingFile>>);

impl LogSink {
    /// Open the log file with the standard size and count.
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::open_with(path, MAX_BYTES, KEEP)
    }

    /// Open the log file with a size and count of the caller's choosing.
    pub fn open_with(path: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
        Ok(Self(Arc::new(Mutex::new(RotatingFile::open(
            path, max_bytes, keep,
        )?))))
    }
}

/// One tracing event's write, held under the file's lock so lines never mix.
pub struct SinkWriter<'a>(std::sync::MutexGuard<'a, RotatingFile>);

impl Write for SinkWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = SinkWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        SinkWriter(
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

/// `path` with `.N` appended to its file name.
pub fn numbered(path: &Path, index: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

/// Create the directories above `path` that do not exist yet, owner-only on
/// Unix. A directory that exists is left exactly as it is: the log may sit in a
/// shared directory or the current one, and its mode is not ours to change.
fn create_parent(path: &Path) -> io::Result<()> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)
}

/// Open a log file for appending, creating it owner-only (0600) on Unix. A file
/// that exists is tightened to that mode and nothing else is touched.
pub fn open_append(path: &Path) -> io::Result<File> {
    create_parent(path)?;
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    open_private(path, &mut options)
}

/// Open with `options`, refusing a symlink at the last component on Unix, and
/// make the opened file owner-only through its handle, never by path: a path
/// can be swapped for another file between the open and a chmod.
fn open_private(path: &Path, options: &mut OpenOptions) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        let file = options.open(path).map_err(|err| {
            if err.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) {
                io::Error::new(
                    err.kind(),
                    format!("{} is a symlink; a log file must be a file", path.display()),
                )
            } else {
                err
            }
        })?;
        let mode = file.metadata()?.permissions().mode();
        if mode & 0o077 != 0 {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        options.open(path)
    }
}

/// Where a started hub's own stdout and stderr go: the log file's path with
/// `.start` appended, so each log file, and so each store, has its own.
///
/// It is rewritten empty on every start and never rotated, so the descriptors
/// the child keeps on it stay valid. The hub's real log is the rotating file it
/// opens itself, and a hub that cannot open that file stops rather than log
/// here.
pub fn startup_path(log_file: &Path) -> PathBuf {
    let mut name = log_file.as_os_str().to_owned();
    name.push(".start");
    PathBuf::from(name)
}

/// Create the startup file empty, owner-only on Unix.
///
/// `start` points the new process's stdout and stderr here, and no rotation
/// ever moves it. It holds what the process says before its own log is open and
/// what it fails with, so `start` can report a process that died at once.
pub fn create_startup_file(path: &Path) -> io::Result<File> {
    create_parent(path)?;
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    open_private(path, &mut options)
}

/// Refuse a log file that would land on the store's lock or its record.
///
/// The log, the files it rolls into and its startup file are all checked: a
/// rotation renames the live file away, and doing that to `hub.lock` would
/// leave the lock on a file nobody else can find.
pub fn check_log_path(log_file: &Path, data_dir: &Path) -> Result<(), String> {
    // The names are reserved in every directory: another hub's data directory
    // is as much at risk as this one, and a name check needs no path to exist.
    if let Some(name) = log_file.file_name().and_then(|name| name.to_str()) {
        let folded = if cfg!(windows) {
            name.to_ascii_lowercase()
        } else {
            name.to_owned()
        };
        if folded == "hub.lock" || folded == "hub.pid.json" {
            return Err(format!(
                "log_file {} is named {name}, which the hub keeps for its lock in a data \
                 directory; set log_file (HUB_LOG_FILE) to a name of its own",
                log_file.display()
            ));
        }
        if folded.ends_with(".start") {
            return Err(format!(
                "log_file {} ends in .start, the startup file of another log; set log_file \
                 (HUB_LOG_FILE) to a name of its own",
                log_file.display()
            ));
        }
    }
    let reserved = [
        super::lock::path_in(data_dir),
        super::lock::record_path(data_dir),
    ];
    let mut written = vec![log_file.to_path_buf(), startup_path(log_file)];
    written.extend((1..=KEEP).map(|index| numbered(log_file, index)));
    for path in &written {
        if let Some(clash) = reserved.iter().find(|reserved| same_place(path, reserved)) {
            return Err(format!(
                "log_file {} would write to {}, which the hub keeps for its lock; set log_file \
                 (HUB_LOG_FILE) to a path of its own",
                log_file.display(),
                clash.display()
            ));
        }
    }
    Ok(())
}

/// Whether two paths name one file: the same file on disk when both exist, or
/// the same name in the same resolved directory.
fn same_place(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(left), Ok(right)) = (std::fs::metadata(a), std::fs::metadata(b))
            && left.dev() == right.dev()
            && left.ino() == right.ino()
        {
            return true;
        }
    }
    let resolved = |path: &Path| -> Option<PathBuf> {
        let absolute = std::path::absolute(path).ok()?;
        let name = absolute.file_name()?.to_owned();
        let parent = absolute.parent()?;
        Some(
            parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf())
                .join(name),
        )
    };
    match (resolved(a), resolved(b)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// The last `count` lines of the file, oldest first.
///
/// Reads backwards from the end in blocks, so a large log costs what is shown.
pub fn last_lines(path: &Path, count: usize) -> io::Result<Vec<String>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut file = File::open(path)?;
    let mut end = file.metadata()?.len();
    let mut tail: Vec<u8> = Vec::new();
    const BLOCK: u64 = 8192;
    loop {
        let start = end.saturating_sub(BLOCK);
        let mut block = vec![0; (end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut block)?;
        block.extend_from_slice(&tail);
        tail = block;
        end = start;
        // One more newline than the lines wanted proves the oldest is whole.
        let newlines = tail.iter().filter(|byte| **byte == b'\n').count();
        if end == 0 || newlines > count {
            break;
        }
    }
    let text = String::from_utf8_lossy(&tail);
    let mut lines: Vec<&str> = text.lines().collect();
    if end > 0 && !lines.is_empty() {
        // The first line starts before the block and is partial.
        lines.remove(0);
    }
    let skip = lines.len().saturating_sub(count);
    Ok(lines[skip..].iter().map(|line| line.to_string()).collect())
}

/// Follows a log file, across the roll-over that replaces it.
pub struct Follower {
    path: PathBuf,
    file: Option<File>,
    offset: u64,
    identity: Option<(u64, u64)>,
}

impl Follower {
    /// Start at the end of the file as it is now.
    pub fn at_end(path: &Path) -> Self {
        let mut follower = Self {
            path: path.to_path_buf(),
            file: None,
            offset: 0,
            identity: None,
        };
        if let Ok(file) = File::open(path) {
            follower.offset = file.metadata().map(|meta| meta.len()).unwrap_or(0);
            follower.identity = identity_of(&file);
            follower.file = Some(file);
        }
        follower
    }

    /// Whatever was appended since the last call.
    pub fn poll(&mut self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        // A rotated file is a new file at the same path: start it from the top.
        if let Ok(current) = File::open(&self.path) {
            let now = identity_of(&current);
            if self.file.is_none() || now != self.identity {
                // Lines written to the old file just before it rolled are still
                // in it, through the handle held open: take them first.
                if let Some(old) = self.file.as_mut() {
                    read_from(old, self.offset, &mut bytes)?;
                }
                self.identity = now;
                self.offset = 0;
                self.file = Some(current);
            }
        }
        if let Some(file) = self.file.as_mut() {
            self.offset = read_from(file, self.offset, &mut bytes)?;
        }
        Ok(bytes)
    }
}

/// Append what `file` holds past `offset` to `out`, and return the new offset.
/// A file shorter than the offset was truncated, so it is read from the top.
fn read_from(file: &mut File, offset: u64, out: &mut Vec<u8>) -> io::Result<u64> {
    let length = file.metadata()?.len();
    let offset = if length < offset { 0 } else { offset };
    file.seek(SeekFrom::Start(offset))?;
    let before = out.len();
    file.read_to_end(out)?;
    Ok(offset + (out.len() - before) as u64)
}

#[cfg(unix)]
fn identity_of(file: &File) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    file.metadata().ok().map(|meta| (meta.dev(), meta.ino()))
}

/// Windows gives a renamed file its old creation time and a new file a new one,
/// which tells a rolled file from its replacement.
#[cfg(windows)]
fn identity_of(file: &File) -> Option<(u64, u64)> {
    use std::os::windows::fs::MetadataExt;
    file.metadata().ok().map(|meta| (meta.creation_time(), 0))
}

#[cfg(not(any(unix, windows)))]
fn identity_of(_file: &File) -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("tmp")
            .join(format!("daemon-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create scratch");
        root
    }

    #[test]
    fn the_log_rolls_over_at_the_cap_and_keeps_the_newest_files() {
        let dir = scratch("rotate");
        let path = dir.join("hub.log");
        let mut log = RotatingFile::open(&path, 100, 3).expect("open");
        for index in 0..12 {
            // Each line is 40 bytes, so two fit and the third rolls.
            // One call per line, as the tracing formatter writes an event.
            let line = format!("line {index:02} {}\n", "x".repeat(30));
            log.write_all(line.as_bytes()).expect("write");
        }
        assert!(path.exists(), "the live file exists");
        for index in 1..=3 {
            assert!(numbered(&path, index).exists(), "hub.log.{index} is kept");
        }
        assert!(!numbered(&path, 4).exists(), "nothing past the keep count");
        let newest = std::fs::read_to_string(&path).expect("read live");
        assert!(
            newest.contains("line 11"),
            "the live file holds the newest line"
        );
        let oldest_kept = std::fs::read_to_string(numbered(&path, 3)).expect("read .3");
        assert!(
            !oldest_kept.contains("line 00"),
            "the oldest lines are gone"
        );
        for file in [
            path.clone(),
            numbered(&path, 1),
            numbered(&path, 2),
            numbered(&path, 3),
        ] {
            let text = std::fs::read_to_string(&file).expect("read");
            assert!(
                text.lines().all(|line| line.len() == 38),
                "no line is split across files: {text:?}"
            );
            assert!(text.len() <= 100, "no file passes the cap");
        }
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn the_last_lines_are_whole_and_in_order() {
        let dir = scratch("tail");
        let path = dir.join("hub.log");
        let body: String = (0..2000).map(|i| format!("entry number {i}\n")).collect();
        std::fs::write(&path, body).expect("seed");
        let lines = last_lines(&path, 3).expect("tail");
        assert_eq!(
            lines,
            [
                "entry number 1997",
                "entry number 1998",
                "entry number 1999"
            ]
        );
        let all = last_lines(&path, 5000).expect("whole file");
        assert_eq!(all.len(), 2000);
        assert_eq!(all[0], "entry number 0");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_follower_sees_appends_and_a_rolled_file() {
        let dir = scratch("follow");
        let path = dir.join("hub.log");
        std::fs::write(&path, "old\n").expect("seed");
        let mut follower = Follower::at_end(&path);
        assert!(follower.poll().expect("quiet").is_empty());
        let mut file = open_append(&path).expect("append");
        writeln!(file, "one").expect("write");
        assert_eq!(follower.poll().expect("append"), b"one\n");
        std::fs::rename(&path, numbered(&path, 1)).expect("roll");
        std::fs::write(&path, "two\n").expect("new file");
        assert_eq!(follower.poll().expect("roll"), b"two\n");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    /// Emit `count` events through a subscriber that writes to `sink`.
    fn emit(sink: &LogSink, first: usize, count: usize) {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(sink.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            for index in first..first + count {
                tracing::info!("event {index:03} {}", "x".repeat(40));
            }
        });
    }

    #[test]
    fn tracing_keeps_writing_to_the_live_file_across_rotations() {
        let dir = scratch("live-after-roll");
        let path = dir.join("hub.log");
        let sink = LogSink::open_with(&path, 400, 2).expect("open");
        emit(&sink, 0, 6);
        let before = std::fs::metadata(numbered(&path, 1)).is_ok();
        emit(&sink, 6, 30);
        assert!(before, "the first events already rolled");
        let live = std::fs::read_to_string(&path).expect("live file");
        assert!(
            live.contains("event 035"),
            "the newest event is in the live file: {live}"
        );
        assert!(
            std::fs::read_to_string(numbered(&path, 1))
                .is_ok_and(|text| !text.contains("event 035")),
            "an archive does not hold the newest event"
        );
        assert!(!numbered(&path, 3).exists(), "nothing past the keep count");
        // The live path is the file the writer holds: a reader that opens the path
        // sees the line the moment it is written.
        emit(&sink, 36, 1);
        let live = std::fs::read_to_string(&path).expect("live file");
        assert!(live.contains("event 036"), "{live}");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_follower_reads_what_landed_in_the_old_file_just_before_it_rolled() {
        let dir = scratch("follow-drain");
        let path = dir.join("hub.log");
        std::fs::write(&path, "old\n").expect("seed");
        let mut follower = Follower::at_end(&path);
        // Written to the old file, then the file rolled, before the next poll.
        let mut file = open_append(&path).expect("append");
        writeln!(file, "last words").expect("write");
        std::fs::rename(&path, numbered(&path, 1)).expect("roll");
        std::fs::write(&path, "fresh\n").expect("new file");
        assert_eq!(follower.poll().expect("poll"), b"last words\nfresh\n");
        assert!(follower.poll().expect("quiet").is_empty());
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_log_that_lands_on_the_lock_or_its_record_is_refused() {
        let dir = scratch("alias");
        let data = dir.join("data");
        std::fs::create_dir_all(&data).expect("data");
        for bad in [
            data.join("hub.lock"),
            data.join("hub.pid.json"),
            data.join("..").join("data").join("hub.lock"),
        ] {
            let err = check_log_path(&bad, &data).expect_err("an alias of the lock");
            assert!(err.contains("keeps for its lock"), "{err}");
        }
        // The names are refused in any directory, and a startup file's name
        // is not a log's.
        let elsewhere = dir.join("other");
        for bad in ["hub.lock", "hub.pid.json", "hub.log.start", "x.start"] {
            assert!(
                check_log_path(&elsewhere.join(bad), &data).is_err(),
                "{bad} is reserved in any directory"
            );
        }
        assert!(check_log_path(&elsewhere.join("hub.lock.log"), &data).is_ok());
        assert!(check_log_path(&elsewhere.join("started"), &data).is_ok());
        assert!(check_log_path(&dir.join("hub.log"), &data).is_ok());
        assert!(check_log_path(&data.join("hub.log"), &data).is_ok());
        #[cfg(unix)]
        {
            std::fs::write(data.join("hub.lock"), "").expect("lock");
            std::fs::hard_link(data.join("hub.lock"), dir.join("linked.log")).expect("hard link");
            assert!(check_log_path(&dir.join("linked.log"), &data).is_err());
            std::os::unix::fs::symlink(&data, dir.join("via")).expect("dir link");
            assert!(check_log_path(&dir.join("via").join("hub.pid.json"), &data).is_err());
        }
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[cfg(unix)]
    mod permissions {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn mode(path: &Path) -> u32 {
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777
        }

        #[test]
        fn an_existing_parent_directory_keeps_its_mode() {
            let dir = scratch("parent-mode");
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            let path = dir.join("hub.log");
            drop(open_append(&path).expect("open"));
            assert_eq!(mode(&dir), 0o755, "the directory was left alone");
            assert_eq!(mode(&path), 0o600, "a new log is owner-only");
            std::fs::remove_dir_all(&dir).expect("clean up");
        }

        #[test]
        fn a_bare_file_name_does_not_touch_the_current_directory() {
            let dir = scratch("cwd-mode");
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            let before = mode(Path::new("."));
            // The parent of a bare name is empty: nothing to create or change.
            create_parent(Path::new("hub.log")).expect("no parent");
            assert_eq!(mode(Path::new(".")), before);
            std::fs::remove_dir_all(&dir).expect("clean up");
        }

        #[test]
        fn an_existing_broad_log_is_tightened_and_a_new_directory_is_private() {
            let dir = scratch("tighten");
            let path = dir.join("logs").join("hub.log");
            drop(open_append(&path).expect("create"));
            assert_eq!(
                mode(&dir.join("logs")),
                0o700,
                "a directory we create is private"
            );
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
            drop(open_append(&path).expect("reopen"));
            assert_eq!(mode(&path), 0o600);
            std::fs::remove_dir_all(&dir).expect("clean up");
        }

        #[test]
        fn a_symlinked_log_is_refused_and_its_target_left_alone() {
            let dir = scratch("symlink");
            let target = dir.join("elsewhere");
            std::fs::write(&target, "keep\n").expect("target");
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644))
                .expect("chmod");
            let link = dir.join("hub.log");
            std::os::unix::fs::symlink(&target, &link).expect("symlink");
            let err = open_append(&link).expect_err("a symlinked log");
            assert!(err.to_string().contains("is a symlink"), "{err}");
            assert!(LogSink::open(&link).is_err());
            let start = startup_path(&link);
            std::os::unix::fs::symlink(&target, &start).expect("symlink");
            assert!(create_startup_file(&start).is_err());
            assert_eq!(std::fs::read_to_string(&target).expect("read"), "keep\n");
            assert_eq!(mode(&target), 0o644, "the target's mode was not touched");
            std::fs::remove_dir_all(&dir).expect("clean up");
        }

        #[test]
        fn the_startup_file_is_truncated_and_private() {
            let dir = scratch("startup-file");
            let path = startup_path(&dir.join("hub.log"));
            std::fs::write(&path, "last run\n").expect("seed");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
            drop(create_startup_file(&path).expect("create"));
            assert_eq!(std::fs::read_to_string(&path).expect("read"), "");
            assert_eq!(mode(&path), 0o600);
            std::fs::remove_dir_all(&dir).expect("clean up");
        }
    }
}
