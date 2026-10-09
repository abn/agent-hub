//! A project knowledge base exported to a folder and imported back, end to end
//! against a running hub.
//!
//! The folder is where an operator edits, diffs and commits the pages, so what
//! matters is that a round trip loses nothing, that an import never writes over
//! a page someone changed on the hub since the export, that a folder the lint
//! refuses writes nothing at all, and that neither direction reads or writes
//! outside the folder it was given.

use std::io::Write;
use std::path::Path;
use std::process::{Output, Stdio};

mod common;

use common::hub::{Hub, PROJECT};
use common::temp::TempDir;

const INDEX: &str = "\
---
okf_version: \"0.2\"
title: Homelab
---
# Homelab

- [Deploy](runbooks/deploy.md)
";

const DEPLOY: &str = "\
---
type: Runbook
title: Deploy the hub
---
# Deploy the hub

Pull the image and restart.
";

const MANIFEST: &str = ".agent-hub-kb.json";

fn run(hub: &Hub, args: &[&str]) -> Output {
    run_with(hub, args, None)
}

fn run_with(hub: &Hub, args: &[&str], stdin: Option<&str>) -> Output {
    let mut command = hub.client();
    command
        .args(args)
        .env("HUB_TOKEN", hub.agent_token.clone())
        .env("HUB_PROJECT", PROJECT)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let Some(input) = stdin else {
        return command
            .stdin(Stdio::null())
            .output()
            .expect("run the binary");
    };
    let mut child = command
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn the binary");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(input.as_bytes())
        .expect("write the page");
    child.wait_with_output().expect("run the binary")
}

fn put(hub: &Hub, path: &str, content: &str) {
    let written = run_with(hub, &["kb", "put", path, "-"], Some(content));
    assert_eq!(written.status.code(), Some(0), "{written:?}");
}

fn get(hub: &Hub, path: &str) -> String {
    let read = run(hub, &["kb", "get", path]);
    assert_eq!(read.status.code(), Some(0), "{read:?}");
    stdout(&read)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn path_arg(dir: &Path) -> &str {
    dir.to_str().expect("a UTF-8 test path")
}

fn export(hub: &Hub, dir: &Path) -> Output {
    run(hub, &["kb", "export", "--dir", path_arg(dir)])
}

fn import(hub: &Hub, dir: &Path, flags: &[&str]) -> Output {
    let mut args = vec!["kb", "import", "--dir", path_arg(dir)];
    args.extend_from_slice(flags);
    run(hub, &args)
}

/// A hub holding the index and one runbook, exported to a fresh folder.
fn exported(tag: &str) -> (Hub, TempDir) {
    let hub = Hub::start(tag);
    put(&hub, "/fs/index.md", INDEX);
    put(&hub, "/fs/runbooks/deploy.md", DEPLOY);
    let dir = TempDir::new(&format!("{tag}-folder"));
    let output = export(&hub, &dir);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    (hub, dir)
}

#[test]
fn an_export_writes_every_page_and_an_import_writes_back_what_changed() {
    let (hub, dir) = exported("kb-bundle-round-trip");

    assert_eq!(
        std::fs::read_to_string(dir.join("index.md")).unwrap(),
        INDEX
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("runbooks/deploy.md")).unwrap(),
        DEPLOY
    );
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(MANIFEST)).unwrap()).unwrap();
    assert_eq!(manifest["project"], PROJECT);
    assert!(
        manifest["pages"]["/fs/runbooks/deploy.md"]
            .as_str()
            .is_some_and(|version| version.starts_with("sha256:")),
        "the manifest records each page's version: {manifest}"
    );

    // An import of the folder as exported changes nothing.
    let untouched = import(&hub, &dir, &[]);
    assert_eq!(untouched.status.code(), Some(0), "{untouched:?}");
    assert!(
        stdout(&untouched).contains("0 created, 0 updated, 0 deleted, 2 unchanged"),
        "{untouched:?}"
    );

    let edited = format!("{DEPLOY}\nThen check /readyz.\n");
    std::fs::write(dir.join("runbooks/deploy.md"), &edited).unwrap();
    std::fs::write(dir.join("notes.md"), "---\ntype: Concept\n---\n# Notes\n").unwrap();
    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(0), "{imported:?}");
    let out = stdout(&imported);
    assert!(out.contains("update /fs/runbooks/deploy.md"), "{out}");
    assert!(out.contains("create /fs/notes.md"), "{out}");
    assert_eq!(get(&hub, "/fs/runbooks/deploy.md"), edited);
    assert_eq!(
        get(&hub, "/fs/notes.md"),
        "---\ntype: Concept\n---\n# Notes\n"
    );

    // The manifest now holds what was written, so the next import is a no-op
    // rather than a conflict with itself.
    let again = import(&hub, &dir, &[]);
    assert_eq!(again.status.code(), Some(0), "{again:?}");
    assert!(
        stdout(&again).contains("0 created, 0 updated, 0 deleted, 3 unchanged"),
        "{again:?}"
    );
}

#[test]
fn a_page_changed_on_the_hub_since_the_export_is_a_conflict_and_is_kept() {
    let (hub, dir) = exported("kb-bundle-conflict");
    let theirs = format!("{DEPLOY}\nEdited on the hub.\n");
    put(&hub, "/fs/runbooks/deploy.md", &theirs);

    std::fs::write(
        dir.join("runbooks/deploy.md"),
        format!("{DEPLOY}\nEdited in the folder.\n"),
    )
    .unwrap();
    let index = format!("{INDEX}\nUpdated in the folder.\n");
    std::fs::write(dir.join("index.md"), &index).unwrap();
    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(1), "{imported:?}");
    let out = stdout(&imported);
    assert!(
        out.contains("conflict /fs/runbooks/deploy.md: changed on the hub since the export"),
        "{out}"
    );
    assert!(out.contains("update /fs/index.md"), "{out}");
    assert_eq!(get(&hub, "/fs/runbooks/deploy.md"), theirs);
    assert_eq!(get(&hub, "/fs/index.md"), index);
}

#[test]
fn an_edit_that_adds_an_escaping_link_refuses_with_nothing_written() {
    let (hub, dir) = exported("kb-bundle-lint");
    let manifest = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
    std::fs::write(dir.join("index.md"), format!("{INDEX}\nClean edit.\n")).unwrap();
    std::fs::write(
        dir.join("runbooks/deploy.md"),
        format!("{DEPLOY}\nSee [the host](../../etc/hosts.md).\n"),
    )
    .unwrap();

    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(1), "{imported:?}");
    let err = stderr(&imported);
    assert!(
        err.contains("link_escapes_bundle /fs/runbooks/deploy.md:9"),
        "the refusal names the finding: {err}"
    );
    assert!(err.contains("nothing was written"), "{err}");
    assert_eq!(get(&hub, "/fs/runbooks/deploy.md"), DEPLOY);
    // The clean edit beside it is not written either, and nor is the manifest.
    assert_eq!(get(&hub, "/fs/index.md"), INDEX);
    assert_eq!(
        std::fs::read_to_string(dir.join(MANIFEST)).unwrap(),
        manifest
    );
}

#[test]
fn a_hub_page_with_an_escaping_link_round_trips() {
    let hub = Hub::start("kb-bundle-escaping-link");
    let page = format!("{DEPLOY}\nSee [the host](../../etc/hosts.md).\n");
    put(&hub, "/fs/index.md", INDEX);
    put(&hub, "/fs/runbooks/deploy.md", &page);
    let dir = TempDir::new("kb-bundle-escaping-link-folder");
    let exported = export(&hub, &dir);
    assert_eq!(exported.status.code(), Some(0), "{exported:?}");

    let index = format!("{INDEX}\nUpdated in the folder.\n");
    std::fs::write(dir.join("index.md"), &index).unwrap();
    let imported = import(&hub, &dir, &[]);

    // The hub already holds the page as it is, so its finding is a warning and
    // the import writes the page that changed.
    assert_eq!(imported.status.code(), Some(0), "{imported:?}");
    assert!(
        stderr(&imported).contains("lint link_escapes_bundle /fs/runbooks/deploy.md:9"),
        "{imported:?}"
    );
    assert_eq!(get(&hub, "/fs/index.md"), index);
    assert_eq!(get(&hub, "/fs/runbooks/deploy.md"), page);
}

#[test]
fn an_incomplete_bundle_is_reported_and_still_imported() {
    let (hub, dir) = exported("kb-bundle-warnings");
    // No log, a section without an index, and a page without a type are what a
    // lenient hub leaves behind; they warn and do not refuse.
    std::fs::write(dir.join("runbooks/untyped.md"), "# Untyped\n").unwrap();

    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(0), "{imported:?}");
    let err = stderr(&imported);
    assert!(err.contains("lint missing_log"), "{err}");
    assert!(
        err.contains("lint missing_frontmatter /fs/runbooks/untyped.md"),
        "{err}"
    );
    assert_eq!(get(&hub, "/fs/runbooks/untyped.md"), "# Untyped\n");
}

#[test]
fn a_hub_page_with_a_broken_link_round_trips() {
    let hub = Hub::start("kb-bundle-broken-link");
    let page = format!("{DEPLOY}\nSee [the plan](plan.md).\n");
    put(&hub, "/fs/index.md", INDEX);
    put(&hub, "/fs/runbooks/deploy.md", &page);
    let dir = TempDir::new("kb-bundle-broken-link-folder");
    let exported = export(&hub, &dir);
    assert_eq!(exported.status.code(), Some(0), "{exported:?}");

    let index = format!("{INDEX}\nUpdated in the folder.\n");
    std::fs::write(dir.join("index.md"), &index).unwrap();
    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(0), "{imported:?}");
    assert!(
        stderr(&imported).contains("lint broken_link /fs/runbooks/deploy.md:9"),
        "the broken link is a warning: {imported:?}"
    );
    assert!(
        stdout(&imported).contains("update /fs/index.md"),
        "{imported:?}"
    );
    assert_eq!(get(&hub, "/fs/index.md"), index);
    assert_eq!(get(&hub, "/fs/runbooks/deploy.md"), page);
}

#[test]
fn an_export_refuses_to_write_through_a_link_out_of_the_folder() {
    let hub = Hub::start("kb-bundle-traversal");
    put(&hub, "/fs/index.md", INDEX);
    put(&hub, "/fs/runbooks/deploy.md", DEPLOY);
    let dir = TempDir::new("kb-bundle-traversal-folder");
    let outside = TempDir::new("kb-bundle-traversal-outside");
    std::os::unix::fs::symlink(outside.path(), dir.join("runbooks")).unwrap();

    let output = run(&hub, &["kb", "export", "--dir", path_arg(&dir), "--force"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("is a symbolic link"), "{output:?}");
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    // Every target is checked first, so the page that could be written was not.
    assert!(!dir.join("index.md").exists());
    assert!(!dir.join(MANIFEST).exists());
}

#[test]
fn an_import_refuses_a_link_out_of_the_folder() {
    let (hub, dir) = exported("kb-bundle-import-link");
    let outside = TempDir::new("kb-bundle-import-link-outside");
    std::fs::write(outside.join("secret.md"), "# Not a page\n").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.md"), dir.join("secret.md")).unwrap();

    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(1), "{imported:?}");
    assert!(
        stderr(&imported).contains("is a symbolic link"),
        "{imported:?}"
    );
    let listed = run(&hub, &["kb", "list"]);
    assert!(!stdout(&listed).contains("secret"), "{listed:?}");
}

#[test]
fn a_dry_run_lists_the_changes_and_writes_nothing() {
    let (hub, dir) = exported("kb-bundle-dry-run");
    let manifest = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
    std::fs::write(dir.join("runbooks/deploy.md"), format!("{DEPLOY}\nMore.\n")).unwrap();
    std::fs::remove_file(dir.join("index.md")).unwrap();
    std::fs::write(
        dir.join("runbooks/index.md"),
        "---\ntitle: Runbooks\n---\n# Runbooks\n\n- [Deploy](deploy.md)\n",
    )
    .unwrap();

    let planned = import(&hub, &dir, &["--dry-run", "--prune"]);

    assert_eq!(planned.status.code(), Some(0), "{planned:?}");
    let out = stdout(&planned);
    assert!(out.contains("would update /fs/runbooks/deploy.md"), "{out}");
    assert!(out.contains("would create /fs/runbooks/index.md"), "{out}");
    assert!(out.contains("would delete /fs/index.md"), "{out}");
    assert!(out.contains("dry run, nothing written"), "{out}");
    assert_eq!(get(&hub, "/fs/runbooks/deploy.md"), DEPLOY);
    assert_eq!(get(&hub, "/fs/index.md"), INDEX);
    assert_eq!(
        std::fs::read_to_string(dir.join(MANIFEST)).unwrap(),
        manifest
    );
}

#[test]
fn a_page_missing_from_the_folder_is_kept_unless_pruned() {
    let (hub, dir) = exported("kb-bundle-prune");
    std::fs::remove_file(dir.join("runbooks/deploy.md")).unwrap();
    std::fs::write(
        dir.join("index.md"),
        "---\nokf_version: \"0.2\"\n---\n# Homelab\n",
    )
    .unwrap();

    let kept = import(&hub, &dir, &[]);
    assert_eq!(kept.status.code(), Some(0), "{kept:?}");
    assert!(
        stdout(&kept).contains("keep /fs/runbooks/deploy.md"),
        "{kept:?}"
    );
    assert_eq!(get(&hub, "/fs/runbooks/deploy.md"), DEPLOY);

    let pruned = import(&hub, &dir, &["--prune"]);
    assert_eq!(pruned.status.code(), Some(0), "{pruned:?}");
    assert!(
        stdout(&pruned).contains("delete /fs/runbooks/deploy.md"),
        "{pruned:?}"
    );
    let gone = run(&hub, &["kb", "get", "/fs/runbooks/deploy.md"]);
    assert_eq!(gone.status.code(), Some(1), "{gone:?}");
}

#[test]
fn an_export_refuses_a_folder_that_is_not_empty_without_force() {
    let hub = Hub::start("kb-bundle-non-empty");
    put(&hub, "/fs/index.md", INDEX);
    let dir = TempDir::new("kb-bundle-non-empty-folder");
    std::fs::write(dir.join("index.md"), "mine\n").unwrap();

    let refused = export(&hub, &dir);

    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert!(stderr(&refused).contains("is not empty"), "{refused:?}");
    assert_eq!(
        std::fs::read_to_string(dir.join("index.md")).unwrap(),
        "mine\n"
    );
    assert!(!dir.join(MANIFEST).exists());

    let forced = run(&hub, &["kb", "export", "--dir", path_arg(&dir), "--force"]);
    assert_eq!(forced.status.code(), Some(0), "{forced:?}");
    assert_eq!(
        std::fs::read_to_string(dir.join("index.md")).unwrap(),
        INDEX
    );
}

#[test]
fn an_import_refuses_a_folder_exported_from_another_project() {
    let (hub, dir) = exported("kb-bundle-other-project");
    let text = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
    std::fs::write(
        dir.join(MANIFEST),
        text.replace(&format!("\"{PROJECT}\""), "\"elsewhere\""),
    )
    .unwrap();

    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(1), "{imported:?}");
    assert!(
        stderr(&imported).contains("was exported from project elsewhere"),
        "{imported:?}"
    );
}

#[test]
fn a_page_over_the_size_limit_is_refused_before_anything_is_written() {
    let (hub, dir) = exported("kb-bundle-size");
    std::fs::write(dir.join("index.md"), format!("{INDEX}\nChanged.\n")).unwrap();
    std::fs::write(dir.join("big.md"), "x".repeat(1024 * 1024 + 1)).unwrap();

    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(1), "{imported:?}");
    assert!(stderr(&imported).contains("page limit"), "{imported:?}");
    assert_eq!(get(&hub, "/fs/index.md"), INDEX);
}

#[test]
fn the_sync_flags_belong_to_their_own_commands() {
    let hub = Hub::start("kb-bundle-flags");
    let dir = TempDir::new("kb-bundle-flags-folder");

    for args in [
        vec!["kb", "export"],
        vec!["kb", "get", "--dir", path_arg(&dir)],
        vec!["kb", "import", "--dir", path_arg(&dir), "--force"],
        vec!["kb", "export", "--dir", path_arg(&dir), "--dry-run"],
        vec!["kb", "export", "/fs/index.md", "--dir", path_arg(&dir)],
    ] {
        let output = run(&hub, &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
    }
}

#[test]
fn an_export_refuses_pages_that_differ_only_in_case() {
    let hub = Hub::start("kb-bundle-case-export");
    put(&hub, "/fs/Notes.md", "# Notes\n");
    put(&hub, "/fs/notes.md", "# notes\n");
    let dir = TempDir::new("kb-bundle-case-export-folder");

    let refused = export(&hub, &dir);

    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert!(
        stderr(&refused).contains("/fs/Notes.md and /fs/notes.md differ only in case"),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn an_import_refuses_files_that_differ_only_in_case() {
    let (hub, dir) = exported("kb-bundle-case-import");
    // Beside runbooks/deploy.md, which the export wrote.
    std::fs::create_dir(dir.join("Runbooks")).unwrap();
    std::fs::write(dir.join("Runbooks/other.md"), "# Other\n").unwrap();

    let refused = import(&hub, &dir, &[]);

    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert!(
        stderr(&refused).contains("differ only in case"),
        "{refused:?}"
    );
    let listed = run(&hub, &["kb", "list"]);
    assert!(!stdout(&listed).contains("other.md"), "{listed:?}");
}

#[test]
fn a_forced_export_removes_the_files_of_pages_the_hub_no_longer_has() {
    let (hub, dir) = exported("kb-bundle-force-stale");
    let gone = run(&hub, &["kb", "delete", "/fs/runbooks/deploy.md"]);
    assert_eq!(gone.status.code(), Some(0), "{gone:?}");
    std::fs::write(dir.join("mine.txt"), "not a page the export wrote\n").unwrap();

    let again = run(&hub, &["kb", "export", "--dir", path_arg(&dir), "--force"]);

    assert_eq!(again.status.code(), Some(0), "{again:?}");
    assert!(
        stdout(&again).contains("removed /fs/runbooks/deploy.md"),
        "{again:?}"
    );
    assert!(!dir.join("runbooks/deploy.md").exists());
    assert!(
        dir.join("mine.txt").exists(),
        "a file it did not write stays"
    );
    // The next import does not bring the deleted page back.
    std::fs::remove_file(dir.join("mine.txt")).unwrap();
    let imported = import(&hub, &dir, &[]);
    assert_eq!(imported.status.code(), Some(0), "{imported:?}");
    assert!(!stdout(&imported).contains("create /fs"), "{imported:?}");
}

#[test]
fn an_import_refuses_a_manifest_with_a_non_canonical_page() {
    let (hub, dir) = exported("kb-bundle-manifest-keys");
    let text = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
    std::fs::write(
        dir.join(MANIFEST),
        text.replace(
            "\"/fs/runbooks/deploy.md\"",
            "\"/fs/runbooks/../runbooks/deploy.md\"",
        ),
    )
    .unwrap();

    let refused = import(&hub, &dir, &[]);

    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert!(
        stderr(&refused).contains("is not a canonical page path"),
        "{refused:?}"
    );
}

#[test]
fn an_import_refuses_a_manifest_that_lists_a_page_twice() {
    let (hub, dir) = exported("kb-bundle-manifest-twice");
    let text = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
    std::fs::write(
        dir.join(MANIFEST),
        text.replace(
            "\"pages\": {",
            "\"pages\": {\n    \"/fs/index.md\": \"sha256:00\",",
        ),
    )
    .unwrap();

    let refused = import(&hub, &dir, &[]);

    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert!(stderr(&refused).contains("listed twice"), "{refused:?}");
}

#[test]
fn a_page_gone_from_the_folder_and_the_hub_leaves_the_manifest() {
    let (hub, dir) = exported("kb-bundle-manifest-gone");
    std::fs::remove_file(dir.join("runbooks/deploy.md")).unwrap();
    let gone = run(&hub, &["kb", "delete", "/fs/runbooks/deploy.md"]);
    assert_eq!(gone.status.code(), Some(0), "{gone:?}");
    std::fs::write(
        dir.join("index.md"),
        "---\nokf_version: \"0.2\"\n---\n# Homelab\n",
    )
    .unwrap();

    let imported = import(&hub, &dir, &["--prune"]);

    assert_eq!(imported.status.code(), Some(0), "{imported:?}");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(MANIFEST)).unwrap()).unwrap();
    assert!(
        manifest["pages"].get("/fs/runbooks/deploy.md").is_none(),
        "{manifest}"
    );
}

#[test]
fn a_forced_export_refuses_a_folder_exported_from_another_project() {
    let (hub, dir) = exported("kb-bundle-force-other-project");
    let text = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
    std::fs::write(
        dir.join(MANIFEST),
        text.replace(&format!("\"{PROJECT}\""), "\"elsewhere\""),
    )
    .unwrap();
    let gone = run(&hub, &["kb", "delete", "/fs/runbooks/deploy.md"]);
    assert_eq!(gone.status.code(), Some(0), "{gone:?}");

    let refused = run(&hub, &["kb", "export", "--dir", path_arg(&dir), "--force"]);

    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert!(
        stderr(&refused).contains("was exported from project elsewhere"),
        "{refused:?}"
    );
    assert!(dir.join("runbooks/deploy.md").exists(), "its files stay");
}

#[test]
fn a_page_the_folder_left_alone_is_left_to_a_newer_hub() {
    let (hub, dir) = exported("kb-bundle-hub-newer");
    let manifest = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
    let theirs = format!("{DEPLOY}\nEdited on the hub.\n");
    put(&hub, "/fs/runbooks/deploy.md", &theirs);

    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(0), "{imported:?}");
    assert!(
        stdout(&imported).contains("hub newer /fs/runbooks/deploy.md: left as is"),
        "{imported:?}"
    );
    assert_eq!(get(&hub, "/fs/runbooks/deploy.md"), theirs);
    // The base stays the exported version, so the next export refreshes it.
    assert_eq!(
        std::fs::read_to_string(dir.join(MANIFEST)).unwrap(),
        manifest
    );
}

#[test]
fn a_page_deleted_on_the_hub_and_left_alone_in_the_folder_is_not_brought_back() {
    let (hub, dir) = exported("kb-bundle-hub-deleted");
    let gone = run(&hub, &["kb", "delete", "/fs/runbooks/deploy.md"]);
    assert_eq!(gone.status.code(), Some(0), "{gone:?}");

    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(0), "{imported:?}");
    assert!(
        stdout(&imported).contains("hub newer /fs/runbooks/deploy.md"),
        "{imported:?}"
    );
    let read = run(&hub, &["kb", "get", "/fs/runbooks/deploy.md"]);
    assert_eq!(read.status.code(), Some(1), "{read:?}");
}

#[test]
fn a_prune_of_a_page_the_hub_edited_is_a_conflict() {
    let (hub, dir) = exported("kb-bundle-prune-conflict");
    let theirs = format!("{DEPLOY}\nEdited on the hub.\n");
    put(&hub, "/fs/runbooks/deploy.md", &theirs);
    std::fs::remove_file(dir.join("runbooks/deploy.md")).unwrap();
    std::fs::write(
        dir.join("index.md"),
        "---\nokf_version: \"0.2\"\n---\n# Homelab\n",
    )
    .unwrap();

    let imported = import(&hub, &dir, &["--prune"]);

    assert_eq!(imported.status.code(), Some(1), "{imported:?}");
    assert!(
        stdout(&imported).contains("conflict /fs/runbooks/deploy.md"),
        "{imported:?}"
    );
    assert_eq!(get(&hub, "/fs/runbooks/deploy.md"), theirs);
}

#[test]
fn a_rename_that_changes_only_case_is_refused_without_prune() {
    let (hub, dir) = exported("kb-bundle-case-rename");
    std::fs::rename(
        dir.join("runbooks/deploy.md"),
        dir.join("runbooks/Deploy.md"),
    )
    .unwrap();
    std::fs::write(
        dir.join("index.md"),
        INDEX.replace("runbooks/deploy.md", "runbooks/Deploy.md"),
    )
    .unwrap();

    let refused = import(&hub, &dir, &[]);

    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert!(
        stderr(&refused).contains("/fs/runbooks/Deploy.md and /fs/runbooks/deploy.md"),
        "{refused:?}"
    );
    let listed = run(&hub, &["kb", "list"]);
    assert!(!stdout(&listed).contains("Deploy.md"), "{listed:?}");
    assert_eq!(get(&hub, "/fs/index.md"), INDEX);

    // With --prune the old spelling goes, so the rename is one page again.
    let renamed = import(&hub, &dir, &["--prune"]);
    assert_eq!(renamed.status.code(), Some(0), "{renamed:?}");
    assert_eq!(get(&hub, "/fs/runbooks/Deploy.md"), DEPLOY);
}

#[test]
fn a_decomposed_file_name_imports_as_the_composed_page() {
    let (hub, dir) = exported("kb-bundle-nfd");
    std::fs::write(dir.join("cafe\u{301}.md"), "# Cafe\n").unwrap();

    let imported = import(&hub, &dir, &[]);

    assert_eq!(imported.status.code(), Some(0), "{imported:?}");
    assert_eq!(get(&hub, "/fs/caf\u{e9}.md"), "# Cafe\n");
}
