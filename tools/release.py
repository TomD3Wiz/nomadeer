#!/usr/bin/env python3
"""Release bookkeeping for Nomadeer: version bumps, release notes and checks.

    release.py bump VERSION [--date YYYY-MM-DD]
        Moves CHANGELOG.md's "## [Unreleased]" notes into a dated
        "## [VERSION]" section (leaving an empty Unreleased section above it),
        sets the version in Cargo.toml and Cargo.lock and adds a <release> to
        the top of the AppStream metainfo. Refuses a version that is not
        semver, not newer than the current one, already present anywhere, or
        that has no Unreleased notes to release.

    release.py notes [VERSION] [--downloads]
        Prints VERSION's changelog section (default: Cargo.toml's version).
        --downloads appends the table of which file to get for which platform.

    release.py check [VERSION] [--tag vX.Y.Z]
        Fails unless Cargo.toml, Cargo.lock, the metainfo and CHANGELOG.md all
        carry VERSION (default: Cargo.toml's version) with matching dates, and,
        given --tag, the tag is "v" + Cargo.toml's version.

    release.py assets [VERSION] [--check DIR]
        Prints the file names a release of VERSION publishes, which the in-app
        updater relies on. --check DIR fails unless DIR holds exactly those.

    release.py version
        Prints Cargo.toml's version.

Only the standard library is used, so it runs on a bare CI runner.
"""

from __future__ import annotations

import argparse
import datetime
import os
import re
import subprocess
import sys
from pathlib import Path
from xml.sax.saxutils import escape

ROOT = Path(__file__).resolve().parent.parent
CARGO_TOML = ROOT / "Cargo.toml"
CARGO_LOCK = ROOT / "Cargo.lock"
CHANGELOG = ROOT / "CHANGELOG.md"
METAINFO = ROOT / "packaging/linux/io.github.TomD3Wiz.Nomadeer.metainfo.xml"
REPO_URL = "https://github.com/TomD3Wiz/nomadeer"

# https://semver.org/#is-there-a-suggested-regular-expression-regex-to-check-a-semver-string
SEMVER = re.compile(
    r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?"
    r"(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?"
)
DATE = re.compile(r"\d{4}-\d{2}-\d{2}")
SECTION = re.compile(r"^## \[([^\]]+)\](?: - (\S+))?[ \t]*$", re.M)
LINK_REF = re.compile(r"^\[[^\]]+\]: \S+[ \t]*$", re.M)
CARGO_VERSION = re.compile(r'^(\[package\]\n(?:(?!\[)[^\n]*\n)*?version = ")([^"]+)(")', re.M)
LOCK_VERSION = re.compile(r'^(\[\[package\]\]\nname = "nomadeer"\nversion = ")([^"]+)(")', re.M)
METAINFO_RELEASES = re.compile(r"^([ \t]*)<releases>[ \t]*\n", re.M)
METAINFO_RELEASE = re.compile(r'<release\s+version="([^"]+)"(?:\s+date="([^"]*)")?')


class ReleaseError(Exception):
    pass


def release_assets(version: str) -> list[str]:
    """Every file a release publishes. The in-app updater finds its download
    by these names, so change them only together with the updater."""
    names = [
        f"Nomadeer-{version}-Setup.exe",
        f"Nomadeer-{version}-Setup.exe.sha256",
        f"Nomadeer-{version}-Windows-Portable.zip",
        f"Nomadeer-{version}-Windows-Portable.zip.sha256",
        f"Nomadeer-{version}-x86_64.AppImage",
        f"Nomadeer-{version}-x86_64.AppImage.sha256",
        f"Nomadeer-{version}-x86_64.AppImage.zsync",
        f"Nomadeer-{version}-x86_64.flatpak",
        f"Nomadeer-{version}-x86_64.flatpak.sha256",
    ]
    for variant in ("optimized", "compatible"):
        tarball = f"nomadeer-{version}-linux-x86_64-{variant}.tar.gz"
        names += [tarball, f"{tarball}.sha256"]
    return names


def downloads_table(version: str) -> str:
    return f"""## Downloads

| Platform | File |
|---|---|
| Windows 10 or later | `Nomadeer-{version}-Setup.exe`: installs for your user without admin rights and picks the right edition for your processor |
| Windows, no install | `Nomadeer-{version}-Windows-Portable.zip`: unzip and run `Nomadeer.exe`, or `Nomadeer-compatible.exe` on processors without AVX2 |
| Linux | `Nomadeer-{version}-x86_64.AppImage`: `chmod +x` and run |
| Linux, sandboxed | `Nomadeer-{version}-x86_64.flatpak`: `flatpak install --user Nomadeer-{version}-x86_64.flatpak` |
| Linux, plain binaries | `nomadeer-{version}-linux-x86_64-optimized.tar.gz`, or `-compatible.tar.gz` for processors without AVX2 |

Every file has a matching `.sha256`. Windows may warn about an unrecognised app the first time, as Nomadeer is not code-signed yet.
"""


# Versions -------------------------------------------------------------------


def parse_version(text: str) -> tuple:
    match = SEMVER.fullmatch(text)
    if not match:
        raise ReleaseError(f"{text!r} is not a semantic version such as 1.2.3")
    major, minor, patch, pre, _build = match.groups()
    return int(major), int(minor), int(patch), pre


def precedes(old: str, new: str) -> bool:
    """True when semver ranks `new` above `old` (build metadata ignored)."""
    a, b = parse_version(old), parse_version(new)
    if a[:3] != b[:3]:
        return a[:3] < b[:3]
    if a[3] == b[3]:
        return False
    if a[3] is None or b[3] is None:
        return b[3] is None  # a release outranks its pre-releases

    def key(identifier: str) -> tuple:
        return (0, int(identifier), "") if identifier.isdigit() else (1, 0, identifier)

    return [key(i) for i in a[3].split(".")] < [key(i) for i in b[3].split(".")]


def cargo_version(text: str | None = None) -> str:
    match = CARGO_VERSION.search(text if text is not None else CARGO_TOML.read_text())
    if not match:
        raise ReleaseError("Cargo.toml has no [package] version")
    return match.group(2)


def lock_version(text: str) -> str:
    match = LOCK_VERSION.search(text)
    if not match:
        raise ReleaseError("Cargo.lock has no nomadeer package")
    return match.group(2)


# CHANGELOG.md ---------------------------------------------------------------


def changelog_sections(text: str) -> list[tuple[str, str | None, int, int]]:
    """(name, date, start of heading, end of section) for each "## [..]"."""
    headings = list(SECTION.finditer(text))
    sections = []
    for index, heading in enumerate(headings):
        end = headings[index + 1].start() if index + 1 < len(headings) else len(text)
        link_refs = LINK_REF.search(text, heading.end(), end)
        if link_refs:
            end = link_refs.start()
        sections.append((heading.group(1), heading.group(2), heading.start(), end))
    return sections


def changelog_section(text: str, version: str) -> tuple[str | None, str]:
    for name, date, start, end in changelog_sections(text):
        if name == version:
            body = text[start:end].split("\n", 1)[1] if "\n" in text[start:end] else ""
            return date, body.strip()
    raise ReleaseError(f"CHANGELOG.md has no '## [{version}]' section")


def bump_changelog(text: str, old: str, new: str, date: str) -> str:
    sections = changelog_sections(text)
    if any(name == new for name, *_ in sections):
        raise ReleaseError(f"CHANGELOG.md already has a section for {new}")
    unreleased = next((s for s in sections if s[0] == "Unreleased"), None)
    if unreleased is None:
        raise ReleaseError("CHANGELOG.md has no '## [Unreleased]' section to release")
    _, _, start, end = unreleased
    heading, _, body = text[start:end].partition("\n")
    notes = body.strip()
    if not notes:
        raise ReleaseError(
            "CHANGELOG.md's Unreleased section is empty; write what changed for players first"
        )
    released = f"## [Unreleased]\n\n## [{new}] - {date}\n\n{notes}\n\n"
    text = text[:start] + released + text[end:]

    # Keep a Changelog's comparison links at the bottom of the file.
    unreleased_link = re.compile(r"^\[Unreleased\]: \S+[ \t]*$", re.M)
    links = (
        f"[Unreleased]: {REPO_URL}/compare/v{new}...HEAD\n"
        f"[{new}]: {REPO_URL}/compare/v{old}...v{new}"
    )
    if unreleased_link.search(text):
        text = unreleased_link.sub(lambda _: links, text, count=1)
    else:
        text = text.rstrip("\n") + "\n\n" + links + "\n"
    return text


# Metainfo -------------------------------------------------------------------


def plain_markdown(text: str) -> str:
    """Markdown inline formatting reduced to the plain text AppStream shows."""
    text = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"(\*\*|__|`)", "", text)
    text = re.sub(r"(?<!\w)[*_]([^*_]+)[*_](?!\w)", r"\1", text)
    return " ".join(text.split())


def appstream_description(notes: str, indent: str) -> str:
    """A changelog section as AppStream markup: paragraphs, "### Group"
    headings as paragraphs and bullets (nested ones flattened) as lists."""
    blocks: list[tuple[str, list[str]]] = []  # ("p", [text]) or ("ul", items)
    paragraph: list[str] = []

    def flush() -> None:
        if paragraph:
            blocks.append(("p", [plain_markdown(" ".join(paragraph))]))
            paragraph.clear()

    for line in notes.splitlines():
        stripped = line.strip()
        bullet = re.match(r"[-*+] (.*)", stripped)
        if not stripped:
            flush()
        elif stripped.startswith("#"):
            flush()
            blocks.append(("p", [plain_markdown(stripped.lstrip("#"))]))
        elif bullet:
            flush()
            if blocks and blocks[-1][0] == "ul":
                blocks[-1][1].append(bullet.group(1))
            else:
                blocks.append(("ul", [bullet.group(1)]))
        elif line[:1].isspace() and blocks and blocks[-1][0] == "ul" and not paragraph:
            blocks[-1][1][-1] += " " + stripped  # wrapped bullet
        else:
            paragraph.append(stripped)
    flush()
    if not blocks:
        blocks = [("p", ["Bug fixes and improvements."])]

    lines = [f"{indent}<description>"]
    for kind, items in blocks:
        if kind == "p":
            lines.append(f"{indent}  <p>{escape(items[0])}</p>")
        else:
            lines.append(f"{indent}  <ul>")
            lines += [f"{indent}    <li>{escape(plain_markdown(item))}</li>" for item in items]
            lines.append(f"{indent}  </ul>")
    lines.append(f"{indent}</description>")
    return "\n".join(lines)


def metainfo_releases(text: str) -> list[tuple[str, str | None]]:
    return [(m.group(1), m.group(2)) for m in METAINFO_RELEASE.finditer(text)]


def bump_metainfo(text: str, version: str, date: str, notes: str) -> str:
    if any(v == version for v, _ in metainfo_releases(text)):
        raise ReleaseError(f"the metainfo already has a <release> for {version}")
    match = METAINFO_RELEASES.search(text)
    if not match:
        raise ReleaseError("the metainfo has no <releases> element")
    indent = match.group(1) + "  "
    release = (
        f'{indent}<release version="{version}" date="{date}">\n'
        f'{indent}  <url type="details">{REPO_URL}/releases/tag/v{version}</url>\n'
        f"{appstream_description(notes, indent + '  ')}\n"
        f"{indent}</release>\n"
    )
    return text[: match.end()] + release + text[match.end() :]


# Commands -------------------------------------------------------------------


def bump(version: str, date: str) -> None:
    parse_version(version)
    if not DATE.fullmatch(date):
        raise ReleaseError(f"{date!r} is not a YYYY-MM-DD date")
    cargo_toml, cargo_lock = CARGO_TOML.read_text(), CARGO_LOCK.read_text()
    changelog, metainfo = CHANGELOG.read_text(), METAINFO.read_text()
    old = cargo_version(cargo_toml)
    if version == old:
        raise ReleaseError(f"Nomadeer is already at {version}")
    if not precedes(old, version):
        raise ReleaseError(f"{version} is not newer than the current version {old}")
    if lock_version(cargo_lock) != old:
        raise ReleaseError(f"Cargo.lock's nomadeer is {lock_version(cargo_lock)}, not {old}")

    # Build every file first so a refusal leaves them all untouched.
    new_changelog = bump_changelog(changelog, old, version, date)
    _, notes = changelog_section(new_changelog, version)
    new_metainfo = bump_metainfo(metainfo, version, date, notes)
    new_toml = CARGO_VERSION.sub(lambda m: m.group(1) + version + m.group(3), cargo_toml, 1)
    new_lock = LOCK_VERSION.sub(lambda m: m.group(1) + version + m.group(3), cargo_lock, 1)

    CHANGELOG.write_text(new_changelog)
    METAINFO.write_text(new_metainfo)
    CARGO_TOML.write_text(new_toml)
    CARGO_LOCK.write_text(new_lock)
    check(version, None)

    # Cargo would rewrite Cargo.lock if the edit above left it inconsistent.
    cargo = os.environ.get("CARGO", "cargo")
    try:
        result = subprocess.run(
            [cargo, "metadata", "--format-version", "1", "--offline", "--locked"],
            cwd=ROOT,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
    except FileNotFoundError:
        print("note: cargo not found, Cargo.lock was not checked by cargo", file=sys.stderr)
    else:
        if result.returncode != 0:
            raise ReleaseError(f"cargo does not accept the updated Cargo.lock:\n{result.stderr}")

    print(f"Bumped Nomadeer {old} -> {version} ({date}).")
    files = " ".join(str(p.relative_to(ROOT)) for p in (CHANGELOG, CARGO_TOML, CARGO_LOCK, METAINFO))
    print("Review the changes, commit them, then tag the commit:")
    print(f'  git commit -m "Release {version}" {files}')
    print(f'  git tag -a v{version} -m "Nomadeer {version}"')


def notes(version: str, downloads: bool) -> None:
    _, body = changelog_section(CHANGELOG.read_text(), version)
    if not body:
        raise ReleaseError(f"CHANGELOG.md's section for {version} is empty")
    print(body)
    if downloads:
        print()
        print(downloads_table(version), end="")


def check(version: str | None, tag: str | None) -> None:
    current = cargo_version()
    version = version or current
    parse_version(version)
    problems = []
    if tag is not None and tag != f"v{current}":
        problems.append(f"tag {tag} does not match Cargo.toml's version {current} (expected v{current})")
    if current != version:
        problems.append(f"Cargo.toml's version is {current}, not {version}")
    if lock_version(CARGO_LOCK.read_text()) != version:
        problems.append(f"Cargo.lock's nomadeer is {lock_version(CARGO_LOCK.read_text())}, not {version}")

    changelog_date = None
    try:
        changelog_date, body = changelog_section(CHANGELOG.read_text(), version)
        if not body:
            problems.append(f"CHANGELOG.md's section for {version} is empty")
        if not changelog_date or not DATE.fullmatch(changelog_date):
            problems.append(f"CHANGELOG.md's section for {version} has no '- YYYY-MM-DD' date")
    except ReleaseError as error:
        problems.append(str(error))

    releases = metainfo_releases(METAINFO.read_text())
    if not releases or releases[0][0] != version:
        problems.append(f"the metainfo's newest <release> is not {version}")
    elif changelog_date and releases[0][1] != changelog_date:
        problems.append(
            f"the metainfo dates {version} {releases[0][1]}, CHANGELOG.md {changelog_date}"
        )

    if problems:
        raise ReleaseError("release check failed:\n  " + "\n  ".join(problems))
    print(f"Nomadeer {version}: Cargo.toml, Cargo.lock, metainfo and CHANGELOG.md agree.")


def assets(version: str, directory: str | None) -> None:
    expected = release_assets(version)
    if directory is None:
        print("\n".join(expected))
        return
    present = {p.name for p in Path(directory).iterdir() if p.is_file()}
    missing = [name for name in expected if name not in present]
    extra = sorted(present - set(expected))
    if missing or extra:
        lines = [f"missing: {name}" for name in missing] + [f"unexpected: {name}" for name in extra]
        raise ReleaseError(f"{directory} does not hold the release files:\n  " + "\n  ".join(lines))
    print(f"{directory} holds all {len(expected)} release files for {version}.")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    p = commands.add_parser("bump", help="release the Unreleased changelog notes as VERSION")
    p.add_argument("version")
    p.add_argument("--date", default=datetime.date.today().isoformat())
    p = commands.add_parser("notes", help="print a version's changelog section")
    p.add_argument("version", nargs="?")
    p.add_argument("--downloads", action="store_true", help="append the downloads table")
    p = commands.add_parser("check", help="check every file carries the version")
    p.add_argument("version", nargs="?")
    p.add_argument("--tag", help="the git tag being released, e.g. v1.2.3")
    p = commands.add_parser("assets", help="list or check a release's file names")
    p.add_argument("version", nargs="?")
    p.add_argument("--check", metavar="DIR", help="fail unless DIR holds exactly these files")
    commands.add_parser("version", help="print Cargo.toml's version")
    args = parser.parse_args(argv)

    try:
        if args.command == "bump":
            bump(args.version, args.date)
        elif args.command == "notes":
            notes(args.version or cargo_version(), args.downloads)
        elif args.command == "check":
            check(args.version, args.tag)
        elif args.command == "assets":
            assets(args.version or cargo_version(), args.check)
        else:
            print(cargo_version())
    except ReleaseError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
