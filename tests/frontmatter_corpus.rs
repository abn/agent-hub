//! The frontmatter corpus, byte for byte.
//!
//! Every case in `tests/fixtures/frontmatter/manifest.json` is loaded and run.
//! A case that cannot be loaded, an operation this runner does not know, and a
//! fixture file no case refers to are all failures: a corpus that silently
//! skips is no contract. The format is described in the fixture directory's
//! `README.md`, which a second implementation reads.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use agent_hub::okf::{
    Change, PromoteParams, patch_frontmatter, promote_frontmatter, review_frontmatter,
};
use serde::Deserialize;

const DIR: &str = "tests/fixtures/frontmatter";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    cases: Vec<CaseMeta>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseMeta {
    id: String,
    description: String,
    input: String,
    patch: String,
    expected: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Patch {
        changes: Vec<Change>,
    },
    Review {
        actor: String,
        time: String,
    },
    Promote {
        #[serde(rename = "type")]
        page_type: Option<String>,
        title: Option<String>,
        description: Option<String>,
        tags: Option<Vec<String>>,
        session_name: String,
        session_id: String,
        from_path: String,
    },
}

fn read(name: &str) -> Vec<u8> {
    fs::read(Path::new(DIR).join(name)).unwrap_or_else(|err| panic!("read {name}: {err}"))
}

/// Run one case to its output bytes or its refusal code.
fn run(case: &CaseMeta) -> Result<Vec<u8>, String> {
    let input = String::from_utf8(read(&case.input))
        .unwrap_or_else(|err| panic!("{}: input is not UTF-8: {err}", case.id));
    let patch_bytes = read(&case.patch);

    // The file must be JSON. Beyond that, a value the patcher cannot write
    // is a refusal of the case, not a broken fixture.
    serde_json::from_slice::<serde_json::Value>(&patch_bytes)
        .unwrap_or_else(|err| panic!("{}: patch file is not JSON: {err}", case.id));
    let operation: Operation = match serde_json::from_slice(&patch_bytes) {
        Ok(operation) => operation,
        Err(err) if err.to_string().contains("invalid_value") => {
            return Err("invalid_value".to_string());
        }
        Err(err) => panic!("{}: cannot load the patch file: {err}", case.id),
    };

    let result = match &operation {
        Operation::Patch { changes } => patch_frontmatter(&input, changes),
        Operation::Review { actor, time } => review_frontmatter(&input, actor, time),
        Operation::Promote {
            page_type,
            title,
            description,
            tags,
            session_name,
            session_id,
            from_path,
        } => promote_frontmatter(
            &input,
            &PromoteParams {
                page_type: page_type.as_deref(),
                title: title.as_deref(),
                description: description.as_deref(),
                tags: tags.as_deref(),
                session_name,
                session_id,
                from_path,
            },
        ),
    };
    result
        .map(String::into_bytes)
        .map_err(|err| err.code().to_string())
}

fn manifest() -> Manifest {
    serde_json::from_slice(&read("manifest.json")).expect("parse manifest.json")
}

#[test]
fn every_case_matches_byte_for_byte() {
    let manifest = manifest();
    assert_eq!(manifest.version, 2, "unknown manifest version");
    assert!(
        manifest.cases.len() >= 60,
        "the corpus shrank to {} cases",
        manifest.cases.len()
    );

    let mut failures = Vec::new();
    for case in &manifest.cases {
        assert!(!case.description.is_empty(), "{}: no description", case.id);
        let want: Result<Vec<u8>, String> = match (&case.expected, &case.error) {
            (Some(expected), None) => Ok(read(expected)),
            (None, Some(code)) => Err(code.clone()),
            _ => panic!("{}: exactly one of expected and error is required", case.id),
        };
        let got = run(case);
        if got != want {
            let show = |side: &Result<Vec<u8>, String>| match side {
                Ok(bytes) => format!("{:?}", String::from_utf8_lossy(bytes)),
                Err(code) => format!("refusal {code}"),
            };
            failures.push(format!(
                "{} ({})\n     got: {}\n  wanted: {}",
                case.id,
                case.description,
                show(&got),
                show(&want)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        manifest.cases.len(),
        failures.join("\n")
    );
}

#[test]
fn every_fixture_file_belongs_to_a_case() {
    let manifest = manifest();
    let mut referenced: BTreeSet<String> = ["manifest.json", "README.md"]
        .iter()
        .map(|name| name.to_string())
        .collect();
    let mut ids = BTreeSet::new();
    for case in &manifest.cases {
        assert!(ids.insert(case.id.clone()), "duplicate case id {}", case.id);
        referenced.insert(case.input.clone());
        referenced.insert(case.patch.clone());
        referenced.extend(case.expected.clone());
    }
    let on_disk: BTreeSet<String> = fs::read_dir(DIR)
        .expect("read fixture directory")
        .map(|entry| {
            entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(on_disk, referenced, "fixture files and manifest disagree");
}

#[test]
fn the_corpus_covers_every_refusal_and_operation() {
    let manifest = manifest();
    let refused: BTreeSet<&str> = manifest
        .cases
        .iter()
        .filter_map(|case| case.error.as_deref())
        .collect();
    for code in [
        "byte_order_mark",
        "carriage_return",
        "unclosed",
        "delimiter",
        "unsupported_line",
        "duplicate_key",
        "anchor",
        "not_block_sequence",
        "indentation",
        "invalid_key",
        "duplicate_change",
        "invalid_value",
    ] {
        assert!(refused.contains(code), "no case refuses with {code}");
    }
    for operation in ["patch", "review", "promote"] {
        let needle = format!("\"operation\": \"{operation}\"");
        assert!(
            manifest
                .cases
                .iter()
                .any(|case| String::from_utf8_lossy(&read(&case.patch)).contains(&needle)),
            "no case runs {operation}"
        );
    }
}
