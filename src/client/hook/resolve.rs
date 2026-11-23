//! Project and session resolution. [`resolve_context`] is pure: the git facts,
//! the files and the environment arrive as data, so the golden fixtures drive
//! it. [`gather_git`] and [`read_top_level_file`] collect them from the machine.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::Digest;

use super::render::{clip, flatten};

const NAME_MAX: usize = 64;
const PERSONAL_LINE: &str = "no project matched; write `.agent-hub` to pin one";

/// What git says about the working directory.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GitFacts {
    pub is_repo: bool,
    #[serde(default)]
    pub top_level: Option<String>,
    #[serde(default)]
    pub origin_url: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub detached: bool,
}

/// Everything resolution reads.
#[derive(Debug, Clone, Deserialize)]
pub struct ResolveInput {
    pub harness: String,
    pub harness_session_id: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub git: GitFacts,
    /// Absolute path to file text, for the `.agent-hub` file.
    #[serde(default)]
    pub files: HashMap<String, String>,
    #[serde(default)]
    pub known_projects: Vec<String>,
    pub personal_project: String,
}

/// Where the project came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSource {
    Env,
    File,
    Origin,
    Dir,
    Personal,
}

/// Where the session name came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    Env,
    Branch,
    HarnessSession,
}

/// The project and session a harness session maps to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolved {
    pub project_id: String,
    pub project_source: ProjectSource,
    pub session_name: String,
    pub session_source: SessionSource,
    pub context_line: Option<String>,
}

impl Resolved {
    /// The context lines, one per line of `context_line`.
    pub fn context_lines(&self) -> Vec<String> {
        self.context_line
            .as_deref()
            .map(|text| text.split('\n').map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Whether the project was named outright, so no project list is needed.
    pub fn project_is_pinned(&self) -> bool {
        matches!(
            self.project_source,
            ProjectSource::Env | ProjectSource::File
        )
    }
}

fn valid_id(text: &str) -> bool {
    let len = text.chars().count();
    (1..=NAME_MAX).contains(&len)
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A name inferred from the repository: lowercased, every run outside
/// `[a-z0-9_]` a single `-`, trimmed of `-`.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for ch in text.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' {
            out.push(ch);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// An inferred name cut to 64 characters, with a `-` the cut leaves removed.
fn inferred(text: &str) -> Option<String> {
    let cut: String = slug(text).chars().take(NAME_MAX).collect();
    let cut = cut.trim_end_matches('-');
    (!cut.is_empty()).then(|| cut.to_string())
}

/// The last path segment of an origin URL, without a trailing `/` or `.git`.
pub fn repo_name(origin_url: &str) -> Option<String> {
    let url = origin_url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let url = url.trim_end_matches('/');
    let last = url.rsplit(['/', ':']).next()?;
    (!last.is_empty()).then(|| last.to_string())
}

/// The first 8 characters of a harness session id that a session name may hold.
/// An id with fewer than 8 of them is named by the start of its SHA-256.
fn short_session_id(id: &str) -> String {
    use std::fmt::Write;
    let valid: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .take(8)
        .collect();
    if valid.len() == 8 {
        return valid;
    }
    sha2::Sha256::digest(id.as_bytes())
        .iter()
        .take(4)
        .fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").ok();
            hex
        })
}

fn set(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|text| !text.is_empty())
}

fn rejection(source: &str, value: &str) -> String {
    format!(
        "ignored {source} \"{}\": not [A-Za-z0-9_-]{{1,64}}",
        clip(&flatten(value), NAME_MAX)
    )
}

/// Resolve the project and the session name.
pub fn resolve_context(input: &ResolveInput) -> Resolved {
    let known: HashSet<&str> = input.known_projects.iter().map(String::as_str).collect();
    let mut project_lines = Vec::new();
    let mut session_lines = Vec::new();
    let mut project: Option<(String, ProjectSource)> = None;

    if let Some(value) = set(input.env.get("HUB_PROJECT")) {
        if valid_id(value) {
            project = Some((value.to_string(), ProjectSource::Env));
        } else {
            project_lines.push(rejection("HUB_PROJECT", value));
        }
    }

    let top = input.git.top_level.as_deref().filter(|_| input.git.is_repo);

    if project.is_none()
        && let Some(top) = top
        && let Some(text) = input.files.get(&format!("{top}/.agent-hub"))
    {
        let first = text.split('\n').next().unwrap_or_default();
        let first = first.strip_suffix('\r').unwrap_or(first).trim();
        if !first.is_empty() {
            if valid_id(first) {
                project = Some((first.to_string(), ProjectSource::File));
            } else {
                project_lines.push(rejection(".agent-hub", first));
            }
        }
    }

    if project.is_none()
        && top.is_some()
        && let Some(origin) = input
            .git
            .origin_url
            .as_deref()
            .and_then(repo_name)
            .and_then(|name| inferred(&name))
        && known.contains(origin.as_str())
    {
        project = Some((origin, ProjectSource::Origin));
    }

    if project.is_none()
        && let Some(top) = top
        && let Some(base) = top
            .split('/')
            .rfind(|part| !part.is_empty())
            .and_then(inferred)
        && known.contains(base.as_str())
    {
        project = Some((base, ProjectSource::Dir));
    }

    let mut lines = project_lines;
    let (project_id, project_source) = project.unwrap_or_else(|| {
        lines.push(PERSONAL_LINE.to_string());
        (input.personal_project.clone(), ProjectSource::Personal)
    });

    let mut session: Option<(String, SessionSource)> = None;
    if let Some(value) = set(input.env.get("HUB_SESSION")) {
        if valid_id(value) {
            session = Some((value.to_string(), SessionSource::Env));
        } else {
            session_lines.push(rejection("HUB_SESSION", value));
        }
    }
    if session.is_none()
        && input.git.is_repo
        && !input.git.detached
        && let Some(branch) = input.git.branch.as_deref()
        && !branch.is_empty()
        && branch != "main"
        && branch != "master"
        && let Some(name) = inferred(branch)
    {
        session = Some((name, SessionSource::Branch));
    }
    let (session_name, session_source) = session.unwrap_or_else(|| {
        (
            format!(
                "{}-{}",
                input.harness,
                short_session_id(&input.harness_session_id)
            ),
            SessionSource::HarnessSession,
        )
    });
    lines.extend(session_lines);

    Resolved {
        project_id,
        project_source,
        session_name,
        session_source,
        context_line: (!lines.is_empty()).then(|| lines.join("\n")),
    }
}

/// Run git in `cwd` and return its trimmed stdout, or `None` when git fails,
/// is missing, or does not finish before `until`.
///
/// A reader thread drains stdout from the moment git starts, so a git that
/// writes more than a pipe holds is never stuck on it. A git that overruns is
/// killed and reaped. The wait for the reader ends at `until` too: a process git
/// left behind may hold the pipe open after git itself is gone, and that is not
/// waited on.
fn run_git(cwd: &str, args: &[&str], until: Instant) -> Option<String> {
    let limit = until
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).ok();
        sender.send(bytes).ok();
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < limit => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                child.kill().ok();
                child.wait().ok();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let bytes = receiver
        .recv_timeout(until.saturating_duration_since(Instant::now()))
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// What git says about `cwd`, each question answered before `until`.
pub fn gather_git(cwd: &str, until: Instant) -> GitFacts {
    let Some(top) =
        run_git(cwd, &["rev-parse", "--show-toplevel"], until).filter(|top| !top.is_empty())
    else {
        return GitFacts::default();
    };
    let branch = run_git(cwd, &["symbolic-ref", "--short", "-q", "HEAD"], until)
        .filter(|branch| !branch.is_empty());
    GitFacts {
        is_repo: true,
        top_level: Some(top),
        origin_url: run_git(cwd, &["remote", "get-url", "origin"], until)
            .filter(|url| !url.is_empty()),
        detached: branch.is_none(),
        branch,
    }
}

/// Open `path` only when it is a regular file, so that a FIFO or a device in
/// its place cannot block the hook. The kind is checked before the open and
/// again on the handle, and the open does not wait.
pub fn open_regular(path: &Path) -> Option<std::fs::File> {
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = options.open(path).ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}

/// The path an open handle names, in the extended form `canonicalize` returns.
/// This is the Windows counterpart of reading `/proc/self/fd/N`.
#[cfg(windows)]
pub(crate) fn handle_path(file: &std::fs::File) -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW,
    };

    let handle = file.as_raw_handle();
    let mut buffer = vec![0u16; 1024];
    loop {
        let capacity = u32::try_from(buffer.len()).ok()?;
        // SAFETY: `handle` is open for the life of `file`, and `buffer` holds
        // `capacity` writable units.
        let written = unsafe {
            GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), capacity, FILE_NAME_NORMALIZED)
        } as usize;
        match written {
            0 => return None,
            n if n < buffer.len() => {
                buffer.truncate(n);
                return Some(std::ffi::OsString::from_wide(&buffer).into());
            }
            n => buffer.resize(n + 1, 0),
        }
    }
}

/// The most of the `.agent-hub` file that is read. Only its first line counts.
const PIN_FILE_MAX: u64 = 4096;

/// The `.agent-hub` file at the top level, keyed by its absolute path. It is
/// read only when it is a regular file, and only its first 4 KiB.
pub fn read_top_level_file(top: Option<&str>) -> HashMap<String, String> {
    let mut files = HashMap::new();
    if let Some(top) = top {
        let path = format!("{top}/.agent-hub");
        if let Some(file) = open_regular(Path::new(&path)) {
            let mut bytes = Vec::new();
            if file.take(PIN_FILE_MAX).read_to_end(&mut bytes).is_ok() {
                files.insert(path, String::from_utf8_lossy(&bytes).into_owned());
            }
        }
    }
    files
}
