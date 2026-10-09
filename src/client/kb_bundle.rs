//! A project knowledge base as a plain folder of files, and back.
//!
//! An export writes every page at its path under `/fs` into a directory, which
//! is then an OKF bundle a person can read, edit, diff and commit, beside a
//! manifest of the version each page had. An import plans every change, lints
//! the folder, and writes nothing when a page it would write has a lint error,
//! then writes each page that changed with the version the manifest recorded
//! as its guard, so a page that changed on the hub since the export is
//! reported as a conflict and skipped, never overwritten. Both go through the
//! brain tools `agent-hub kb` uses, over one connection, so a page gets the
//! hub's own path rules, size limit, log row and search row.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Write};
use std::path::{Component, Path, PathBuf};

use rmcp::service::{RoleClient, RunningService};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

use super::{Failure, call_on, connect};
use crate::brain::{VERSION_ABSENT, version};
use crate::config::ClientConfig;
use crate::limits::{
    KB_BUNDLE_BYTES_MAX, KB_BUNDLE_PAGES_MAX, KB_PAGE_BYTES_MAX, KB_PATH_BYTES_MAX,
};
use crate::okf::{LintFinding, lint_bundle};

/// The manifest an export writes at the root of the folder.
///
/// It starts with a dot, and an import reads no dot-named file or directory as
/// a page, so it never becomes one; nor does a `.git` beside it.
pub const MANIFEST: &str = ".agent-hub-kb.json";

/// The manifest layout this binary writes and reads.
const MANIFEST_FORMAT: u64 = 1;

/// The lint codes that refuse an import when they are on a page it writes.
///
/// These say a page is wrong: a frontmatter block the hub cannot read safely,
/// `okf_version` off the root, or a link that leaves the bundle. The other
/// codes say a bundle is incomplete (no frontmatter, no type, no index, no
/// log, an orphan, a missing index entry, a link to a page that is not
/// there), which a hub's lenient writes leave behind as a matter of course, so
/// they are reported and the import goes ahead. A blocking code on a page the
/// import leaves as it is only reports too: the hub already holds it, and its
/// own export has to come back.
pub const BLOCKING_LINT: [&str; 3] = [
    "unparsed_frontmatter",
    "okf_version_misplaced",
    "link_escapes_bundle",
];

/// What the manifest records: which project, and each page's version.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    format: u64,
    project: String,
    #[serde(deserialize_with = "unique_pages")]
    pages: BTreeMap<String, String>,
}

/// The manifest's pages, refusing a path listed twice: a JSON reader keeps one
/// of the two quietly, and either could be the version the folder was read at.
fn unique_pages<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Pages;
    impl<'de> serde::de::Visitor<'de> for Pages {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a map of page paths to versions")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut pages = BTreeMap::new();
            while let Some((path, version)) = map.next_entry::<String, String>()? {
                if pages.contains_key(&path) {
                    return Err(serde::de::Error::custom(format!("{path} is listed twice")));
                }
                pages.insert(path, version);
            }
            Ok(pages)
        }
    }
    deserializer.deserialize_map(Pages)
}

/// What an export wrote.
#[derive(Debug)]
pub struct Exported {
    /// Pages written to the folder.
    pub pages: usize,
    /// Bytes of page content written.
    pub bytes: u64,
    /// Pages left out because a dot-named component would hide them from an
    /// import.
    pub skipped: Vec<String>,
    /// Files of pages an earlier export wrote and the hub no longer has,
    /// removed by an export with `force`.
    pub removed: Vec<String>,
}

/// What an import does, or would do on a dry run, to one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The folder and the hub hold the same bytes.
    Unchanged,
    /// A page in the folder the hub does not have.
    Create,
    /// A page the folder changed and the hub did not.
    Update,
    /// A page the folder no longer holds, deleted with `--prune`.
    Delete,
    /// A page missing from the folder and kept on the hub without `--prune`.
    Kept,
    /// A page the folder left as exported and the hub changed or deleted
    /// since: the hub is newer, so it is left as it is.
    HubNewer,
    /// A page the hub changed since the export, skipped, and why.
    Conflict(String),
    /// A page `--prune` would delete that the hub had already deleted.
    Gone,
    /// A change left unmade because the hub refused an earlier one.
    NotAttempted,
}

/// One page and what happens to it.
#[derive(Debug)]
pub struct Change {
    /// The page path, under `/fs`.
    pub path: String,
    /// What the import does to it.
    pub action: Action,
}

/// What an import found and did.
#[derive(Debug)]
pub struct Imported {
    /// Every page the folder or the manifest names, in path order.
    pub changes: Vec<Change>,
    /// Lint findings that did not refuse the import.
    pub warnings: Vec<LintFinding>,
    /// What went wrong once the import had begun writing: the hub's refusal
    /// that stopped it part way, after which the rest are
    /// [`Action::NotAttempted`], and a manifest that could not be saved.
    pub failures: Vec<Failure>,
}

impl Imported {
    /// How many pages were skipped as conflicts.
    pub fn conflicts(&self) -> usize {
        self.changes
            .iter()
            .filter(|change| matches!(change.action, Action::Conflict(_)))
            .count()
    }
}

/// Write every page of `project` into `dir` and record its version.
///
/// A directory that already holds anything is refused unless `force` is set.
/// Then the page files and the manifest are written over, and the files of
/// pages the old manifest names and this export no longer has are removed, so
/// a later import does not bring them back; nothing else in it is touched.
/// Every page is read and every target checked before the directory is created
/// or the first file written, so a failed read or a refused path leaves the
/// folder as it was.
pub async fn export(
    config: &ClientConfig,
    project: &str,
    dir: &Path,
    force: bool,
) -> Result<Exported, Failure> {
    check_target(dir, force)?;
    let previous = match parse_manifest(dir)? {
        Some(manifest) if force && manifest.project != project => {
            return Err(Failure::Failed(format!(
                "{} was exported from project {}, not {project}; its pages are not this export's to remove",
                dir.display(),
                manifest.project
            )));
        }
        Some(manifest) if force => manifest.pages,
        _ => BTreeMap::new(),
    };
    let hub = Hub::open(config, project).await?;
    let fetched = fetch(&hub).await;
    hub.close().await;
    let (pages, skipped) = fetched?;
    check_collisions(
        pages.iter().map(|page| page.path.as_str()),
        "refusing to export",
    )?;

    let exported: HashSet<&str> = pages.iter().map(|page| page.path.as_str()).collect();
    let mut stale = Vec::new();
    for page in previous.keys() {
        if exported.contains(page.as_str()) {
            continue;
        }
        if let Some(relative) = relative_file(page)? {
            stale.push((page.clone(), relative));
        }
    }
    match dir.canonicalize() {
        Ok(root) => {
            for page in &pages {
                place(&root, &page.relative, &page.path, false)?;
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(io_failure(dir, &err)),
    }

    std::fs::create_dir_all(dir).map_err(|err| io_failure(dir, &err))?;
    let root = dir.canonicalize().map_err(|err| io_failure(dir, &err))?;
    // Stale files go first, so on a filesystem that folds case a stale
    // `Notes.md` is never removed after a fresh `notes.md` was written as it.
    let mut removed = Vec::new();
    for (page, relative) in &stale {
        if let Some(file) = stale_file(&root, relative) {
            std::fs::remove_file(&file).map_err(|err| io_failure(&file, &err))?;
            removed.push(page.clone());
        }
    }
    let mut manifest = Manifest {
        format: MANIFEST_FORMAT,
        project: project.to_string(),
        pages: BTreeMap::new(),
    };
    let mut bytes = 0;
    for page in &pages {
        let target = place(&root, &page.relative, &page.path, true)?;
        write_file(&target, page.content.as_bytes())?;
        bytes += page.content.len() as u64;
        manifest
            .pages
            .insert(page.path.clone(), page.version.clone());
    }
    write_manifest(&root, &manifest)?;
    Ok(Exported {
        pages: pages.len(),
        bytes,
        skipped,
        removed,
    })
}

/// Write the pages of `dir` that changed into `project`, guarded by the
/// versions the manifest recorded.
///
/// The folder is read whole before the hub is reached and linted once the
/// changes are planned, and a blocking finding on a page the import would
/// create or update refuses it with nothing written. A page that changed on
/// the hub since the export is a conflict and is skipped; every other change is
/// still made, unless the hub refuses one for another reason, which stops the
/// import there and is returned in [`Imported::failures`]. With `prune`, a page the manifest names and
/// the folder no longer holds is deleted, under the same guard. A dry run
/// plans the same changes and writes nothing, the manifest included.
pub async fn import(
    config: &ClientConfig,
    project: &str,
    dir: &Path,
    dry_run: bool,
    prune: bool,
) -> Result<Imported, Failure> {
    let manifest = read_manifest(dir, project)?;
    let pages = read_bundle(dir)?;

    let hub = Hub::open(config, project).await?;
    let outcome = sync(&hub, dir, project, &manifest, &pages, dry_run, prune).await;
    hub.close().await;
    outcome
}

/// One page read from the hub for an export.
struct Fetched {
    path: String,
    relative: PathBuf,
    content: String,
    version: String,
}

/// Every page of the knowledge base with its version, and the pages left out.
async fn fetch(hub: &Hub<'_>) -> Result<(Vec<Fetched>, Vec<String>), Failure> {
    let mut pages = Vec::new();
    let mut skipped = Vec::new();
    let mut bytes = 0u64;
    for path in hub.pages().await? {
        let Some(relative) = relative_file(&path)? else {
            skipped.push(path);
            continue;
        };
        // A page deleted between the listing and the read is simply not in
        // this export.
        let Some((content, version)) = hub.read(&path).await? else {
            continue;
        };
        bytes += content.len() as u64;
        if bytes > KB_BUNDLE_BYTES_MAX {
            return Err(Failure::Failed(format!(
                "the knowledge base holds more than {KB_BUNDLE_BYTES_MAX} bytes of pages, the most one export carries"
            )));
        }
        pages.push(Fetched {
            path,
            relative,
            content,
            version,
        });
    }
    Ok((pages, skipped))
}

/// Refuse an export target that holds anything, unless asked to write over it.
fn check_target(dir: &Path, force: bool) -> Result<(), Failure> {
    let mut entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(io_failure(dir, &err)),
    };
    if !force && entries.next().is_some() {
        return Err(Failure::Failed(format!(
            "{} is not empty; export into a new or empty directory, or pass --force to write over the pages and manifest in it",
            dir.display()
        )));
    }
    Ok(())
}

/// The file a page is written to, relative to the folder, or `None` for a page
/// an import would not read back.
///
/// Every component has to be a plain name, so a path the hub should never
/// hand out still cannot reach outside the folder.
fn relative_file(page: &str) -> Result<Option<PathBuf>, Failure> {
    let escapes = || {
        Failure::Failed(format!(
            "refusing to export {page}: the path does not stay inside the folder"
        ))
    };
    let rest = page.strip_prefix("/fs/").ok_or_else(escapes)?;
    let mut relative = PathBuf::new();
    let mut hidden = false;
    for name in rest.split('/') {
        let mut components = Path::new(name).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(_)), None) if !name.contains('\\') => {}
            _ => return Err(escapes()),
        }
        hidden |= name.starts_with('.');
        relative.push(name);
    }
    Ok((!hidden).then_some(relative))
}

/// The file one page is written to under `root`, refusing a symbolic link or
/// a file where a directory has to be, and a directory or a symbolic link where
/// the page goes. With `create`, missing directories are made on the way.
fn place(root: &Path, relative: &Path, page: &str, create: bool) -> Result<PathBuf, Failure> {
    let outside = |at: &Path, what: &str| {
        Failure::Failed(format!(
            "refusing to export {page}: {} {what}, so the page could land outside {}",
            at.display(),
            root.display()
        ))
    };
    let mut current = root.to_path_buf();
    let directories = relative.parent().unwrap_or(Path::new(""));
    for component in directories.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(outside(&current, "is a symbolic link"));
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(outside(&current, "is a file, not a directory")),
            Err(err) if err.kind() == ErrorKind::NotFound && !create => {
                return Ok(root.join(relative));
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                std::fs::create_dir(&current).map_err(|err| io_failure(&current, &err))?;
            }
            Err(err) => return Err(io_failure(&current, &err)),
        }
    }
    let target = root.join(relative);
    match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => Err(outside(&target, "is a symbolic link")),
        Ok(meta) if meta.is_dir() => Err(outside(&target, "is a directory")),
        Ok(_) => Ok(target),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(target),
        Err(err) => Err(io_failure(&target, &err)),
    }
}

/// The file of a page an earlier export wrote, when it is still a regular file
/// reached through plain directories inside `root`; anything else is left.
fn stale_file(root: &Path, relative: &Path) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    for component in relative.parent().unwrap_or(Path::new("")).components() {
        current.push(component);
        let meta = std::fs::symlink_metadata(&current).ok()?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return None;
        }
    }
    let target = root.join(relative);
    let meta = std::fs::symlink_metadata(&target).ok()?;
    meta.file_type().is_file().then_some(target)
}

/// The key a filesystem that ignores case and Unicode normalisation (macOS by
/// default) files a path under.
fn folded(path: &str) -> String {
    path.nfc().collect::<String>().to_lowercase()
}

/// Refuse two page paths that differ only in case or normalisation, in a file
/// name or in a directory on the way to one.
///
/// Such a filesystem keeps the two as one file or one directory, so an export
/// would write one page over the other and the import after it would write
/// the survivor to the wrong page, or prune the right one.
fn check_collisions<'a>(
    pages: impl Iterator<Item = &'a str>,
    refusing: &str,
) -> Result<(), Failure> {
    check_collisions_with(pages, std::iter::empty(), refusing)
}

/// [`check_collisions`] over `pages`, and each of `pages` against each of
/// `beside`, which are not checked against one another.
///
/// An import checks the folder's pages against the hub pages it keeps, so a
/// rename that changes only case is refused rather than written as a second
/// page beside the first.
fn check_collisions_with<'a>(
    pages: impl Iterator<Item = &'a str>,
    beside: impl Iterator<Item = &'a str>,
    refusing: &str,
) -> Result<(), Failure> {
    let prefixes = |page: &'a str| {
        page.match_indices('/')
            .map(move |(at, _)| &page[..at])
            .chain(std::iter::once(page))
            .filter(|prefix| !prefix.is_empty())
    };
    let collide = |first: &str, second: &str| {
        Failure::Failed(format!(
            "{refusing}: {first} and {second} differ only in case or Unicode normalisation, which a case-insensitive filesystem keeps as one name"
        ))
    };
    let mut seen: HashMap<String, &str> = HashMap::new();
    for page in pages {
        for prefix in prefixes(page) {
            match seen.entry(folded(prefix)) {
                Entry::Occupied(first) if *first.get() != prefix => {
                    return Err(collide(first.get(), prefix));
                }
                Entry::Occupied(_) => {}
                Entry::Vacant(slot) => {
                    slot.insert(prefix);
                }
            }
        }
    }
    for page in beside {
        for prefix in prefixes(page) {
            if let Some(first) = seen.get(&folded(prefix))
                && *first != prefix
            {
                return Err(collide(first, prefix));
            }
        }
    }
    Ok(())
}

/// Open options that refuse to follow a symbolic link at the last component.
fn no_follow(options: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    options
}

/// Write a file, never through a symbolic link put there since it was checked.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    no_follow(OpenOptions::new().write(true).create(true).truncate(true))
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|err| io_failure(path, &err))
}

/// Read at most `limit` bytes of a file, never through a symbolic link, and
/// `None` when it holds more.
fn read_file(path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
    let file = no_follow(OpenOptions::new().read(true)).open(path)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
}

/// The most bytes of manifest an import reads: a path and a version a page.
const MANIFEST_BYTES_MAX: u64 = (KB_BUNDLE_PAGES_MAX * (KB_PATH_BYTES_MAX + 128)) as u64;

fn write_manifest(root: &Path, manifest: &Manifest) -> Result<(), Failure> {
    let path = root.join(MANIFEST);
    let mut text = serde_json::to_string_pretty(manifest)
        .map_err(|err| Failure::Failed(format!("could not encode the manifest: {err}")))?;
    text.push('\n');
    write_file(&path, text.as_bytes())
}

/// The manifest in `dir`, or an empty one when the folder has none.
///
/// A folder put together by hand has no manifest, so every page in it is a page
/// the hub should not have yet: each is created only while nothing is there.
fn read_manifest(dir: &Path, project: &str) -> Result<Manifest, Failure> {
    let Some(manifest) = parse_manifest(dir)? else {
        return Ok(Manifest {
            format: MANIFEST_FORMAT,
            project: project.to_string(),
            pages: BTreeMap::new(),
        });
    };
    if manifest.project != project {
        return Err(Failure::Failed(format!(
            "{} was exported from project {}, not {project}; its versions say nothing about this project",
            dir.display(),
            manifest.project
        )));
    }
    Ok(manifest)
}

/// The manifest in `dir`, `None` when there is none, or why it cannot be read.
///
/// Every page it names has to be a canonical page path under `/fs` that an
/// export could have written: a path the hub would spell another way would
/// guard one page with the version of another.
fn parse_manifest(dir: &Path) -> Result<Option<Manifest>, Failure> {
    let path = dir.join(MANIFEST);
    let not_a_manifest =
        |why: &str| Failure::Failed(format!("{} is not a manifest: {why}", path.display()));
    let bytes = match read_file(&path, MANIFEST_BYTES_MAX) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Err(not_a_manifest("it is too large")),
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(io_failure(&path, &err)),
    };
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|err| not_a_manifest(&err.to_string()))?;
    if manifest.format != MANIFEST_FORMAT {
        return Err(Failure::Failed(format!(
            "{} has format {}, and this binary reads format {MANIFEST_FORMAT}",
            path.display(),
            manifest.format
        )));
    }
    for page in manifest.pages.keys() {
        let canonical = crate::brain::knowledge::page_path(page).ok();
        let exportable = matches!(relative_file(page), Ok(Some(_)));
        if canonical.as_deref() != Some(page.as_str()) || !exportable {
            return Err(not_a_manifest(&format!(
                "{page} is not a canonical page path under /fs"
            )));
        }
    }
    Ok(Some(manifest))
}

/// Every page file in `dir`, keyed by its page path.
///
/// Dot-named files and directories are left out. A symbolic link, a file that
/// is not UTF-8 text, a page over the size limit and a folder over the bundle
/// limits are refused, before anything is written.
fn read_bundle(dir: &Path) -> Result<BTreeMap<String, String>, Failure> {
    let meta = std::fs::metadata(dir).map_err(|err| io_failure(dir, &err))?;
    if !meta.is_dir() {
        return Err(Failure::Failed(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    let mut pages = BTreeMap::new();
    let mut bytes = 0u64;
    let mut stack = vec![(dir.to_path_buf(), String::from("/fs"))];
    while let Some((at, prefix)) = stack.pop() {
        let entries = std::fs::read_dir(&at).map_err(|err| io_failure(&at, &err))?;
        for entry in entries {
            let entry = entry.map_err(|err| io_failure(&at, &err))?;
            let file = entry.path();
            let Some(name) = entry.file_name().to_str().map(page_name) else {
                return Err(Failure::Failed(format!(
                    "{} is not a UTF-8 name, so it cannot be a page path",
                    file.display()
                )));
            };
            if name.starts_with('.') {
                continue;
            }
            let page = format!("{prefix}/{name}");
            let kind = entry.file_type().map_err(|err| io_failure(&file, &err))?;
            if kind.is_symlink() {
                return Err(Failure::Failed(format!(
                    "{} is a symbolic link; an import reads only files inside the folder",
                    file.display()
                )));
            }
            if kind.is_dir() {
                stack.push((file, page));
                continue;
            }
            if !kind.is_file() {
                return Err(Failure::Failed(format!(
                    "{} is not a regular file",
                    file.display()
                )));
            }
            let over = || {
                Failure::Failed(format!(
                    "{} is over the {KB_PAGE_BYTES_MAX} byte page limit",
                    file.display()
                ))
            };
            let size = entry
                .metadata()
                .map_err(|err| io_failure(&file, &err))?
                .len();
            if size > KB_PAGE_BYTES_MAX as u64 {
                return Err(over());
            }
            let content = read_file(&file, KB_PAGE_BYTES_MAX as u64)
                .map_err(|err| io_failure(&file, &err))?
                .ok_or_else(over)?;
            let content = String::from_utf8(content)
                .map_err(|_| Failure::Failed(format!("{} is not UTF-8 text", file.display())))?;
            bytes += content.len() as u64;
            if bytes > KB_BUNDLE_BYTES_MAX {
                return Err(Failure::Failed(format!(
                    "{} holds more than {KB_BUNDLE_BYTES_MAX} bytes of pages, the most one import carries",
                    dir.display()
                )));
            }
            if pages.len() == KB_BUNDLE_PAGES_MAX {
                return Err(Failure::Failed(format!(
                    "{} holds more than {KB_BUNDLE_PAGES_MAX} pages, the most one import carries",
                    dir.display()
                )));
            }
            let canonical = crate::brain::knowledge::page_path(&page)
                .map_err(|err| Failure::Failed(format!("{}: {err}", file.display())))?;
            pages.insert(canonical, content);
        }
    }
    check_collisions(pages.keys().map(String::as_str), "refusing to import")?;
    Ok(pages)
}

/// The page name a folder file name stands for, in NFC.
///
/// macOS hands out names decomposed, and the hub keys a page by the bytes of
/// its path, so a name read back has to be the composed spelling the export
/// wrote, or the page would come back as a second one.
fn page_name(name: &str) -> String {
    name.nfc().collect()
}

/// The lint findings that do not refuse the import, or the refusal.
///
/// A blocking finding refuses only on a page the import would write. The same
/// finding on a page the hub already holds as it is in the folder is the hub's
/// own state, and a warning, so every export imports back.
fn lint(pages: &BTreeMap<String, String>, changes: &[Change]) -> Result<Vec<LintFinding>, Failure> {
    let written: HashSet<&str> = changes
        .iter()
        .filter(|change| matches!(change.action, Action::Create | Action::Update))
        .map(|change| change.path.as_str())
        .collect();
    let bundle: HashMap<String, String> = pages
        .iter()
        .map(|(path, content)| (path.clone(), content.clone()))
        .collect();
    let (mut findings, _) = lint_bundle(&bundle);
    findings.sort_by(|a, b| (&a.path, a.line, &a.code).cmp(&(&b.path, b.line, &b.code)));
    let (errors, warnings): (Vec<_>, Vec<_>) = findings.into_iter().partition(|finding| {
        BLOCKING_LINT.contains(&finding.code.as_str()) && written.contains(finding.path.as_str())
    });
    if errors.is_empty() {
        return Ok(warnings);
    }
    let listed: Vec<String> = errors
        .iter()
        .map(|finding| format!("  {}", describe(finding)))
        .collect();
    Err(Failure::Failed(format!(
        "the pages this import would write have {} lint error{}, so nothing was written:\n{}",
        errors.len(),
        if errors.len() == 1 { "" } else { "s" },
        listed.join("\n")
    )))
}

/// One lint finding on one line: its code, where, and what.
pub fn describe(finding: &LintFinding) -> String {
    match finding.line {
        Some(line) => format!(
            "{} {}:{line}: {}",
            finding.code, finding.path, finding.message
        ),
        None => format!("{} {}: {}", finding.code, finding.path, finding.message),
    }
}

/// Plan every change against the hub, then make the ones that are not
/// conflicts unless this is a dry run, and record what was written.
async fn sync(
    hub: &Hub<'_>,
    dir: &Path,
    project: &str,
    manifest: &Manifest,
    pages: &BTreeMap<String, String>,
    dry_run: bool,
    prune: bool,
) -> Result<Imported, Failure> {
    let mut changes = Vec::new();
    for (path, content) in pages {
        let local = version(content.as_bytes());
        let current = hub.version(path).await?;
        let base = manifest.pages.get(path).map(String::as_str);
        changes.push(Change {
            path: path.clone(),
            action: plan_write(&local, current.as_deref(), base),
        });
    }
    let mut gone = Vec::new();
    for (path, base) in &manifest.pages {
        if pages.contains_key(path) {
            continue;
        }
        let current = hub.version(path).await?;
        match plan_missing(current.as_deref(), base, prune) {
            Some(action) => changes.push(Change {
                path: path.clone(),
                action,
            }),
            None => gone.push(path),
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    let deleted: HashSet<&str> = changes
        .iter()
        .filter(|change| change.action == Action::Delete)
        .map(|change| change.path.as_str())
        .collect();
    let listed = hub.pages().await?;
    let kept = listed
        .iter()
        .map(String::as_str)
        .filter(|page| !pages.contains_key(*page) && !deleted.contains(page));
    check_collisions_with(pages.keys().map(String::as_str), kept, "refusing to import")?;
    let warnings = lint(pages, &changes)?;
    if dry_run {
        return Ok(Imported {
            changes,
            warnings,
            failures: Vec::new(),
        });
    }

    let mut recorded = Manifest {
        format: MANIFEST_FORMAT,
        project: project.to_string(),
        pages: manifest.pages.clone(),
    };
    // A page gone from both the folder and the hub has nothing left to guard.
    for path in gone {
        recorded.pages.remove(path);
    }
    let mut failures: Vec<Failure> = apply(hub, manifest, pages, &mut changes, &mut recorded)
        .await
        .into_iter()
        .collect();
    // What was written is recorded even when a later write failed, so the
    // next import guards each page with the version it now has. The hub has
    // been written by now, so a manifest that cannot be saved is reported
    // beside the changes rather than in place of them.
    let saved = dir
        .canonicalize()
        .map_err(|err| io_failure(dir, &err))
        .and_then(|root| write_manifest(&root, &recorded));
    if let Err(failure) = saved {
        failures.push(failure);
    }
    Ok(Imported {
        changes,
        warnings,
        failures,
    })
}

/// What to do with a page the manifest names and the folder no longer holds,
/// or `None` when the hub has none either.
///
/// Without `prune` it is kept. With it, it is deleted only while the hub still
/// has the version the export saw: a page edited on the hub and removed from
/// the folder was changed on both sides, which is a conflict.
fn plan_missing(current: Option<&str>, base: &str, prune: bool) -> Option<Action> {
    let current = current?;
    Some(if !prune {
        Action::Kept
    } else if current == base {
        Action::Delete
    } else {
        Action::Conflict("changed on the hub since the export, so it was not deleted".into())
    })
}

/// What to do with a page the folder holds, given the version it has there,
/// on the hub, and in the manifest.
fn plan_write(local: &str, current: Option<&str>, base: Option<&str>) -> Action {
    if current == Some(local) {
        return Action::Unchanged;
    }
    // The folder still holds what was exported, so only the hub moved.
    if base == Some(local) {
        return Action::HubNewer;
    }
    let expected = base.unwrap_or(VERSION_ABSENT);
    match (current, base) {
        (None, None) => Action::Create,
        (Some(current), Some(_)) if current == expected => Action::Update,
        (None, Some(_)) => Action::Conflict("deleted on the hub since the export".into()),
        (Some(_), None) => Action::Conflict("the hub has this page and the export did not".into()),
        (Some(_), Some(_)) => Action::Conflict("changed on the hub since the export".into()),
    }
}

/// Make every planned write and delete, guarded by the manifest's version.
///
/// A refusal that is not a conflict stops here: the change it refused and
/// every later write or delete are marked not attempted, and the refusal is
/// returned for the caller to report beside them.
async fn apply(
    hub: &Hub<'_>,
    manifest: &Manifest,
    pages: &BTreeMap<String, String>,
    changes: &mut [Change],
    recorded: &mut Manifest,
) -> Option<Failure> {
    let mut failure = None;
    for change in changes.iter_mut() {
        let path = &change.path;
        let guard = manifest
            .pages
            .get(path)
            .map_or(VERSION_ABSENT, String::as_str);
        if failure.is_some() {
            if matches!(
                change.action,
                Action::Create | Action::Update | Action::Delete
            ) {
                change.action = Action::NotAttempted;
            }
            continue;
        }
        match change.action {
            Action::Unchanged => {
                let local = version(pages[path].as_bytes());
                recorded.pages.insert(path.clone(), local);
            }
            Action::Create | Action::Update => match hub.put(path, &pages[path], guard).await {
                Ok(Some(written)) => {
                    recorded.pages.insert(path.clone(), written);
                }
                Ok(None) => change.action = raced(),
                Err(refused) => {
                    change.action = Action::NotAttempted;
                    failure = Some(refused);
                }
            },
            Action::Delete => match hub.delete(path, guard).await {
                Ok(Deleted::Yes) => {
                    recorded.pages.remove(path);
                }
                Ok(Deleted::Gone) => {
                    recorded.pages.remove(path);
                    change.action = Action::Gone;
                }
                Ok(Deleted::Changed) => change.action = raced(),
                Err(refused) => {
                    change.action = Action::NotAttempted;
                    failure = Some(refused);
                }
            },
            Action::Kept
            | Action::HubNewer
            | Action::Conflict(_)
            | Action::Gone
            | Action::NotAttempted => {}
        }
    }
    failure
}

/// What a guarded delete did.
enum Deleted {
    Yes,
    /// The page had changed since the version the guard named.
    Changed,
    /// The page was not there to delete.
    Gone,
}

fn raced() -> Action {
    Action::Conflict("changed on the hub during the import".into())
}

fn io_failure(path: &Path, err: &std::io::Error) -> Failure {
    Failure::Failed(format!("{}: {err}", path.display()))
}

/// The hub's error code for a failed call, when it named one.
fn error_code(failure: &Failure) -> Option<&str> {
    match failure {
        Failure::Tool(error) => error.get("error")?.get("code")?.as_str(),
        _ => None,
    }
}

/// One connection to the hub, and the project knowledge base it reaches.
struct Hub<'a> {
    service: RunningService<RoleClient, ()>,
    config: &'a ClientConfig,
    project: &'a str,
}

impl<'a> Hub<'a> {
    async fn open(config: &'a ClientConfig, project: &'a str) -> Result<Self, Failure> {
        Ok(Self {
            service: connect(config).await?,
            config,
            project,
        })
    }

    async fn close(self) {
        self.service.cancel().await.ok();
    }

    /// Call a brain tool on the project store.
    async fn call(&self, tool: &str, arguments: Value) -> Result<Value, Failure> {
        let Value::Object(mut arguments) = arguments else {
            unreachable!("every call here builds an object");
        };
        arguments.insert("store".into(), "project".into());
        arguments.insert("project_id".into(), self.project.into());
        call_on(&self.service, self.config, tool, arguments).await
    }

    /// Every page path, walking one listing level at a time.
    async fn pages(&self) -> Result<Vec<String>, Failure> {
        let mut pages = Vec::new();
        let mut stack = vec!["/fs".to_string()];
        while let Some(dir) = stack.pop() {
            let listed = self.call("brain_list", json!({"path": dir})).await?;
            for entry in listed["entries"].as_array().into_iter().flatten() {
                let Some(path) = entry.get("path").and_then(Value::as_str) else {
                    continue;
                };
                match entry.get("type").and_then(Value::as_str) {
                    Some("file") => pages.push(path.to_string()),
                    Some("dir") => stack.push(path.to_string()),
                    _ => {}
                }
            }
            if pages.len() > KB_BUNDLE_PAGES_MAX {
                return Err(Failure::Failed(format!(
                    "the knowledge base holds more than {KB_BUNDLE_PAGES_MAX} pages, the most one export carries"
                )));
            }
        }
        pages.sort();
        Ok(pages)
    }

    /// A page's content and version, or `None` when it is not there.
    async fn read(&self, path: &str) -> Result<Option<(String, String)>, Failure> {
        match self.call("brain_get", json!({"path": path})).await {
            Ok(page) => {
                let field = |name: &str| page[name].as_str().unwrap_or_default().to_string();
                Ok(Some((field("content"), field("version"))))
            }
            Err(failure) if error_code(&failure) == Some("not_found") => Ok(None),
            Err(failure) => Err(failure),
        }
    }

    /// A page's version, or `None` when it is not there.
    async fn version(&self, path: &str) -> Result<Option<String>, Failure> {
        Ok(self.read(path).await?.map(|(_, version)| version))
    }

    /// Write a page while it still has `guard`; the new version, or `None` on
    /// a conflict.
    async fn put(&self, path: &str, content: &str, guard: &str) -> Result<Option<String>, Failure> {
        let arguments = json!({"path": path, "content": content, "if_version": guard});
        match self.call("brain_put", arguments).await {
            Ok(written) => Ok(Some(
                written["version"].as_str().unwrap_or_default().to_string(),
            )),
            Err(failure) if error_code(&failure) == Some("conflict") => Ok(None),
            Err(failure) => Err(failure),
        }
    }

    /// Delete a page while it still has `guard`.
    async fn delete(&self, path: &str, guard: &str) -> Result<Deleted, Failure> {
        let arguments = json!({"path": path, "if_version": guard});
        match self.call("brain_delete", arguments).await {
            Ok(_) => Ok(Deleted::Yes),
            Err(failure) => match error_code(&failure) {
                Some("conflict") => Ok(Deleted::Changed),
                Some("not_found") => Ok(Deleted::Gone),
                _ => Err(failure),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_path_maps_to_a_file_inside_the_folder() {
        assert_eq!(
            relative_file("/fs/runbooks/deploy.md").ok().flatten(),
            Some(PathBuf::from("runbooks/deploy.md"))
        );
    }

    #[test]
    fn a_page_path_that_climbs_or_is_absolute_is_refused() {
        for page in [
            "/fs/../etc/passwd",
            "/fs/a/../../b",
            "/fs//b",
            "/kv/x",
            "/fs/a\\b",
        ] {
            assert!(relative_file(page).is_err(), "{page} was not refused");
        }
    }

    #[test]
    fn a_dot_named_page_is_left_out() {
        assert!(matches!(relative_file("/fs/.git/config"), Ok(None)));
        assert!(matches!(
            relative_file(&format!("/fs/{MANIFEST}")),
            Ok(None)
        ));
    }

    #[test]
    fn paths_that_differ_only_in_case_or_normalisation_collide() {
        for pair in [
            ["/fs/Notes.md", "/fs/notes.md"],
            ["/fs/Runbooks/a.md", "/fs/runbooks/b.md"],
            ["/fs/caf\u{e9}.md", "/fs/cafe\u{301}.md"],
        ] {
            assert!(
                check_collisions(pair.into_iter(), "refusing").is_err(),
                "{pair:?} was not refused"
            );
        }
        let distinct = ["/fs/runbooks/a.md", "/fs/runbooks/b.md", "/fs/notes.md"];
        assert!(check_collisions(distinct.into_iter(), "refusing").is_ok());
    }

    #[test]
    fn the_plan_writes_only_what_the_hub_has_not_changed() {
        assert_eq!(plan_write("v1", Some("v1"), Some("v0")), Action::Unchanged);
        assert_eq!(plan_write("v1", None, None), Action::Create);
        assert_eq!(plan_write("v1", Some("v0"), Some("v0")), Action::Update);
        assert!(matches!(
            plan_write("v1", Some("v2"), Some("v0")),
            Action::Conflict(_)
        ));
        assert!(matches!(
            plan_write("v1", Some("v2"), None),
            Action::Conflict(_)
        ));
        assert!(matches!(
            plan_write("v1", None, Some("v0")),
            Action::Conflict(_)
        ));
    }

    #[test]
    fn a_page_the_folder_left_as_exported_is_left_to_the_hub() {
        assert_eq!(plan_write("v0", Some("v2"), Some("v0")), Action::HubNewer);
        assert_eq!(plan_write("v0", None, Some("v0")), Action::HubNewer);
    }

    #[test]
    fn a_page_missing_from_the_folder_is_deleted_only_while_the_hub_has_the_export() {
        assert_eq!(plan_missing(None, "v0", true), None);
        assert_eq!(plan_missing(None, "v0", false), None);
        assert_eq!(plan_missing(Some("v0"), "v0", false), Some(Action::Kept));
        assert_eq!(plan_missing(Some("v2"), "v0", false), Some(Action::Kept));
        assert_eq!(plan_missing(Some("v0"), "v0", true), Some(Action::Delete));
        assert!(matches!(
            plan_missing(Some("v2"), "v0", true),
            Some(Action::Conflict(_))
        ));
    }

    #[test]
    fn a_decomposed_file_name_is_the_composed_page() {
        assert_eq!(page_name("cafe\u{301}.md"), "caf\u{e9}.md");
    }

    #[test]
    fn a_folder_page_that_differs_only_in_case_from_a_kept_hub_page_collides() {
        let folder = ["/fs/Notes.md"];
        let hub = ["/fs/notes.md", "/fs/NOTES.md"];
        assert!(check_collisions_with(folder.into_iter(), hub.into_iter(), "refusing").is_err());
        // Two hub pages are not the folder's to reconcile.
        assert!(
            check_collisions_with(["/fs/a.md"].into_iter(), hub.into_iter(), "refusing").is_ok()
        );
    }
}
