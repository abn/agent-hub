#!/usr/bin/env python3
"""The identity release gate: every named check is listed by its own runner and passes.

  identity_gate.py            list, then run, every named check
  identity_gate.py --list     list only: read the runners' own listings and fail
                              when a named check is missing, ignored or skipped

A named check is never skipped, ignored or conditional on a tool being present.
The gate reads what each runner says rather than the source: `cargo test -- --list`
and `--list --ignored`, Playwright's JSON listing (expected status and annotations),
and Vitest's JSON counts. A name that is not listed, a listed name that is ignored
or skipped, or a run in which a file reports an ignored or filtered-out test fails
the gate.

The live checks (real clients on a host that has them) are not part of this
script: see SKILL.md, `harness_check.py run identity-*`.
"""
import json, os, re, subprocess, sys

REPO = subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=os.path.dirname(os.path.abspath(__file__)),
                      capture_output=True, text=True).stdout.strip()
TMP = os.path.join(REPO, "target", "tmp", "identity-gate")

RELEASE = [
    ("no-binary setup", "no_binary_http"),
    ("enrol over MCP", "enrol_via_mcp"),
    ("two agents, one session", "two_agents_one_session"),
    ("child session", "child_session"),
    ("delivery", "reply_after_switch"),
    ("delivery", "answer_before_first_drain"),
    ("delivery", "concurrent_drains_deliver_once"),
    ("delivery", "addressed_comment_without_subscription"),
    ("confidential pairing", "confidential_pairing"),
    ("revoke and reissue", "revoke_reissue"),
    ("revoke and reissue", "replacement_stream_survives"),
    ("proxy hot swap", "proxy_enrol_rename_hot_swap"),
    ("proxy parity", "proxy_parity"),
    ("launcher and archive", "hooks_off_line"),
    ("launcher and archive", "setup_antigravity_installs_an_executable_launcher"),
    ("launcher and archive", "antigravity_archive_holds_an_executable_launcher"),
]

# Test files owned elsewhere that the release rests on. Their other names are theirs; the gate
# names the ones the release cannot do without.
OTHER = [
    ("launcher", "launcher", "the_launcher_installs_locks_and_fails_as_specified"),
    ("confidential pairing", "credential_fixtures", "every_credential_fixture_resolves_as_expected"),
    ("confidential pairing", "credential_fixtures", "two_writers_in_separate_threads_lose_no_entry"),
]

# The migration file is the store's. The gate names the exact tests the release needs, so a
# rename fails here instead of matching by accident.
MIGRATION_FILE = "identity_migration"
MIGRATION_TESTS = [
    ("fault injection at every stage", "every_fail_point_finishes_on_the_next_open_and_then_holds_still"),
    ("fault injection at every stage", "a_restart_at_closed_finishes_a_missing_loser"),
    ("a reply asked before the upgrade is delivered after it", "a_reply_asked_in_a_loser_reaches_its_participant_after_the_upgrade"),
    ("a reply asked before the upgrade is delivered after it", "a_staggered_cursor_delivers_the_merged_answer_once"),
    ("rollback and restore", "backup_restore_and_check_carry_the_merged_brains"),
    ("rollback and restore", "rolling_back_to_the_pre_upgrade_set_brings_back_a_deleted_artifact"),
]

PLAYWRIGHT_FILE = "e2e/release-identity.spec.mjs"
PLAYWRIGHT_PROJECTS = ["desktop", "phone"]
PLAYWRIGHT = [
    ("members add", "adding one shows the token once and keeps it out of the project forms"),
    ("approval rename", "approving a request under another name lands on that name"),
    ("Now counts sessions", "Now counts live sessions and folds a subagent under its parent"),
    ("byline per kind", "a byline names the subject and the receipts for each kind of author"),
    ("legacy label", "a row from before members keeps its stored label and says so"),
    ("credential lock interop", "the Rust and the JavaScript writer share one lock and lose no entry"),
]

VITEST_CONFIG = "plugins/vitest.config.mjs"
VITEST_RELEASE = "plugins/opencode/test/release.test.mjs"
VITEST_CREDENTIAL = "plugins/opencode/test/credential.test.mjs"
VITEST_RELEASE_NAMES = [
    ("child session", "starts as <parent>/<type>-<id8>, exclusively, under a live parent"),
    ("child session", "steps to -2 and on when the hub says the name is taken, and never reuses a taken one"),
    ("child session", "carries its own session on hub calls and leaves the root's alone"),
    ("child session", "resumes its own name, not a new one, when the host reopens a child it already mapped"),
    ("child session", "leaves its session when it is deleted, and ends nothing"),
]
# The one fixture the confidential-project pairing rests on, and the family it sits in.
PAIRING_FIXTURE = "credential-project-url-and-member-untrusted"

rows = []  # (gate, runner, name, listed, result)
problems = []


def say(*a):
    print(*a, flush=True)


def sh(cmd, env=None, timeout=3600):
    return subprocess.run(cmd, cwd=REPO, capture_output=True, text=True, timeout=timeout,
                          env=dict(os.environ, **(env or {})))


def record(gate, runner, name, listed, result):
    rows.append((gate, runner, name, listed, result))
    if not listed or result not in ("listed", "passed"):
        shown = "not listed" if listed is False and result in ("listed", "passed") else result
        problems.append(f"{runner}: {name}: {shown}")


# ------------------------------------------------------------------ cargo
def cargo_listing(test):
    plain = sh(["cargo", "test", "--test", test, "--", "--list"])
    if plain.returncode != 0:
        return None, None, plain.stderr[-600:]
    names = set(re.findall(r"^(.*): test$", plain.stdout, re.M))
    ignored = sh(["cargo", "test", "--test", test, "--", "--list", "--ignored"])
    return names, set(re.findall(r"^(.*): test$", ignored.stdout, re.M)), ""


def cargo_run(test):
    out = sh(["cargo", "test", "--test", test])
    text = out.stdout + out.stderr
    status = dict(re.findall(r"^test (\S+) \.\.\. (\S+)", text, re.M))
    result = re.findall(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; \d+ measured; (\d+) filtered out", text)
    return status, result[-1] if result else None, text


def check_cargo(test, wanted, run):
    """wanted: [(gate, name)]. Fails on a missing, ignored, filtered or failed test."""
    names, ignored, err = cargo_listing(test)
    if names is None:
        for gate, name in wanted:
            record(gate, f"cargo --test {test}", name, False, "does not build")
        problems.append(f"cargo --test {test} does not build: {err}")
        return
    status, counts = (None, None)
    if run:
        status, counts, _ = cargo_run(test)
        if counts and (counts[2] != "0" or counts[3] != "0"):
            problems.append(f"cargo --test {test}: {counts[2]} ignored, {counts[3]} filtered out")
    for gate, name in wanted:
        listed = name in names
        if name in ignored:
            record(gate, f"cargo --test {test}", name, listed, "ignored")
        elif not run:
            record(gate, f"cargo --test {test}", name, listed, "listed")
        else:
            record(gate, f"cargo --test {test}", name, listed, "passed" if (status or {}).get(name) == "ok" else f"{(status or {}).get(name, 'not run')}")


def check_migration(run):
    names, ignored, err = cargo_listing(MIGRATION_FILE)
    runner = f"cargo --test {MIGRATION_FILE}"
    if names is None:
        record("migration", runner, "(the file)", False, "does not build or is missing")
        return
    status = {}
    if run:
        status, counts, _ = cargo_run(MIGRATION_FILE)
        if counts and (counts[2] != "0" or counts[3] != "0"):
            problems.append(f"{runner}: {counts[2]} ignored, {counts[3]} filtered out")
    if ignored:
        problems.append(f"{runner}: ignored tests {sorted(ignored)}")
    for kind, name in MIGRATION_TESTS:
        label = f"{kind}: {name}"
        if name not in names:
            record("migration", runner, label, False, "not listed")
        elif run:
            record("migration", runner, label, True, "passed" if status.get(name) == "ok" else status.get(name, "not run"))
        else:
            record("migration", runner, label, True, "listed")
    if run:
        failing = [n for n in names if status.get(n) != "ok"]
        if failing:
            problems.append(f"{runner}: not passing: {sorted(failing)[:5]}")


# -------------------------------------------------------------- playwright
def playwright_json(args, env=None):
    out = sh(["npx", "playwright", "test", PLAYWRIGHT_FILE, *[f"--project={p}" for p in PLAYWRIGHT_PROJECTS],
              "--reporter=json", *args], env=env)
    # The harness prints its hub lines before the report: the report starts at the first bare brace.
    start = out.stdout.find("\n{\n")
    text = out.stdout if out.stdout.startswith("{") else out.stdout[start + 1:] if start >= 0 else out.stdout
    try:
        return json.loads(text), out
    except ValueError:
        return None, out


def walk(suite, found, title_path=()):
    for spec in suite.get("specs", []):
        for test in spec.get("tests", []):
            found.append((spec["title"], test))
    for child in suite.get("suites", []):
        walk(child, found)


def check_playwright(run):
    listed, out = playwright_json(["--list"])
    runner = "playwright"
    if listed is None:
        for gate, name in PLAYWRIGHT:
            record(gate, runner, name, False, "cannot list")
        problems.append(f"playwright cannot list {PLAYWRIGHT_FILE}: {(out.stderr or out.stdout)[-400:]}")
        return
    found = []
    for suite in listed.get("suites", []):
        walk(suite, found)
    results = {}
    if run:
        data, out = playwright_json([], env={"CI": ""})
        if data is None:
            problems.append(f"playwright run produced no report: {(out.stderr or out.stdout)[-400:]}")
        else:
            ran = []
            for suite in data.get("suites", []):
                walk(suite, ran)
            if not ran:
                problems.append(f"playwright run reported no tests: {str(data.get('errors'))[:400]}")
            for title, test in ran:
                last = (test.get("results") or [{}])[-1]
                results[(title, test.get("projectName"))] = last.get("status", "not run")
    for gate, name in PLAYWRIGHT:
        for project in PLAYWRIGHT_PROJECTS:
            label = f"[{project}] {name}"
            tests = [t for title, t in found if title == name and t.get("projectName") == project]
            if not tests:
                record(gate, runner, label, False, "not listed")
                continue
            kinds = {a.get("type") for a in tests[0].get("annotations", [])}
            if tests[0].get("expectedStatus") != "passed" or kinds & {"skip", "fixme"}:
                record(gate, runner, label, True, "skipped")
            elif not run:
                record(gate, runner, label, True, "listed")
            else:
                status = results.get((name, project), "not run")
                record(gate, runner, label, True, "passed" if status == "passed" else status)


# ------------------------------------------------------------------ vitest
def check_vitest_listing():
    """List only: `vitest list` names the tests without running them. It names a
    fixture-per-test family once, so the pairing fixture is read from its file."""
    runner = "vitest"
    out = sh(["npx", "vitest", "list", "--config", VITEST_CONFIG, VITEST_RELEASE, VITEST_CREDENTIAL])
    listed = out.stdout.splitlines()
    missing = "missing, or skipped, todo or conditionally skipped (vitest list hides those)"

    def record_listed(gate, label, ok):
        record(gate, runner, label, ok, "listed" if ok else missing)

    for gate, name in VITEST_RELEASE_NAMES:
        record_listed(gate, f"release.test.mjs > {name}", any(line.endswith(f" > {name}") and VITEST_RELEASE in line for line in listed))
    fixture = os.path.join(REPO, "plugins", "contract", "fixtures", f"{PAIRING_FIXTURE}.json")
    family = any(line.endswith("credential fixtures > finds the family") for line in listed)
    record_listed("confidential pairing", f"credential.test.mjs > {PAIRING_FIXTURE}", os.path.exists(fixture) and family)


def check_vitest():
    runner = "vitest"
    os.makedirs(TMP, exist_ok=True)
    out_file = os.path.join(TMP, "vitest.json")
    if os.path.exists(out_file):
        os.remove(out_file)
    out = sh(["npx", "vitest", "run", "--config", VITEST_CONFIG, "--reporter=json", f"--outputFile={out_file}",
              VITEST_RELEASE, VITEST_CREDENTIAL])
    try:
        data = json.load(open(out_file))
    except (OSError, ValueError):
        for gate, name in VITEST_RELEASE_NAMES:
            record(gate, runner, name, False, "no report")
        problems.append(f"vitest produced no report: {(out.stderr or out.stdout)[-400:]}")
        return
    by_file = {}
    for file in data.get("testResults", []):
        by_file[os.path.relpath(file["name"], REPO)] = {a["title"]: a["status"] for a in file.get("assertionResults", [])}
    for f, tests in by_file.items():
        for t, s in tests.items():
            if s in ("skipped", "pending", "todo"):
                problems.append(f"vitest: {f}: '{t}' is {s}")
    wanted = [(g, VITEST_RELEASE, n) for g, n in VITEST_RELEASE_NAMES]
    wanted.append(("confidential pairing", VITEST_CREDENTIAL, PAIRING_FIXTURE))
    for gate, file, name in wanted:
        status = by_file.get(file, {}).get(name)
        label = f"{os.path.basename(file)} > {name}"
        if status is None:
            record(gate, runner, label, False, "not listed")
        else:
            record(gate, runner, label, True, "passed" if status == "passed" else status)


def main(argv):
    run = "--list" not in argv
    check_cargo("release_identity", RELEASE, run)
    grouped = {}
    for gate, test, name in OTHER:
        grouped.setdefault(test, []).append((gate, name))
    for test, wanted in grouped.items():
        check_cargo(test, wanted, run)
    check_migration(run)
    check_playwright(run)
    if run:
        check_vitest()
    else:
        check_vitest_listing()

    width = max(len(r[2]) for r in rows) if rows else 0
    for gate, runner, name, listed, result in rows:
        mark = "ok  " if listed and result in ("listed", "passed") else "FAIL"
        say(f"{mark} {gate:<24} {runner:<34} {name[:90]:<{min(width, 90)}} {result}")
    say()
    if problems:
        say(f"identity gate: {len(problems)} problem(s)")
        for p in dict.fromkeys(problems):
            say(f"  - {p}")
        return 1
    say(f"identity gate: {len(rows)} named checks, {'listed' if not run else 'listed and passing'}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
