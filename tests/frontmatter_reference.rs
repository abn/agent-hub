//! The Rust patcher as a reference a second implementation is compared with.
//!
//! The fixture corpus cannot hold every input, so the browser module's runner
//! (`.agents/scripts/test-frontmatter.mjs --differential`) generates cases,
//! has this test answer them, and requires its own answers to be the same
//! bytes or the same refusal. The test is ignored in a normal run: it reads
//! the file named by `FRONTMATTER_CASES`, one JSON case per line, and writes
//! one JSON answer per line to the file named by `FRONTMATTER_ANSWERS`.
//!
//! A case is `{"text": ..., "op": ...}` where `op` has the shape of a corpus
//! patch file, or is `{"operation": "read"}`. An answer is `{"ok": <page>}`,
//! `{"read": <frontmatter or null>}`, `{"error": <refusal code>}`, or
//! `{"load_error": <message>}` for a case this side cannot even load.

use std::env;
use std::fs;
use std::io::{BufWriter, Write};

use agent_hub::okf::{
    Change, PromoteParams, parse_frontmatter, patch_frontmatter, promote_frontmatter,
    review_frontmatter,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    text: String,
    op: Operation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Read,
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

fn read(text: &str) -> Value {
    match parse_frontmatter(text) {
        Err(err) => json!({ "error": err.code() }),
        Ok(None) => json!({ "read": null }),
        Ok(Some(fm)) => json!({ "read": {
            "type": fm.page_type,
            "title": fm.title,
            "description": fm.description,
            "status": fm.status,
            "tags": fm.tags,
            "stale_after": fm.stale_after,
            "okf_version": fm.okf_version,
            "verified": fm.verified,
            "sources": fm.sources,
            "custom": fm.custom_keys,
            "raw": fm.raw,
        }}),
    }
}

fn answer(line: &str) -> Value {
    let case: Case = match serde_json::from_str(line) {
        Ok(case) => case,
        // The same rule the corpus runner applies to a patch file.
        Err(err) if err.to_string().contains("invalid_value") => {
            return json!({ "error": "invalid_value" });
        }
        Err(err) => return json!({ "load_error": err.to_string() }),
    };
    let text = case.text.as_str();
    let result = match &case.op {
        Operation::Read => return read(text),
        Operation::Patch { changes } => patch_frontmatter(text, changes),
        Operation::Review { actor, time } => review_frontmatter(text, actor, time),
        Operation::Promote {
            page_type,
            title,
            description,
            tags,
            session_name,
            session_id,
            from_path,
        } => promote_frontmatter(
            text,
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
    match result {
        Ok(page) => json!({ "ok": page }),
        Err(err) => json!({ "error": err.code() }),
    }
}

#[test]
#[ignore = "driven by .agents/scripts/test-frontmatter.mjs --differential"]
fn answer_the_cases_in_a_file() {
    let cases = env::var("FRONTMATTER_CASES").expect("FRONTMATTER_CASES names the case file");
    let answers =
        env::var("FRONTMATTER_ANSWERS").expect("FRONTMATTER_ANSWERS names the answer file");
    let input = fs::read_to_string(&cases).unwrap_or_else(|err| panic!("read {cases}: {err}"));
    let mut out = BufWriter::new(
        fs::File::create(&answers).unwrap_or_else(|err| panic!("create {answers}: {err}")),
    );
    for line in input.lines() {
        writeln!(out, "{}", answer(line)).expect("write an answer");
    }
    out.flush().expect("flush the answers");
}
