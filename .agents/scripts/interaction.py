#!/usr/bin/env python3
"""Interaction checks for the PWA: the list filter and the single-key verbs.

These test behaviour, not appearance: a query typed into a list's filter field
has to leave the rows it matched on screen under the group that holds them, `a`
and `r` have to reach the verbs a row keeps beside it rather than only the ones it
carries inside it, `/` has to reach a field the screen is showing or fall through
to Search instead of focusing one that is not there, and a filter chip's number
has to count the rows it reveals. Nothing here reads geometry, so a redesign that
keeps the behaviour passes.

The filter is checked on the inbox and on a project feed together because the two
draw their rows differently: the inbox wraps a group's rows in a block inside the
group, while the feed sets them beside the day heading. A walk that knows only
one of the two hides a group that still holds rows, which is how typing into the
inbox filter emptied the list while the count line above it still said what had
matched.
"""

from __future__ import annotations

import json
import sys
import time
from contextlib import contextmanager
from urllib.parse import quote

import hub_harness as harness

NAME = "web-interaction"

try:
    from playwright.sync_api import sync_playwright
except ImportError:
    harness.skip(NAME, "playwright is not installed")

DESKTOP = {"width": 1440, "height": 900}
PHONE = {"width": 390, "height": 780}

# The group headings a filter judges, which is the filter's own list rather than a
# second one here, so a screen that draws another shape is walked by both.
HEADINGS = ".day, .hub-group-header, .inbox-group"

# The rows the keyboard map moves through, as the inbox registers them.
KEY_ROWS = ".inbox-row:not(details:not([open]) .inbox-row)"

# What the index holds, asked of the rendered page: every row, which of them are
# showing, and every group heading with the rows it heads. A heading's rows are
# read the way the filter reads them, from the heading up to the next one, so the
# two shapes (rows inside the group, rows beside the heading) are both described
# by one walk.
SNAPSHOT = """() => {
  const body = document.querySelector('.shell-index .shell-body');
  const text = (el) => (el.textContent || '').replace(/\\s+/g, ' ').trim();
  const rows = [...body.querySelectorAll('.row')];
  const count = body.querySelector('.shell-count');
  const headed = (head) => {
    const held = [];
    for (let node = head; node; node = node.nextElementSibling) {
      if (node !== head && node.matches(HEADINGS)) break;
      held.push(...node.querySelectorAll('.row'));
    }
    return held.map(text);
  };
  return {
    rows: rows.map(text),
    shown: rows.filter((row) => !row.hidden).map(text),
    groups: [...body.querySelectorAll(HEADINGS)].map((head) => ({
      key: head.dataset.group || text(head).slice(0, 24),
      hidden: head.hidden,
      rows: headed(head),
    })),
    labels: [...body.querySelectorAll('.inbox-label')].map((label) => ({
      key: label.dataset.group || text(label).slice(0, 24),
      hidden: label.hidden,
    })),
    count: count && !count.hidden ? text(count) : '',
  };
}"""

SNAPSHOT = SNAPSHOT.replace("HEADINGS", repr(HEADINGS))

# Where the keyboard ended up, asked as a reader would see it.
FOCUS = """() => {
  const active = document.activeElement;
  const filter = document.querySelector('[data-index-filter]');
  return {
    hash: location.hash,
    tag: active ? active.tagName.toLowerCase() : '',
    filter: active === filter,
    q: active === document.getElementById('q'),
    filterBoxes: filter ? filter.getClientRects().length : -1,
  };
}"""

# A rendered document is opaque-origin on purpose, so a frame of one that reads
# local storage throws in the frame and not in the app. That is the design's own
# rule for the viewer (see the prefix check), and it is not what these checks are
# about, so it is the one error left unreported.
NOT_THE_SUBJECT = "sandboxed"


def settle(page, expression: str, timeout: int = 8000) -> bool:
    """Wait on a condition in the page.

    The shell's content security policy has no 'unsafe-eval', which is what
    `page.wait_for_function` injects, so the polling is from here, as in the other
    browser checks.
    """
    deadline = time.monotonic() + timeout / 1000
    while True:
        if page.evaluate(f"!!({expression})"):
            return True
        if time.monotonic() >= deadline:
            return False
        page.wait_for_timeout(100)


def goto(page, route: str, segment: str, failures: list[str], where: str) -> bool:
    """Move to a route and wait for the screen it paints.

    A shell names its own section in `data-segment`, which is the screen's identity
    rather than whatever it happened to open, so that is what is asked for instead
    of a heading: a project screen's `h1` is the selected event or artifact.
    """
    page.evaluate(f"location.hash = {json.dumps(route)}")
    if not settle(page, "!!document.querySelector('main h1')"):
        failures.append(f"{where}: {route} painted nothing")
        return False
    if not segment:
        # The one screen with no index and no segment: Home.
        if not settle(page, "!!document.querySelector('.shell-no-index .home-pad')"):
            failures.append(f"{where}: {route} painted no Home")
            return False
    elif not settle(page, f"!!document.querySelector('.shell[data-segment={json.dumps(segment)}] .shell-index .shell-body')"):
        failures.append(f"{where}: {route} painted no {segment} index")
        return False
    page.wait_for_timeout(350)
    return True


@contextmanager
def watching(page, phase: str, failures: list[str]):
    """Report what the browser reported while one check ran."""
    errors: list[str] = []

    def on_error(error) -> None:
        if NOT_THE_SUBJECT in str(error):
            return
        errors.append(f"{phase}: uncaught error: {error}")

    page.on("pageerror", on_error)
    try:
        yield
    finally:
        page.remove_listener("pageerror", on_error)
        failures.extend(errors)


def expected(term: str, before: dict) -> dict:
    """What one query owes the reader, read off the list as it was before it.

    A group stays while any row it heads matches, and a heading that names such a
    group stays with it.
    """
    needle = term.lower()
    return {
        "shown": [text for text in before["rows"] if needle in text.lower()],
        "holding": {
            group["key"]: any(needle in text.lower() for text in group["rows"])
            for group in before["groups"]
        },
    }


def report_filter(before: dict, after: dict, term: str, where: str, failures: list[str]) -> None:
    """The list after one query, against what that query owes it."""
    want = expected(term, before)
    if not want["shown"]:
        failures.append(f"{where}: nothing on the list matches {term!r}, so this check is not covering it")
        return
    if len(after["shown"]) != len(want["shown"]):
        failures.append(
            f"{where}: filtering {term!r} left {len(after['shown'])} rows showing, not the "
            f"{len(want['shown'])} that match: {after['shown']}"
        )
    for group in after["groups"]:
        holds = want["holding"].get(group["key"])
        if holds is None or group["hidden"] == holds:
            failures.append(
                f"{where}: the {group['key']} group is "
                f"{'hidden' if group['hidden'] else 'standing over nothing'} while it "
                f"{'holds a matching row' if holds else 'holds none'} ({term!r})"
            )
    for label in after["labels"]:
        holds = want["holding"].get(label["key"])
        if holds is not None and label["hidden"] == holds:
            failures.append(
                f"{where}: the {label['key']} heading is "
                f"{'hidden' if label['hidden'] else 'over an empty group'} while its group "
                f"{'holds a matching row' if holds else 'holds none'} ({term!r})"
            )
    # The count line is the only statement of what the query did, so it has to
    # count the rows the list kept.
    if not after["count"].startswith(f"{len(want['shown'])} of {len(before['rows'])} match "):
        failures.append(f"{where}: the count line reads {after['count']!r} after filtering {term!r}")


def check_filter(page, failures: list[str], where: str, route: str, segment: str, terms: list[str]) -> None:
    """Typing a query leaves the matching rows under the groups that hold them."""
    with watching(page, where, failures):
        if not goto(page, route, segment, failures, where):
            return
        field = ".shell-index [data-index-filter]"
        for term in terms:
            before = page.evaluate(SNAPSHOT)
            page.fill(field, term)
            page.wait_for_timeout(250)
            report_filter(before, page.evaluate(SNAPSHOT), term, where, failures)
            page.fill(field, "")
            page.wait_for_timeout(150)
        # A query nothing matches leaves no group standing over an empty list.
        before = page.evaluate(SNAPSHOT)
        page.fill(field, harness.SEARCH_MISS_TERM)
        page.wait_for_timeout(250)
        after = page.evaluate(SNAPSHOT)
        if after["shown"]:
            failures.append(f"{where}: {harness.SEARCH_MISS_TERM!r} left {after['shown']} showing")
        standing = [group["key"] for group in after["groups"] if not group["hidden"]]
        if standing:
            failures.append(f"{where}: {standing} stayed visible with no matching row under them")
        page.fill(field, "")
        page.wait_for_timeout(150)


def select_row(page, needle: str) -> bool:
    """Walk the selection onto the row carrying this text, as a reader would."""
    # The map is deliberately deaf while the keyboard is in a field, so a check
    # that came from the filter field hands the keyboard back to the page first,
    # which is what leaving the field does.
    page.evaluate("() => document.activeElement && document.activeElement.blur()")
    at = page.evaluate(
        "(args) => { const list = [...document.querySelectorAll(args.rows)];"
        " return { want: list.findIndex((row) => row.textContent.includes(args.needle)),"
        "  have: list.findIndex((row) => row.tabIndex === 0) }; }",
        {"rows": KEY_ROWS, "needle": needle},
    )
    if at["want"] < 0:
        return False
    steps = at["want"] - (at["have"] if at["have"] >= 0 else 0)
    key = "j" if steps >= 0 else "k"
    for _ in range(abs(steps)):
        page.keyboard.press(key)
    page.wait_for_timeout(200)
    return True


def check_row_verbs(page, failures: list[str]) -> None:
    """`a` and `r` reach the verbs the inbox keeps beside the row, not only on it.

    An inbox row carries no Approve or Reply of its own: the tray a swipe uncovers
    is a sibling of the row, so a key that looked only inside the row found
    nothing and did nothing at all, on the one screen whose whole purpose is
    deciding and answering.
    """
    where = "inbox: a and r reach the selected row's verbs"
    with watching(page, where, failures):
        if not goto(page, "#/inbox", "inbox", failures, where):
            return
        if not select_row(page, harness.APPROVAL_SUMMARY):
            failures.append(f"{where}: no row carries {harness.APPROVAL_SUMMARY!r}")
            return
        chosen = page.evaluate("() => document.activeElement.textContent.trim().slice(0, 80)")
        if harness.APPROVAL_SUMMARY not in chosen:
            failures.append(f"{where}: the selection is {chosen!r}, not the approval row")
            return
        waiting = page.evaluate(
            "() => document.querySelectorAll('.inbox-group[data-group=\"waiting\"] .inbox-row').length"
        )
        page.keyboard.press("a")
        if not settle(page, "!!document.querySelector('dialog[open]')", timeout=4000):
            failures.append(f"{where}: a on an approval row opened no confirmation")
            return
        title = page.inner_text("dialog[open] .dialog-title")
        if harness.APPROVAL_SUMMARY not in title:
            failures.append(f"{where}: a on the approval row asked about {title!r}")
        page.click("dialog[open] .dialog-commit")
        left = settle(
            page,
            f'document.querySelectorAll(\'.inbox-group[data-group="waiting"] .inbox-row\').length === {waiting - 1}',
        )
        if not left:
            now = page.evaluate(
                "() => document.querySelectorAll('.inbox-group[data-group=\"waiting\"] .inbox-row').length"
            )
            failures.append(f"{where}: the waiting group holds {now} rows after a key approved one of {waiting}")
        if page.evaluate("!!document.querySelector('dialog[open]')"):
            failures.append(f"{where}: the confirmation stayed open after the decision")

        if not select_row(page, harness.QUESTION_SUBJECT):
            failures.append(f"{where}: no row carries {harness.QUESTION_SUBJECT!r}")
            return
        page.keyboard.press("r")
        if not settle(page, "!!document.querySelector('.inbox-rows .composer .composer-field')", timeout=4000):
            failures.append(f"{where}: r on a question row opened no reply card")
            return
        reply = page.evaluate(
            """(needle) => {
              const field = document.querySelector('.inbox-rows .composer .composer-field');
              const item = field.closest('.composer').previousElementSibling;
              return {
                under: !!(item && item.classList.contains('inbox-item') && item.textContent.includes(needle)),
                held: field === document.activeElement || field.contains(document.activeElement),
              };
            }""",
            harness.QUESTION_SUBJECT,
        )
        if not reply["under"]:
            failures.append(f"{where}: the reply card is not under the row it answers")
        if not reply["held"]:
            failures.append(f"{where}: the reply card opened with the keyboard nowhere in it")


def check_slash(browser, failures: list[str], port: int, project: str) -> None:
    """`/` focuses the field the screen shows and falls through to Search when it shows none.

    A phone keeps a project's filter in the DOM and out of the layout until its
    own control opens it. Focusing a field with no box is accepted by the DOM and
    does nothing, so the key was swallowed on four of the five project segments.
    """
    where = "phone: / reaches a field the screen shows, or Search"
    with browser.new_context(viewport=PHONE) as context:
        context.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
        page = context.new_page()
        page.goto(f"http://127.0.0.1:{port}/#/inbox", wait_until="load")
        with watching(page, where, failures):
            # The case that already worked: a screen showing its filter takes the key.
            if not goto(page, "#/inbox", "inbox", failures, where):
                context.close()
                return
            page.keyboard.press("/")
            page.wait_for_timeout(250)
            focus = page.evaluate(FOCUS)
            if not focus["filter"] or not focus["hash"].startswith("#/inbox"):
                failures.append(
                    f"{where}: / on the inbox left the keyboard on {focus['tag']!r} at {focus['hash']!r} "
                    "rather than in the filter the inbox is showing"
                )
            page.keyboard.type("release")
            page.wait_for_timeout(200)
            typed = page.evaluate("() => document.querySelector('[data-index-filter]').value")
            if typed != "release":
                failures.append(f"{where}: text typed after / did not land in the inbox filter ({typed!r})")
            page.fill("[data-index-filter]", "")

            for segment in ("feed", "wiki", "artifacts", "sessions"):
                screen = f"phone project {segment}: / reaches a field the screen shows, or Search"
                if not goto(page, f"#/projects/{quote(project)}/{segment}", segment, failures, screen):
                    continue
                before = page.evaluate(FOCUS)
                page.keyboard.press("/")
                if not settle(page, "location.hash.startsWith('#/search')", timeout=4000):
                    after = page.evaluate(FOCUS)
                    failures.append(
                        f"{screen}: / left the address at {after['hash']!r} with the keyboard on "
                        f"{after['tag']!r}, having started at {before['hash']!r} where the filter "
                        f"field was drawing {before['filterBoxes']} boxes"
                    )
                    continue
                if not settle(page, "document.activeElement === document.getElementById('q')", timeout=4000):
                    failures.append(f"{screen}: Search opened but the keyboard did not land in its field")
                    continue
                page.keyboard.type(harness.SEARCH_TERM)
                page.wait_for_timeout(400)
                landed = page.evaluate(
                    "() => ({ q: (document.getElementById('q') || {}).value, hash: location.hash })"
                )
                if landed["q"] != harness.SEARCH_TERM:
                    failures.append(f"{screen}: what was typed after / landed in {landed['q']!r}")
                if not landed["hash"].startswith(f"#/search?q={harness.SEARCH_TERM}"):
                    failures.append(f"{screen}: the query never reached the address ({landed['hash']!r})")
        context.close()


def check_home_chip(browser, failures: list[str], port: int) -> None:
    """A filter chip's number counts the rows it reveals.

    The Unread chip keeps the newest rows carrying the unread dot, which is what
    sits above a project's cursor. Its number was the hub's own unread queue, so
    it read "Unread 3" over seven rows. Run before anything reads a project feed,
    because the hub's cursor moves as soon as the feed has been seen and the dots
    go with it.
    """
    where = "home: the unread chip counts the rows it reveals"
    with browser.new_context(viewport=PHONE) as context:
        context.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
        page = context.new_page()
        page.goto(f"http://127.0.0.1:{port}/#/home", wait_until="load")
        with watching(page, where, failures):
            if not goto(page, "#/home", "", failures, where):
                context.close()
                return
            chip = " ".join(page.inner_text(".home-chips-flow [data-home-chip='unread']").split())
            rows = page.evaluate("() => document.querySelectorAll('.home-row.unread').length")
            number = "".join(character for character in chip if character.isdigit())
            if not rows:
                failures.append(f"{where}: Home drew no unread rows, so the chip filters nothing to count")
            elif not number:
                failures.append(f"{where}: the Unread chip carries no number ({chip!r})")
            elif int(number) != rows:
                failures.append(f"{where}: the chip reads {chip!r} over {rows} rows")
            page.click(".home-chips-flow [data-home-chip='unread']")
            page.wait_for_timeout(250)
            revealed = page.evaluate(
                "() => [...document.querySelectorAll('.home-row')].filter((row) => !row.hidden).length"
            )
            if revealed != rows:
                failures.append(f"{where}: pressing the chip revealed {revealed} rows, not the {rows} it counts")
        context.close()


def run() -> int:
    failures: list[str] = []
    with harness.running_hub(NAME) as (port, seeded):
        project = seeded["project_id"]
        # A term in the waiting group and one in the unread group: a filter that
        # judges a group by what follows it gets one of the two wrong.
        inbox_terms = [harness.APPROVAL_SUMMARY.split()[0], harness.INBOX_READ_SUMMARY.split()[1]]
        with sync_playwright() as playwright:
            browser = harness.launch_browser(playwright, NAME)
            # First, because the hub's read cursor moves as soon as a project's
            # feed has been read: Home draws its unread dots from what is still
            # above that cursor, so it is asked before anything reads a feed.
            check_home_chip(browser, failures, port)
            context = browser.new_context(viewport=DESKTOP, color_scheme="light")
            context.add_init_script(f"localStorage.setItem('hub.token', {json.dumps(harness.ADMIN_TOKEN)});")
            page = context.new_page()
            page.goto(f"http://127.0.0.1:{port}/#/inbox", wait_until="load")
            check_filter(page, failures, "inbox at 1440", "#/inbox", "inbox", inbox_terms)
            page.set_viewport_size(PHONE)
            check_filter(page, failures, "inbox at 390", "#/inbox", "inbox", inbox_terms)
            page.set_viewport_size(DESKTOP)
            check_filter(
                page,
                failures,
                "project feed at 1440",
                f"#/projects/{quote(project)}/feed",
                "feed",
                [harness.FINISHED_SUMMARY.split()[1]],
            )
            check_row_verbs(page, failures)
            check_slash(browser, failures, port, project)
            context.close()
            browser.close()
    if failures:
        for failure in dict.fromkeys(failures):
            print(f"{NAME}: {failure}", file=sys.stderr)
        return 1
    print(f"{NAME}: the list filter and the single-key verbs reach what they name")
    return 0


if __name__ == "__main__":
    sys.exit(run())
