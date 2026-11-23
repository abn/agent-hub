//! The credential resolver against the shared contract fixtures, and the
//! `config.toml` store it reads and writes.
#![cfg(feature = "client")]

mod common;

use std::path::{Path, PathBuf};

use agent_hub::client::credential::{
    self, ConfigView, Credential, Pairing, ProjectFile, ResolveInput, Resolved, State,
    WriteOptions, parse_config, rekey, resolve, trust, write,
};
use common::temp::TempDir;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    name: String,
    input: ResolveInput,
    expect: Resolved,
}

fn fixtures() -> Vec<Fixture> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/contract/fixtures");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("fixtures dir") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with("credential-") && name.ends_with(".json") {
            let text = std::fs::read_to_string(&path).expect("read fixture");
            found.push(
                serde_json::from_str::<Fixture>(&text)
                    .unwrap_or_else(|err| panic!("{name}: {err}")),
            );
        }
    }
    found
}

#[test]
fn every_credential_fixture_resolves_as_expected() {
    let all = fixtures();
    assert!(all.len() > 20, "found only {} fixtures", all.len());
    for fixture in all {
        assert_eq!(resolve(&fixture.input), fixture.expect, "{}", fixture.name);
    }
}

fn root() -> TempDir {
    TempDir::new("credential")
}

fn config_path(dir: &TempDir) -> PathBuf {
    dir.join("config.toml")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read config")
}

fn credentials(path: &Path) -> Vec<Credential> {
    parse_config(&read(path)).expect("parse").credentials
}

fn entry(url: &str, member: &str, token: &str) -> Credential {
    Credential {
        url: url.to_string(),
        member: Some(member.to_string()),
        token: token.to_string(),
        default: false,
    }
}

fn opts() -> WriteOptions {
    WriteOptions::default()
}

const HUB: &str = "https://hub.example.net";

#[test]
fn a_write_appends_a_credential_and_keeps_the_rest_of_the_file() {
    let dir = root();
    let path = config_path(&dir);
    std::fs::write(
        &path,
        "[client]\nurl = \"https://hub.example.net\"\n# keep\n",
    )
    .unwrap();
    write(&path, HUB, "laptop", "t-1", &opts()).unwrap();
    write(&path, HUB, "desktop", "t-2", &opts()).unwrap();
    assert!(read(&path).contains("# keep"));
    assert_eq!(
        credentials(&path),
        vec![entry(HUB, "laptop", "t-1"), entry(HUB, "desktop", "t-2")]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert!(!dir.join("config.toml.lock").exists());
}

#[test]
fn a_write_replaces_the_token_of_a_member_matching_the_url_normalised() {
    let dir = root();
    let path = config_path(&dir);
    write(&path, "https://Hub.example.net/", "laptop", "old", &opts()).unwrap();
    write(&path, HUB, "laptop", "new", &opts()).unwrap();
    let got = credentials(&path);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].token, "new");
}

#[test]
fn rekey_renames_the_member_and_keeps_the_token() {
    let dir = root();
    let path = config_path(&dir);
    write(&path, HUB, "laptop", "t-1", &opts()).unwrap();
    write(&path, HUB, "desktop", "t-2", &opts()).unwrap();
    rekey(&path, HUB, "laptop", "laptop-2", &opts()).unwrap();
    assert_eq!(
        credentials(&path),
        vec![entry(HUB, "laptop-2", "t-1"), entry(HUB, "desktop", "t-2")]
    );
}

#[test]
fn a_stored_credential_reads_back_through_the_resolver() {
    let dir = root();
    let path = config_path(&dir);
    write(&path, HUB, "laptop", "t-1", &opts()).unwrap();
    let input = ResolveInput {
        project_root: Some("/r".to_string()),
        config: parse_config(&read(&path)).unwrap(),
        ..ResolveInput::default()
    };
    let got = resolve(&input);
    assert_eq!(got.state, State::Ok);
    assert_eq!(got.member.as_deref(), Some("laptop"));
    assert_eq!(got.token.as_deref(), Some("t-1"));
}

#[test]
fn client_member_timeout_and_pairings_parse() {
    let config = parse_config(
        "[client]\nurl = \"https://h\"\nmember = \"m\"\ntimeout = 120\n[trust]\npairings = [ { path = \"/p\", url = \"https://h\", member = \"m2\" } ]\n",
    )
    .unwrap();
    assert_eq!(config.client.url.as_deref(), Some("https://h"));
    assert_eq!(config.client.member.as_deref(), Some("m"));
    assert_eq!(
        config.trust.pairings,
        vec![Pairing {
            path: "/p".to_string(),
            url: "https://h".to_string(),
            member: Some("m2".to_string()),
        }]
    );
}

#[test]
fn a_pairing_written_as_an_array_of_tables_is_not_read() {
    let config =
        parse_config("[[trust.pairings]]\npath = \"/p\"\nurl = \"https://h\"\nmember = \"m\"\n")
            .unwrap();
    assert!(config.trust.pairings.is_empty());
}

#[test]
fn a_legacy_client_token_folds_in_as_one_unnamed_credential_and_is_never_touched() {
    let dir = root();
    let path = config_path(&dir);
    let legacy = "[client]\nurl = \"https://h\"\ntoken = \"legacy\" # old\n";
    std::fs::write(&path, legacy).unwrap();
    assert_eq!(
        parse_config(legacy).unwrap().credentials,
        vec![Credential {
            url: "https://h".to_string(),
            member: None,
            token: "legacy".to_string(),
            default: false,
        }]
    );
    write(&path, "https://h", "laptop", "named", &opts()).unwrap();
    rekey(&path, "https://h", "laptop", "laptop-2", &opts()).unwrap();
    let text = read(&path);
    assert!(text.starts_with(legacy), "{text}");
    assert_eq!(parse_config(&text).unwrap().credentials.len(), 2);
}

#[test]
fn two_writers_in_separate_threads_lose_no_entry() {
    let dir = root();
    let path = config_path(&dir);
    let handles: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|tag| {
            let path = path.clone();
            std::thread::spawn(move || {
                for i in 0..25 {
                    write(
                        &path,
                        HUB,
                        &format!("{tag}{i}"),
                        &format!("tok-{tag}{i}"),
                        &opts(),
                    )
                    .expect("write");
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("thread");
    }
    let mut got: Vec<String> = credentials(&path)
        .into_iter()
        .map(|c| format!("{} {}", c.member.unwrap_or_default(), c.token))
        .collect();
    got.sort();
    let mut want: Vec<String> = ["a", "b"]
        .iter()
        .flat_map(|t| (0..25).map(move |i| format!("{t}{i} tok-{t}{i}")))
        .collect();
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn a_leftover_temp_file_is_replaced_not_reused_and_the_result_is_private() {
    let dir = root();
    let path = config_path(&dir);
    let temp = dir.join(format!("config.toml.tmp.{}", std::process::id()));
    std::fs::write(&temp, "leftover secret").unwrap();
    write(&path, HUB, "laptop", "t-1", &opts()).unwrap();
    assert!(!read(&path).contains("leftover"));
    assert!(!temp.exists());
}

#[test]
fn a_commented_out_pairing_never_counts() {
    let pairing = |p: &str| format!("{{ path = \"{p}\", url = \"https://h\", member = \"m\" }}");
    let text = format!(
        "[trust]\npairings = [\n  # {},\n  {}, # {}\n]\n",
        pairing("/r"),
        pairing("/other"),
        pairing("/r")
    );
    let config = parse_config(&text).unwrap();
    assert_eq!(config.trust.pairings.len(), 1);
    assert_eq!(config.trust.pairings[0].path, "/other");
    let with_credential =
        format!("{text}[[credential]]\nurl = \"https://h\"\nmember = \"m\"\ntoken = \"t\"\n");
    let got = resolve(&ResolveInput {
        project_root: Some("/r".to_string()),
        project_file: Some(ProjectFile {
            url: Some("https://h".to_string()),
            member: Some("m".to_string()),
            ..ProjectFile::default()
        }),
        config: parse_config(&with_credential).unwrap(),
        ..ResolveInput::default()
    });
    assert_eq!(got.state, State::NeedsTrust);
    assert_eq!(got.token, None);
}

#[test]
fn a_hash_inside_a_string_is_kept_and_a_comment_after_the_array_is_ignored() {
    let text = "[trust]\npairings = [ { path = \"/r#1\", url = 'https://h', member = \"m\" } ] # { path = \"/x\", url = \"u\" }\n";
    let config = parse_config(text).unwrap();
    assert_eq!(config.trust.pairings.len(), 1);
    assert_eq!(config.trust.pairings[0].path, "/r#1");
}

#[test]
fn a_commented_out_credential_block_or_key_never_counts() {
    let text = [
        "# [[credential]]",
        "# url = \"https://h\"",
        "[[credential]]",
        "url = \"https://h\"",
        "# member = \"ghost\"",
        "member = \"real\" # not \"other\"",
        "token = \"t\" # [[credential]]",
        "",
    ]
    .join("\n");
    assert_eq!(
        parse_config(&text).unwrap().credentials,
        vec![entry("https://h", "real", "t")]
    );
}

#[test]
fn a_commented_out_block_is_no_match_for_a_write_and_survives_it() {
    let dir = root();
    let path = config_path(&dir);
    std::fs::write(
        &path,
        "# [[credential]]\n# url = \"https://h\"\n# member = \"m\"\n# token = \"old\"\n",
    )
    .unwrap();
    write(&path, "https://h", "m", "new", &opts()).unwrap();
    let text = read(&path);
    assert!(text.contains("# token = \"old\""));
    assert_eq!(credentials(&path), vec![entry("https://h", "m", "new")]);
}

#[test]
fn multiline_and_literal_strings_read() {
    let text = "[[credential]]\nurl = \"https://h\"\nmember = 'm'\ntoken = \"\"\"\nabc\\\n   def\n\"\"\"\n";
    assert_eq!(
        parse_config(text).unwrap().credentials,
        vec![entry("https://h", "m", "abcdef\n")]
    );
}

fn edited(dir: &TempDir, text: &str, token: &str) -> String {
    let path = config_path(dir);
    std::fs::write(&path, text).unwrap();
    write(&path, "https://h", "m", token, &opts()).unwrap();
    read(&path)
}

fn block(token_line: &str) -> String {
    format!("[[credential]]\nurl = \"https://h\"\nmember = \"m\"\n{token_line}\n")
}

#[test]
fn a_multiline_basic_token_is_replaced_whole() {
    let dir = root();
    let out = edited(
        &dir,
        &format!(
            "{}[client]\nurl = \"https://h\"\n",
            block("token = \"\"\"\nline one\nline two\"\"\"  # keep")
        ),
        "new",
    );
    assert_eq!(
        out,
        format!(
            "{}[client]\nurl = \"https://h\"\n",
            block("token = \"new\"  # keep")
        )
    );
}

#[test]
fn multiline_and_single_line_literal_tokens_are_replaced_whole() {
    let dir = root();
    assert_eq!(
        edited(&dir, &block("token = '''\na\nb'''"), "new"),
        block("token = \"new\"")
    );
    assert_eq!(
        edited(&dir, &block("token = 'a#b' # note"), "new"),
        block("token = \"new\" # note")
    );
}

#[test]
fn inline_comments_survive_a_replace_and_a_rekey() {
    let dir = root();
    let path = config_path(&dir);
    std::fs::write(
        &path,
        "[[credential]]\nurl = \"https://h\" # hub\nmember = \"old\" # who\ntoken = \"t\" # secret\n",
    )
    .unwrap();
    rekey(&path, "https://h", "old", "new", &opts()).unwrap();
    write(&path, "https://h", "new", "t2", &opts()).unwrap();
    assert_eq!(
        read(&path),
        "[[credential]]\nurl = \"https://h\" # hub\nmember = \"new\" # who\ntoken = \"t2\" # secret\n"
    );
}

#[test]
fn crlf_is_kept_on_a_replace_an_inserted_field_and_an_appended_block() {
    let dir = root();
    let path = config_path(&dir);
    std::fs::write(
        &path,
        "[[credential]]\r\nurl = \"https://h\"\r\nmember = \"m\"\r\n",
    )
    .unwrap();
    write(&path, "https://h", "m", "t1", &opts()).unwrap();
    write(&path, "https://h", "m", "t2", &opts()).unwrap();
    write(&path, "https://h", "other", "t3", &opts()).unwrap();
    let out = read(&path);
    assert!(
        !out.replace("\r\n", "").contains(['\r', '\n']),
        "a bare line break in {out:?}"
    );
    let tokens: Vec<String> = credentials(&path).into_iter().map(|c| c.token).collect();
    assert_eq!(tokens, ["t2", "t3"]);
}

#[test]
fn an_lf_file_stays_lf_and_a_missing_final_newline_stays_missing() {
    let dir = root();
    let path = config_path(&dir);
    std::fs::write(&path, "[client]\nurl = \"https://h\"").unwrap();
    write(&path, "https://h", "m", "t1", &opts()).unwrap();
    let out = read(&path);
    assert!(!out.contains('\r'));
    assert!(!out.ends_with('\n'));
    write(&path, "https://h", "m", "t2", &opts()).unwrap();
    let out = read(&path);
    assert!(out.ends_with("token = \"t2\""));
    assert_eq!(credentials(&path), vec![entry("https://h", "m", "t2")]);
}

#[test]
fn a_missing_token_is_inserted_into_a_block_at_the_end_of_a_file_with_no_newline() {
    let dir = root();
    let out = edited(
        &dir,
        "[[credential]]\nurl = \"https://h\"\nmember = \"m\"",
        "new",
    );
    assert_eq!(
        out,
        "[[credential]]\nurl = \"https://h\"\nmember = \"m\"\ntoken = \"new\""
    );
}

#[test]
fn a_new_file_ends_with_a_newline() {
    let dir = root();
    assert!(edited(&dir, "", "new").ends_with('\n'));
}

#[test]
fn a_pairing_is_recorded_in_the_inline_array_form_and_never_twice() {
    let dir = root();
    let path = config_path(&dir);
    std::fs::write(&path, "[client]\nurl = \"https://h\"\n").unwrap();
    let first = Pairing {
        path: "/work/a".to_string(),
        url: "https://h".to_string(),
        member: Some("m".to_string()),
    };
    trust(&path, &first, &opts()).unwrap();
    trust(&path, &first, &opts()).unwrap();
    let second = Pairing {
        path: "/work/b".to_string(),
        url: "https://h".to_string(),
        member: None,
    };
    trust(&path, &second, &opts()).unwrap();
    let text = read(&path);
    assert!(!text.contains("[[trust.pairings]]"), "{text}");
    assert_eq!(
        parse_config(&text).unwrap().trust.pairings,
        vec![first, second]
    );
    assert!(text.starts_with("[client]\nurl = \"https://h\"\n"));
}

#[test]
fn a_pairing_joins_a_multiline_array_and_keeps_its_comments() {
    let dir = root();
    let path = config_path(&dir);
    std::fs::write(
        &path,
        "[trust]\npairings = [\n  { path = \"/a\", url = \"https://h\", member = \"m\" }, # first\n]\n",
    )
    .unwrap();
    let added = Pairing {
        path: "/b".to_string(),
        url: "https://h".to_string(),
        member: Some("m".to_string()),
    };
    trust(&path, &added, &opts()).unwrap();
    let text = read(&path);
    assert!(text.contains("# first"));
    let got = parse_config(&text).unwrap().trust.pairings;
    assert_eq!(got.len(), 2);
    assert_eq!(got[1], added);
}

#[test]
fn members_compare_exactly_after_trim_in_inference_and_trust() {
    let config = |member: &str| ConfigView {
        credentials: vec![entry("https://h", member, "t")],
        trust: credential::Trust {
            pairings: vec![Pairing {
                path: "/r".to_string(),
                url: "https://h".to_string(),
                member: Some(" m ".to_string()),
            }],
        },
        ..ConfigView::default()
    };
    let got = resolve(&ResolveInput {
        project_root: Some("/r".to_string()),
        project_file: Some(ProjectFile {
            url: Some("https://h".to_string()),
            member: Some("m".to_string()),
            ..ProjectFile::default()
        }),
        config: config("  m "),
        ..ResolveInput::default()
    });
    assert_eq!(got.state, State::Ok);
    assert_eq!(got.member.as_deref(), Some("m"));
    assert_eq!(got.token.as_deref(), Some("t"));
    let inferred = resolve(&ResolveInput {
        project_root: Some("/r".to_string()),
        config: config("  m "),
        ..ResolveInput::default()
    });
    assert_eq!(inferred.member.as_deref(), Some("m"));
    assert_eq!(inferred.token.as_deref(), Some("t"));
}

#[test]
fn members_compare_after_trim_in_lookup_write_and_rekey() {
    let dir = root();
    let path = config_path(&dir);
    std::fs::write(
        &path,
        "[[credential]]\nurl = \"https://h\"\nmember = \" m \"\ntoken = \"old\"\n",
    )
    .unwrap();
    write(&path, "https://h", "m", "new", &opts()).unwrap();
    assert_eq!(credentials(&path), vec![entry("https://h", " m ", "new")]);
    rekey(&path, "https://h", "m", "n", &opts()).unwrap();
    assert_eq!(credentials(&path), vec![entry("https://h", "n", "new")]);
}

#[test]
fn a_project_file_reads_and_refuses_a_token() {
    let dir = root();
    std::fs::write(
        dir.join(".agent-hub.toml"),
        "project = \"homelab\"\nurl = \"https://h\"\nmember = \"laptop-secret\"\n",
    )
    .unwrap();
    let (file, lines) = credential::read_project_file(&dir).unwrap();
    assert_eq!(
        file,
        Some(ProjectFile {
            project: Some("homelab".to_string()),
            url: Some("https://h".to_string()),
            member: Some("laptop-secret".to_string()),
        })
    );
    assert!(lines.is_empty());

    std::fs::write(dir.join(".agent-hub.toml"), "token = \"nope\"\n").unwrap();
    let err = credential::read_project_file(&dir).expect_err("a token is refused");
    assert!(
        err.to_string()
            .contains(".agent-hub.toml must not hold a token; it is shared"),
        "{err}"
    );
}

#[test]
fn a_legacy_pin_file_is_ignored_with_its_context_line() {
    let dir = root();
    std::fs::write(dir.join(".agent-hub"), "homelab\nsecond line\n").unwrap();
    let (file, lines) = credential::read_project_file(&dir).unwrap();
    assert_eq!(file, None);
    assert_eq!(
        lines,
        vec![
            "found .agent-hub; this release reads .agent-hub.toml: project = \"homelab\""
                .to_string()
        ]
    );
}

#[test]
fn the_project_root_is_the_nearest_directory_holding_a_dot_git() {
    let dir = root();
    std::fs::create_dir_all(dir.join("repo/.git")).unwrap();
    std::fs::create_dir_all(dir.join("repo/a/b")).unwrap();
    assert_eq!(
        credential::project_root(&dir.join("repo/a/b")),
        Some(dir.join("repo"))
    );
}
