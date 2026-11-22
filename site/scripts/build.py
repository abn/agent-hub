#!/usr/bin/env python3
"""Assemble the static site for Cloudflare.

    build.py assemble --site-url URL --fonts DIR --render DIR <site> <dist>
        Write the landing and not-found pages with the site origin filled
        in, copy the assets and the app's fonts, add the rendered docs under
        docs/, publish llms.txt, the bootstrap guide and the skill, and write _headers with the hashes of the docs' inline scripts.
"""

import argparse
import base64
import hashlib
import pathlib
import re
import shutil
import sys

PAGES = ("index.html", "404.html")
STATIC = ("favicon.svg", "favicon-32.png", "apple-touch-icon.png", "robots.txt", "llms.txt")
SKILL = pathlib.Path("skills") / "agent-hub"
BASE_URL = "{{base_url}}"
INLINE_SCRIPT = re.compile(r"<script(?![^>]*(?:^|\s)src=)([^>]*)>(.*?)</script>", re.S)
PLACEHOLDER = re.compile(r"@[A-Z_]+@")


def script_hashes(docs: pathlib.Path) -> list[str]:
    """The CSP hashes of every distinct inline script the docs renderer wrote."""
    found = set()
    for page in docs.rglob("*.html"):
        # Read as bytes decoded without newline translation, so the hash is of
        # exactly what the browser receives.
        for attrs, body in INLINE_SCRIPT.findall(page.read_bytes().decode("utf-8")):
            if "application/json" in attrs or "application/ld+json" in attrs or not body.strip():
                continue
            digest = base64.b64encode(hashlib.sha256(body.encode("utf-8")).digest()).decode()
            found.add(f"'sha256-{digest}'")
    return sorted(found)


def provision_skill(root: pathlib.Path, dist: pathlib.Path) -> None:
    """Publish the bootstrap guide and the skill, with the hub address left as <hub>.

    A hub serves its own bootstrap with its address filled in; the public copy
    cannot know it, so it names the placeholder an agent replaces.
    """
    source = root / SKILL
    if not (source / "bootstrap.md").is_file():
        sys.exit(f"build.py: no skill at {source}")
    bootstrap = (source / "bootstrap.md").read_text(encoding="utf-8").replace(BASE_URL, "<hub>")
    (dist / "bootstrap").mkdir()
    (dist / "bootstrap" / "SKILL.md").write_text(bootstrap, encoding="utf-8")
    target = dist / "skill" / "agent-hub"
    shutil.copytree(source, target)
    copy = target / "bootstrap.md"
    copy.write_text(copy.read_text(encoding="utf-8").replace(BASE_URL, "<hub>"), encoding="utf-8")


def assemble(args: argparse.Namespace) -> None:
    site = pathlib.Path(args.site)
    dist = pathlib.Path(args.dist)
    url = args.site_url.rstrip("/")
    if not re.fullmatch(r"https://[a-z0-9.-]+(:[0-9]+)?", url):
        sys.exit(f"build.py: SITE_URL must be an https origin with no path, got {url!r}")

    shutil.rmtree(dist, ignore_errors=True)
    dist.mkdir(parents=True)
    shutil.copytree(site / "assets", dist / "assets")
    fonts = pathlib.Path(args.fonts)
    shutil.copytree(fonts, dist / "assets" / "fonts", ignore=shutil.ignore_patterns("*.md"))

    for name in PAGES:
        text = (site / name).read_text(encoding="utf-8")
        if INLINE_SCRIPT.search(text) or re.search(r"<style|\sstyle=", text):
            sys.exit(f"build.py: {name} has an inline script or style; the policy forbids it")
        text = text.replace("@SITE_URL@", url)
        left = PLACEHOLDER.findall(text)
        if left:
            sys.exit(f"build.py: {name} has unfilled placeholders: {', '.join(sorted(set(left)))}")
        (dist / name).write_text(text, encoding="utf-8")
    for name in STATIC:
        shutil.copy2(site / name, dist / name)

    provision_skill(site.resolve().parent, dist)

    rendered = pathlib.Path(args.render) / "docs"
    if not (rendered / "index.html").is_file():
        sys.exit(f"build.py: no rendered docs at {rendered}")
    shutil.copytree(rendered, dist / "docs")

    hashes = script_hashes(dist / "docs")
    headers = (site / "_headers").read_text(encoding="utf-8")
    headers = headers.replace("@DOCS_SCRIPT_HASHES@", " ".join(hashes))
    (dist / "_headers").write_text(headers, encoding="utf-8")

    files = sum(1 for p in dist.rglob("*") if p.is_file())
    print(f"build.py: {dist} ready, {files} files, {len(hashes)} docs script hashes, "
          f"site {url}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    a = sub.add_parser("assemble")
    a.add_argument("--site-url", required=True)
    a.add_argument("--fonts", required=True)
    a.add_argument("--render", required=True)
    a.add_argument("site")
    a.add_argument("dist")
    a.set_defaults(run=assemble)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
