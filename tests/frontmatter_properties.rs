//! Properties of the frontmatter patcher over generated input.
//!
//! The generator is a seeded xorshift, so a failure names a seed that
//! reproduces it. Generated pages are built from segments whose byte ranges
//! the generator records itself, so "every byte outside the key" is checked
//! against the generator's knowledge and not the patcher's own scan.

use std::time::{Duration, Instant};

use agent_hub::okf::{
    Change, PatchValue, PromoteParams, Scalar, parse_frontmatter, patch_frontmatter,
    promote_frontmatter, review_frontmatter,
};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

fn from_code_point(code: u32) -> String {
    char::from_u32(code).expect("valid code point").to_string()
}

/// Fragments that matter to a line-oriented YAML patcher.
fn alphabet() -> Vec<String> {
    let mut fragments: Vec<String> = [
        "-", "---", "...", ":", ": ", "#", " #", "\"", "'", "\\", "\r", "\n", "\r\n", "\t", " ",
        "  ", "a", "key", "verified", "sources", "[", "]", "{", "}", ",", "&", "*", "!", "|", ">",
        "0", "true", "\0",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for code in [0xE9, 0x65E5, 0x1D11E, 0x2028, 0x85, 0xFEFF] {
        fragments.push(from_code_point(code));
    }
    fragments
}

fn noise(rng: &mut Rng, alphabet: &[String], max: usize) -> String {
    let len = rng.below(max + 1);
    (0..len).map(|_| rng.pick(alphabet).as_str()).collect()
}

fn promote_params<'a>(title: &'a str, tags: &'a [String]) -> PromoteParams<'a> {
    PromoteParams {
        page_type: Some("concept"),
        title: Some(title),
        description: Some(title),
        tags: Some(tags),
        session_name: title,
        session_id: "sess_1",
        from_path: "/fs/n.md",
    }
}

#[test]
fn empty_patch_is_the_identity_on_arbitrary_bytes() {
    let alphabet = alphabet();
    for seed in 0..3000 {
        let mut rng = Rng::new(seed);
        let text = noise(&mut rng, &alphabet, 40);
        assert_eq!(
            patch_frontmatter(&text, &[]).as_deref(),
            Ok(text.as_str()),
            "seed {seed}"
        );
    }
}

#[test]
fn nothing_panics_on_arbitrary_bytes_and_output_never_reads_worse() {
    let alphabet = alphabet();
    for seed in 0..6000 {
        let mut rng = Rng::new(seed);
        // Bias half the inputs towards pages that do open a block.
        let mut text = if rng.below(2) == 0 {
            "---\n".to_string()
        } else {
            String::new()
        };
        text.push_str(&noise(&mut rng, &alphabet, 40));
        if rng.below(2) == 0 {
            text.push_str("\n---\n");
            text.push_str(&noise(&mut rng, &alphabet, 8));
        }
        let value = noise(&mut rng, &alphabet, 6);
        let tags = vec![value.clone(), noise(&mut rng, &alphabet, 3)];

        let _ = parse_frontmatter(&text);
        let outputs = [
            patch_frontmatter(
                &text,
                &[
                    ("key".to_string(), Some(PatchValue::from(value.as_str()))),
                    ("verified".to_string(), None),
                    ("tags".to_string(), Some(PatchValue::List(tags.clone()))),
                ],
            ),
            review_frontmatter(&text, &value, "2026-09-19T17:00:00Z"),
            promote_frontmatter(&text, &promote_params(&value, &tags)),
        ];
        // Whatever was accepted must still lay out: no operation may turn a
        // page the hub can read into one it cannot.
        for output in outputs.into_iter().flatten() {
            assert!(
                parse_frontmatter(&output).is_ok(),
                "seed {seed}: {text:?} became unreadable {output:?}"
            );
        }
    }
}

/// One generated key: its name and its exact bytes in the page.
struct Segment {
    key: Option<String>,
    bytes: String,
}

fn generated_page(rng: &mut Rng) -> (Vec<Segment>, &'static str) {
    let nl = if rng.below(3) == 0 { "\r\n" } else { "\n" };
    let count = 2 + rng.below(6);
    let mut segments = vec![Segment {
        key: None,
        bytes: format!("---{nl}"),
    }];
    for position in 0..count {
        if rng.below(3) == 0 {
            let filler = *rng.pick(&["# a comment  ", "", "#", "   "]);
            segments.push(Segment {
                key: None,
                bytes: format!("{filler}{nl}"),
            });
        }
        let key = format!("key{position}");
        let bytes = match rng.below(8) {
            0 => format!("{key}: plain value  {nl}"),
            1 => format!("{key}: \"quoted: # value\" # trailing{nl}"),
            2 => format!("{key}: |{nl}  para one{nl}{nl}  para two{nl}"),
            3 => format!("{key}: >-{nl}  folded{nl}{nl}{nl}  more{nl}"),
            4 => format!("{key}:{nl}  - one{nl}# inner comment{nl}  - two{nl}"),
            5 => format!("{key}:{nl}- by: a{nl}  at: b{nl}{nl}- by: c{nl}"),
            6 => format!("{key}:{nl}  nested:{nl}    deep: [1, 2]{nl}\t  tabbed: x{nl}"),
            _ => format!("{key}: [a, \"b\"]{nl}"),
        };
        segments.push(Segment {
            key: Some(key),
            bytes,
        });
    }
    let body = *rng.pick(&[
        "",
        "Body",
        "\n---\n\nkey0: not frontmatter\n",
        "\r\nBody\r\n",
    ]);
    segments.push(Segment {
        key: None,
        bytes: format!("---{nl}{body}"),
    });
    (segments, nl)
}

#[test]
fn patching_one_key_leaves_every_other_byte_alone() {
    let alphabet = alphabet();
    for seed in 0..4000 {
        let mut rng = Rng::new(seed);
        let (segments, nl) = generated_page(&mut rng);
        let page: String = segments.iter().map(|s| s.bytes.as_str()).collect();
        let keyed: Vec<usize> = (0..segments.len())
            .filter(|i| segments[*i].key.is_some())
            .collect();
        let target = *rng.pick(&keyed);
        let key = segments[target].key.clone().expect("keyed");
        let before: String = segments[..target]
            .iter()
            .map(|s| s.bytes.as_str())
            .collect();
        let after: String = segments[target + 1..]
            .iter()
            .map(|s| s.bytes.as_str())
            .collect();

        let value = noise(&mut rng, &alphabet, 5);
        let replaced = patch_frontmatter(
            &page,
            &[(key.clone(), Some(PatchValue::from(value.as_str())))],
        )
        .unwrap_or_else(|err| panic!("seed {seed}: {err}: {page:?}"));
        let middle = replaced
            .strip_prefix(before.as_str())
            .and_then(|rest| rest.strip_suffix(after.as_str()))
            .unwrap_or_else(|| panic!("seed {seed}: bytes outside {key} changed: {replaced:?}"));
        assert!(
            middle.starts_with(&format!("{key}: ")) && middle.ends_with(nl),
            "seed {seed}: {middle:?}"
        );
        assert_eq!(
            middle.matches('\n').count(),
            1,
            "seed {seed}: the value left its line: {middle:?}"
        );

        let deleted = patch_frontmatter(&page, &[(key.clone(), None)])
            .unwrap_or_else(|err| panic!("seed {seed}: {err}"));
        assert_eq!(deleted, format!("{before}{after}"), "seed {seed}");
    }
}

#[test]
fn review_leaves_every_byte_outside_verified_alone() {
    for seed in 0..2000 {
        let mut rng = Rng::new(seed);
        let (segments, nl) = generated_page(&mut rng);
        let page: String = segments.iter().map(|s| s.bytes.as_str()).collect();
        let reviewed = review_frontmatter(&page, "human", "2026-09-19T17:00:00Z")
            .unwrap_or_else(|err| panic!("seed {seed}: {err}"));
        let added = format!("verified:{nl}  - by: human{nl}    at: 2026-09-19T17:00:00Z{nl}");
        let last = segments.len() - 1;
        let head: String = segments[..last].iter().map(|s| s.bytes.as_str()).collect();
        assert_eq!(
            reviewed,
            format!("{head}{added}{}", segments[last].bytes),
            "seed {seed}"
        );

        // A second review appends under the first and touches nothing else.
        let again = review_frontmatter(&reviewed, "human", "2026-09-20T08:00:00Z")
            .unwrap_or_else(|err| panic!("seed {seed}: {err}"));
        let second = format!("  - by: human{nl}    at: 2026-09-20T08:00:00Z{nl}");
        assert_eq!(
            again,
            format!("{head}{added}{second}{}", segments[last].bytes),
            "seed {seed}"
        );
    }
}

#[test]
fn patch_then_read_returns_what_was_set() {
    let alphabet = alphabet();
    for seed in 0..4000 {
        let mut rng = Rng::new(seed);
        let title = noise(&mut rng, &alphabet, 6);
        let custom = noise(&mut rng, &alphabet, 6);
        let tags: Vec<String> = (0..rng.below(4))
            .map(|_| noise(&mut rng, &alphabet, 4))
            .collect();
        let by = noise(&mut rng, &alphabet, 4);
        let record = vec![
            ("by".to_string(), Scalar::Str(by.clone())),
            (
                "at".to_string(),
                Scalar::Str("2026-09-19T17:00:00Z".to_string()),
            ),
        ];
        let changes: Vec<Change> = vec![
            ("title".to_string(), Some(PatchValue::from(title.as_str()))),
            (
                "custom".to_string(),
                Some(PatchValue::from(custom.as_str())),
            ),
            ("tags".to_string(), Some(PatchValue::List(tags.clone()))),
            (
                "verified".to_string(),
                Some(PatchValue::Records(vec![record])),
            ),
        ];
        let page = *rng.pick(&[
            "",
            "# Body\n",
            "---\ntitle: old\ntags:\n  - x\n\n  - y\n---\n",
            "---\r\ncustom: |\r\n  a\r\n\r\n  b\r\n---\r\n",
        ]);
        let out =
            patch_frontmatter(page, &changes).unwrap_or_else(|err| panic!("seed {seed}: {err}"));
        let fm = parse_frontmatter(&out)
            .unwrap_or_else(|err| panic!("seed {seed}: {err}: {out:?}"))
            .expect("a block");
        assert_eq!(
            fm.title.as_deref(),
            Some(title.as_str()),
            "seed {seed}: {out:?}"
        );
        assert_eq!(
            fm.custom_keys,
            vec![("custom".to_string(), custom.clone())],
            "seed {seed}: {out:?}"
        );
        assert_eq!(fm.tags, tags, "seed {seed}: {out:?}");
        assert_eq!(fm.verified.len(), 1, "seed {seed}: {out:?}");
        assert_eq!(fm.verified[0].by, by, "seed {seed}: {out:?}");
    }
}

fn timed<T>(work: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let out = work();
    (out, start.elapsed())
}

/// Generous for an unoptimised build on a loaded machine, and far below what
/// copying the page once per key would cost at these sizes.
const BUDGET: Duration = Duration::from_secs(10);

#[test]
fn a_five_megabyte_page_is_patched_in_linear_time() {
    let mut page = String::from("---\ntitle: old\ndescription: |\n");
    while page.len() < 2_500_000 {
        page.push_str("  a line of a very long literal scalar\n\n");
    }
    page.push_str("status: draft\n---\n");
    while page.len() < 5_000_000 {
        page.push_str("A body line with --- and key: value text.\n");
    }
    let changes: Vec<Change> = vec![
        ("description".to_string(), Some(PatchValue::from("short"))),
        ("status".to_string(), Some(PatchValue::from("stable"))),
    ];
    let (out, elapsed) = timed(|| patch_frontmatter(&page, &changes).expect("patch"));
    // The blank line that ends the scalar is followed by no indented line,
    // so it is not part of the value and stays.
    assert!(out.starts_with("---\ntitle: old\ndescription: short\n\nstatus: stable\n---\n"));
    assert!(elapsed < BUDGET, "took {elapsed:?}");
    let (_, elapsed) = timed(|| review_frontmatter(&page, "human", "2026-09-19T17:00:00Z"));
    assert!(elapsed < BUDGET, "review took {elapsed:?}");
    let (_, elapsed) = timed(|| parse_frontmatter(&page));
    assert!(elapsed < BUDGET, "read took {elapsed:?}");
}

#[test]
fn a_hundred_thousand_keys_are_patched_in_linear_time() {
    let mut page = String::from("---\n");
    for n in 0..100_000 {
        page.push_str(&format!("key{n}: value {n}\n"));
    }
    page.push_str("---\nBody\n");
    // Replace every fifth key, delete every seventh, add as many again.
    let mut changes: Vec<Change> = Vec::new();
    for n in (0..100_000).step_by(5) {
        changes.push((format!("key{n}"), Some(PatchValue::from("patched"))));
    }
    for n in (1..100_000).step_by(7).filter(|n| n % 5 != 0) {
        changes.push((format!("key{n}"), None));
    }
    for n in 0..20_000 {
        changes.push((
            format!("added{n}"),
            Some(PatchValue::Scalar(Scalar::Int(n))),
        ));
    }
    let (out, elapsed) = timed(|| patch_frontmatter(&page, &changes).expect("patch"));
    assert!(elapsed < BUDGET, "took {elapsed:?}");
    assert!(out.contains("\nkey99995: patched\n"));
    assert!(!out.contains("\nkey8:"));
    assert!(out.ends_with("added19999: 19999\n---\nBody\n"));
}
