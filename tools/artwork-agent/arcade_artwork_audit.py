#!/usr/bin/env python3
"""Scoped Arcade artwork audit and reviewed gamelist installation.

Run on the computer with ``python3 arcade_artwork_audit.py`` for a read-only audit.
``--manifest FILE`` prints a full installation dry run; adding ``--apply``
installs only that manifest after verifying its exact expected scope.
The only device connection is the configured ``ssh mister`` alias.
"""

from __future__ import annotations

import argparse
import collections
import datetime
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import uuid
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape


ARCADE = "/media/fat/_Arcade"
LOG = "/media/fat/Scripts/.config/downloader/downloader.log"


def ssh(command: str, input_text: str | None = None) -> str:
    result = subprocess.run(
        ["ssh", "-o", "BatchMode=yes", "mister", command],
        input=input_text,
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(f"SSH {command!r} failed: {result.stderr.strip()}")
    return result.stdout


def ssh_bytes(command: str) -> bytes:
    result = subprocess.run(
        ["ssh", "-o", "BatchMode=yes", "mister", command],
        capture_output=True,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(f"SSH {command!r} failed: {result.stderr.decode(errors='replace').strip()}")
    return result.stdout


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def remote_digest(path: str) -> tuple[int, str] | None:
    quoted = shlex.quote(path)
    output = ssh(f"if test -f {quoted}; then stat -c %s {quoted}; sha256sum {quoted}; fi").splitlines()
    if not output:
        return None
    if len(output) != 2:
        raise RuntimeError(f"Unexpected size/hash response for {path}")
    return int(output[0]), output[1].split()[0]


def copy_to_part(local_path: str, final_path: str, expected_hash: str, expected_size: int) -> None:
    current = remote_digest(final_path)
    if current == (expected_size, expected_hash):
        return
    if current is not None:
        raise RuntimeError(f"Refusing to replace a different existing image: {final_path}")
    part = final_path + ".part-arcade-art-" + uuid.uuid4().hex
    try:
        result = subprocess.run(
            ["scp", "-q", local_path, f"mister:{part}"],
            capture_output=True,
            check=False,
        )
        if result.returncode:
            raise RuntimeError(f"SCP to {part} failed: {result.stderr.decode(errors='replace').strip()}")
        if remote_digest(part) != (expected_size, expected_hash):
            raise RuntimeError(f"Transferred size/hash mismatch for {part}")
        ssh(f"mv {shlex.quote(part)} {shlex.quote(final_path)} && sync")
        if remote_digest(final_path) != (expected_size, expected_hash):
            raise RuntimeError(f"Final size/hash mismatch for {final_path}")
    finally:
        ssh(f"if test -e {shlex.quote(part)}; then rm {shlex.quote(part)}; fi")


def normalize_path(path: str) -> str:
    path = path.strip().replace("\\", "/")
    if path.startswith("./"):
        path = path[2:]
    return path.lstrip("/")


def stem(name: str) -> str:
    index = name.rfind(".")
    return name[:index] if index > 0 else name


def slugify(value: str) -> str:
    return "".join(char.lower() for char in value if char.isascii() and char.isalnum())


def without_tags(value: str) -> str:
    output = []
    depth = 0
    for char in value:
        if char in "([":
            depth += 1
        elif char in ")]":
            depth = max(0, depth - 1)
        elif depth == 0:
            output.append(char)
    return "".join(output).strip()


def digits_for_words(value: str) -> str:
    words = {name: str(index) for index, name in enumerate(
        ("one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"),
        start=1,
    )}
    output = []
    for word in value.split():
        bare = slugify(word)
        output.append(words.get(bare, word))
    return " ".join(output)


def slug_candidates(value: str) -> list[str]:
    variants = (value, without_tags(value), digits_for_words(value), digits_for_words(without_tags(value)))
    return list(dict.fromkeys(candidate for variant in variants if (candidate := slugify(variant))))


def artwork(game: ET.Element | None) -> str | None:
    if game is None:
        return None
    for tag in ("image", "screenshot", "thumbnail"):
        value = game.findtext(tag)
        if value is not None:
            return value.strip()
    return None


def read_gamelist(xml_text: str) -> tuple[list[dict], dict]:
    root = ET.fromstring(xml_text)
    if root.tag != "gameList":
        raise ValueError("Arcade gamelist root is not <gameList>")
    games = root.findall("game")
    parents = {game.get("id"): game for game in games if game.get("id")}
    entries = []
    by_path: dict[str, dict] = {}
    by_filename: dict[str, dict] = {}
    by_stem: dict[str, dict] = {}
    by_slug: dict[str, dict] = {}
    for game in games:
        raw_path = game.findtext("path")
        if raw_path is None:
            continue
        path = normalize_path(raw_path)
        parent = parents.get(game.get("parentid"))
        own_art = artwork(game)
        art = own_art if own_art is not None else artwork(parent)
        name = game.findtext("name")
        if name is None and parent is not None:
            name = parent.findtext("name")
        entry = {
            "path": path,
            "name": (name or "").strip(),
            "art": art,
            "parentid": game.get("parentid"),
            "source": game.get("source"),
        }
        # Match Degauss gamelist.rs: a path-only element carries no metadata
        # and is not indexed. Keeping it would mask a later filename, stem or
        # slug entry that actually supplies artwork.
        if not entry["name"] and entry["art"] is None:
            continue
        entries.append(entry)
        if not path:
            continue
        if path.endswith(".slug"):
            by_slug[slugify(path[:-5])] = entry
            continue
        filename = os.path.basename(path).lower()
        by_path[path] = entry  # Degauss keeps the final exact-path entry.
        by_filename.setdefault(filename, entry)
        by_stem.setdefault(stem(filename), entry)
    tables = {"path": by_path, "filename": by_filename, "stem": by_stem, "slug": by_slug}
    return entries, tables


def degauss_lookup(path: str, tables: dict) -> tuple[dict | None, str]:
    """Mirror the current Arcade-relevant lookup order in Degauss gamelist.rs."""
    path = normalize_path(path)
    if entry := tables["path"].get(path):
        return entry, "exact"
    filename = os.path.basename(path).lower()
    if entry := tables["filename"].get(filename):
        return entry, "filename"
    file_stem = stem(filename)
    if entry := tables["stem"].get(file_stem):
        return entry, "stem"
    for candidate in slug_candidates(file_stem):
        if entry := tables["slug"].get(candidate):
            return entry, "slug"
    return None, "none"


def latest_mras(log_text: str) -> list[str]:
    paths = []
    for line in log_text.splitlines():
        if line.startswith("_Arcade/") and line.endswith(".mra"):
            path = line[len("_Arcade/"):]
            if not path or path.startswith("/") or ".." in path.split("/"):
                raise ValueError(f"Unsafe MRA path in downloader log: {line!r}")
            paths.append(path)
    if not paths:
        raise ValueError("No Arcade MRAs found in the current downloader log")
    return list(dict.fromkeys(paths))


def remote_mras(paths: list[str]) -> list[dict]:
    code = '''import json, os, re, xml.etree.ElementTree as ET
paths = json.loads(PATHS_JSON)
root_dir = "/media/fat/_Arcade"
result = []
for rel in paths:
    if rel.startswith("/") or ".." in rel.split("/"):
        raise ValueError("Unsafe MRA path")
    path = os.path.join(root_dir, rel)
    record = {"path": rel, "exists": os.path.isfile(path), "realpath": os.path.realpath(path), "symlink": os.path.islink(path)}
    if record["exists"]:
        try:
            root = ET.parse(path).getroot()
            record.update({key: (root.findtext(key) or "").strip() for key in ("name", "setname", "rbf")})
            record["zip_chains"] = [rom.get("zip", "") for rom in root.iter("rom") if rom.get("zip")]
            record["parse"] = "xml"
        except ET.ParseError:
            source = open(path, encoding="utf-8", errors="replace").read()
            for key in ("name", "setname", "rbf"):
                found = re.search(r"<" + key + r"(?:\\s[^>]*)?>(.*?)</" + key + r">", source, re.I | re.S)
                record[key] = found.group(1).strip() if found else ""
            record["zip_chains"] = re.findall(r"<rom\\b[^>]*\\bzip\\s*=\\s*['\\\"]([^'\\\"]+)", source, re.I)
            record["parse"] = "fallback"
    result.append(record)
print(json.dumps(result))
'''.replace("PATHS_JSON", repr(json.dumps(paths)))
    return json.loads(ssh("python3 -", code))


def remote_organized(mras: list[dict]) -> list[dict]:
    """Find only organized symlinks targeting a latest-run MRA."""
    targets = {item["realpath"] for item in mras if item["exists"]}
    code = '''import json, os
targets = set(json.loads(TARGETS_JSON))
root = "/media/fat/_Arcade"
organized = os.path.join(root, "_Organized")
result = []
for directory, _, filenames in os.walk(organized, followlinks=False):
    for filename in filenames:
        path = os.path.join(directory, filename)
        if filename.endswith(".mra") and os.path.islink(path):
            link = os.readlink(path)
            target = link if link.startswith("/") else os.path.normpath(os.path.join(directory, link))
            if target in targets:
                result.append({"path": os.path.relpath(path, root), "realpath": target, "exists": os.path.isfile(path)})
print(json.dumps(sorted(result, key=lambda item: item["path"])))
'''.replace("TARGETS_JSON", repr(json.dumps(sorted(targets))))
    return json.loads(ssh("python3 -", code))


def remote_images(refs: list[str]) -> dict[str, dict]:
    code = '''import json, os
refs = json.loads(REFS_JSON)
base = "/media/fat/_Arcade"
result = {}
for ref in refs:
    path = os.path.normpath(os.path.join(base, ref))
    if not path.startswith("/media/fat/"):
        result[ref] = {"valid": False, "reason": "outside card"}
        continue
    try:
        with open(path, "rb") as handle:
            header = handle.read(16)
        size = os.path.getsize(path)
        fmt = "png" if header.startswith(b"\\x89PNG\\r\\n\\x1a\\n") else "jpeg" if header.startswith(b"\\xff\\xd8\\xff") else "other"
        result[ref] = {"valid": size > 100 and fmt in ("png", "jpeg"), "size": size, "format": fmt}
    except OSError as error:
        result[ref] = {"valid": False, "reason": str(error)}
print(json.dumps(result))
'''.replace("REFS_JSON", repr(json.dumps(refs)))
    return json.loads(ssh("python3 -", code))


def candidate_image_refs(mras: list[dict]) -> list[str]:
    refs = set()
    for mra in mras:
        names = [mra.get("setname", "")]
        for chain in mra.get("zip_chains", []):
            names.extend(os.path.splitext(os.path.basename(archive))[0] for archive in chain.split("|"))
        for name in names:
            if re.fullmatch(r"[A-Za-z0-9_.-]+", name or ""):
                for extension in ("png", "jpg", "jpeg"):
                    refs.add(f"media/screenshot/{name}.{extension}")
    return sorted(refs)


def audit(log_text: str, gamelist_text: str, include_organized: bool = True) -> dict:
    paths = latest_mras(log_text)
    entries, tables = read_gamelist(gamelist_text)
    listed_mras = remote_mras(paths)
    if [mra["path"] for mra in listed_mras] != paths:
        raise RuntimeError("MRA metadata response did not match the requested paths")
    mras = list(listed_mras)
    if include_organized:
        by_target = {item["realpath"]: item for item in listed_mras if item["exists"]}
        listed_paths = set(paths)
        for link in remote_organized(listed_mras):
            if link["path"] in listed_paths:
                continue
            source = by_target[link["realpath"]]
            mras.append({**source, **link, "symlink": True, "organized_source": source["path"]})
    checked = []
    for mra in mras:
        entry, match = degauss_lookup(mra["path"], tables)
        checked.append({**mra, "match": match, "entry": entry})
    refs = {item["entry"]["art"] for item in checked if item["entry"] and item["entry"]["art"]}
    refs.update(candidate_image_refs([item for item in checked if item["match"] == "none"]))
    images = remote_images(sorted(refs))
    for item in checked:
        entry = item["entry"]
        item["art_valid"] = images.get(entry["art"], {}).get("valid", False) if entry and entry["art"] else False
        item["local_images"] = [ref for ref in candidate_image_refs([item]) if images.get(ref, {}).get("valid")]
        realpath = item.get("realpath", "")
        if realpath.startswith(ARCADE + "/"):
            target = realpath[len(ARCADE) + 1:]
            item["target_entry"] = tables["path"].get(target)
        else:
            item["target_entry"] = None
    return {
        "scope": "MRAs explicitly listed in latest downloader log",
        "listed_mras": len(paths),
        "organized_links": len(mras) - len(paths),
        "gamelist_games": len(ET.fromstring(gamelist_text).findall("game")),
        "gamelist_nonblank_paths": sum(bool(item["path"]) for item in entries),
        "gamelist_unique_paths": len({item["path"] for item in entries if item["path"]}),
        "gamelist_metadata_only": len(ET.fromstring(gamelist_text).findall("game")) - sum(bool(item["path"]) for item in entries),
        "match_counts": dict(collections.Counter(item["match"] for item in checked)),
        "mra_parse_counts": dict(collections.Counter(item.get("parse", "unreadable") for item in checked)),
        "items": checked,
    }


def review_manifest(manifest: dict, audit_result: dict, log_bytes: bytes, gamelist_bytes: bytes) -> tuple[list[dict], list[dict]]:
    if manifest.get("log_sha256") != sha256(log_bytes):
        raise ValueError("Downloader log changed since manifest review")
    if manifest.get("gamelist_sha256") != sha256(gamelist_bytes):
        raise ValueError("Arcade gamelist changed since manifest review")
    groups = manifest.get("groups")
    if not isinstance(groups, list) or not groups:
        raise ValueError("Manifest must contain nonempty groups")
    by_setname: dict[str, dict] = {}
    for group in groups:
        names = group.get("setnames")
        if not isinstance(names, list) or not names or not all(isinstance(x, str) and x for x in names):
            raise ValueError("Every group must name its exact MRA setnames")
        image = group.get("image")
        target = group.get("target")
        if not isinstance(image, str) or not isinstance(target, str):
            raise ValueError("Every group needs an image and target")
        if not re.fullmatch(r"media/screenshot/[A-Za-z0-9_.-]+\.(?:png|jpg|jpeg)", target):
            raise ValueError(f"Unsafe screenshot target: {target!r}")
        if not os.path.isfile(image):
            raise ValueError(f"Source image is missing: {image}")
        with open(image, "rb") as handle:
            source = handle.read()
        extension = os.path.splitext(target)[1].lower()
        valid = source.startswith(b"\x89PNG\r\n\x1a\n") if extension == ".png" else source.startswith(b"\xff\xd8\xff")
        if not valid or len(source) <= 100:
            raise ValueError(f"Source image format is invalid: {image}")
        if group.get("sha256") != sha256(source):
            raise ValueError(f"Source image hash differs from reviewed manifest: {image}")
        if not isinstance(group.get("expected_missing_paths"), int) or group["expected_missing_paths"] < 1:
            raise ValueError("Every group needs its expected missing-path count")
        edit_paths = group.get("edit_paths", [])
        if not isinstance(edit_paths, list) or not all(isinstance(path, str) for path in edit_paths):
            raise ValueError("edit_paths must be an explicit list of Arcade-relative paths")
        if len(set(edit_paths)) != len(edit_paths):
            raise ValueError("edit_paths contains a duplicate path")
        for name in names:
            if name in by_setname:
                raise ValueError(f"Setname appears in more than one group: {name}")
            by_setname[name] = group
    selected = []
    counts = collections.Counter()
    for item in audit_result["items"]:
        group = by_setname.get(item.get("setname", ""))
        if group is None:
            continue
        if not item["exists"] or item["parse"] not in ("xml", "fallback"):
            raise ValueError(f"Selected MRA is missing or unreadable: {item['path']}")
        if item["art_valid"]:
            continue
        if item["match"] == "exact" and item["path"] in group.get("edit_paths", []):
            if item["entry"]["art"]:
                raise ValueError(f"Refusing to replace an existing artwork reference: {item['path']}")
            selected.append({"path": item["path"], "name": item.get("name") or stem(os.path.basename(item["path"])),
                             "setname": item["setname"], "target": group["target"], "operation": "edit"})
            counts[id(group)] += 1
            continue
        if item["match"] != "none":
            raise ValueError(f"Selected path needs an existing-entry edit, not an append: {item['path']}")
        selected.append({"path": item["path"], "name": item.get("name") or stem(os.path.basename(item["path"])),
                         "setname": item["setname"], "target": group["target"], "operation": "append"})
        counts[id(group)] += 1
    for group in groups:
        selected_edits = {item["path"] for item in selected if item["operation"] == "edit"
                          and item["setname"] in group["setnames"]}
        if selected_edits != set(group.get("edit_paths", [])):
            raise ValueError(f"Explicit edit paths did not resolve: {group['setnames']}")
        if counts[id(group)] != group["expected_missing_paths"]:
            raise ValueError(f"Scope changed for {group['setnames']}: expected {group['expected_missing_paths']}, got {counts[id(group)]}")
    if len(selected) != manifest.get("expected_total_new_entries"):
        raise ValueError("Total new-entry count differs from reviewed manifest")
    if len({item["path"] for item in selected}) != len(selected):
        raise ValueError("Reviewed scope contains duplicate paths")
    return selected, groups


def append_entries(original: str, selected: list[dict]) -> str:
    edits = {item["path"]: item for item in selected if item.get("operation") == "edit"}
    if edits:
        matches = list(re.finditer(r"<game\b[^>]*>.*?</game>", original, re.S))
        if len(matches) != len(ET.fromstring(original).findall("game")):
            raise ValueError("Cannot locate every existing game without rewriting the XML")
        replacements = []
        for match in matches:
            fragment = match.group()
            game = ET.fromstring(fragment)
            path = normalize_path(game.findtext("path") or "")
            if path not in edits:
                continue
            if artwork(game) is not None:
                raise ValueError(f"Existing entry already contains an artwork field: {path}")
            art = escape("./" + edits[path]["target"])
            insertion = match.end() - len("</game>")
            replacements.append((path, insertion, f"<screenshot>{art}</screenshot>"))
        counts = collections.Counter(path for path, _, _ in replacements)
        if set(counts) != set(edits) or any(count != 1 for count in counts.values()):
            raise ValueError("An existing path to edit was absent or duplicated in the gamelist")
        for _, position, tag in sorted(replacements, key=lambda item: item[1], reverse=True):
            original = original[:position] + tag + original[position:]
    closing = "</gameList>"
    position = original.rfind(closing)
    if position < 0 or original[position + len(closing):].strip():
        raise ValueError("Arcade gamelist has no unambiguous closing tag")
    lines = []
    for item in selected:
        if item.get("operation") == "edit":
            continue
        path = escape("./" + item["path"])
        name = escape(item["name"])
        art = escape("./" + item["target"])
        lines.append(f"  <game><path>{path}</path><name>{name}</name><screenshot>{art}</screenshot></game>\n")
    addition = "".join(lines)
    result = original[:position] + addition + original[position:]
    if result[:position] != original[:position] or result[position + len(addition):] != original[position:]:
        raise AssertionError("Existing gamelist text changed while adding entries")
    ET.fromstring(result)
    return result


def assert_no_other_writer() -> None:
    lines = ssh("ps ww").splitlines()
    busy = [line for line in lines if re.search(r"(?i)(update_all|downloader\.sh|screenscraper|[Ss]crap(?:e|ing)|gamelist.*(?:write|edit))", line)]
    if busy:
        raise RuntimeError("Another updater, scraper, or gamelist writer appears to be running")


def install_manifest(selected: list[dict], groups: list[dict], gamelist_bytes: bytes) -> tuple[str, str]:
    assert_no_other_writer()
    gamelist_path = f"{ARCADE}/gamelist.xml"
    old_hash = sha256(gamelist_bytes)
    if remote_digest(gamelist_path) != (len(gamelist_bytes), old_hash):
        raise RuntimeError("Live gamelist changed before installation")
    for group in groups:
        with open(group["image"], "rb") as handle:
            source = handle.read()
        copy_to_part(group["image"], f"{ARCADE}/{group['target']}", sha256(source), len(source))
    new_text = append_entries(gamelist_bytes.decode("utf-8"), selected)
    new_bytes = new_text.encode("utf-8")
    new_hash = sha256(new_bytes)
    if remote_digest(gamelist_path) != (len(gamelist_bytes), old_hash):
        raise RuntimeError("Live gamelist changed while images were installed")
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    backup = f"{gamelist_path}.bak-arcade-artwork-{stamp}"
    if remote_digest(backup) is not None:
        raise RuntimeError(f"Backup name already exists: {backup}")
    ssh(f"cp {shlex.quote(gamelist_path)} {shlex.quote(backup)} && sync")
    if remote_digest(backup) != (len(gamelist_bytes), old_hash):
        raise RuntimeError("Gamelist backup size/hash differs from the reviewed version")
    part = gamelist_path + ".part-arcade-art-" + uuid.uuid4().hex
    local_part = None
    try:
        with tempfile.NamedTemporaryFile(prefix="mister-arcade-gamelist-", suffix=".xml", delete=False) as handle:
            local_part = handle.name
            handle.write(new_bytes)
        result = subprocess.run(["scp", "-q", local_part, f"mister:{part}"], capture_output=True, check=False)
        if result.returncode:
            raise RuntimeError(f"Gamelist upload failed: {result.stderr.decode(errors='replace').strip()}")
        if remote_digest(part) != (len(new_bytes), new_hash):
            raise RuntimeError("Uploaded gamelist size/hash differs from reviewed bytes")
        if remote_digest(gamelist_path) != (len(gamelist_bytes), old_hash):
            raise RuntimeError("Live gamelist changed during the upload")
        ssh(f"mv {shlex.quote(part)} {shlex.quote(gamelist_path)} && sync")
        if remote_digest(gamelist_path) != (len(new_bytes), new_hash):
            raise RuntimeError("Final gamelist size/hash differs from reviewed bytes")
    finally:
        if local_part:
            os.unlink(local_part)
        ssh(f"if test -e {shlex.quote(part)}; then rm {shlex.quote(part)}; fi")
    live_xml = ssh_bytes(f"cat {shlex.quote(gamelist_path)}")
    if live_xml != new_bytes:
        raise RuntimeError("Final gamelist reread differs from reviewed bytes")
    _, tables = read_gamelist(live_xml.decode("utf-8"))
    images = remote_images(sorted({item["target"] for item in selected}))
    for item in selected:
        entry, kind = degauss_lookup(item["path"], tables)
        if kind != "exact" or not entry or entry["art"] != "./" + item["target"] or not images[item["target"]]["valid"]:
            raise RuntimeError(f"Installed artwork does not resolve for {item['path']}")
    return backup, new_hash


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", help="local downloader log snapshot; default: live MiSTer")
    parser.add_argument("--gamelist", help="local Arcade gamelist snapshot; default: live MiSTer")
    parser.add_argument("--json", action="store_true", help="print full JSON audit")
    parser.add_argument("--no-organized", action="store_true", help="omit organized symlinks to changed MRAs")
    parser.add_argument("--manifest", help="reviewed JSON image-and-setname manifest; dry-run unless --apply is given")
    parser.add_argument("--apply", action="store_true", help="install only the exact reviewed manifest")
    args = parser.parse_args()
    if args.apply and (not args.manifest or args.log or args.gamelist or args.no_organized):
        parser.error("--apply requires --manifest and live default scope, without --no-organized")
    log_bytes = open(args.log, "rb").read() if args.log else ssh_bytes(f"cat {LOG}")
    gamelist_bytes = open(args.gamelist, "rb").read() if args.gamelist else ssh_bytes(f"cat {ARCADE}/gamelist.xml")
    log_text = log_bytes.decode("utf-8")
    gamelist_text = gamelist_bytes.decode("utf-8")
    result = audit(log_text, gamelist_text, include_organized=not args.no_organized)
    if args.manifest:
        manifest = json.load(open(args.manifest, encoding="utf-8"))
        selected, groups = review_manifest(manifest, result, log_bytes, gamelist_bytes)
        operations = collections.Counter(item.get("operation", "append") for item in selected)
        print(f"Reviewed {len(groups)} image groups: {operations['append']} new entries, "
              f"{operations['edit']} existing-entry artwork edits:")
        for item in selected:
            print(f"{item['path']}\t{item['setname']}\t{item['target']}")
        if args.apply:
            backup, new_hash = install_manifest(selected, groups, gamelist_bytes)
            print(f"Installed {operations['append']} new entries and {operations['edit']} artwork edits; "
                  f"backup {backup}; gamelist SHA-256 {new_hash}")
        return 0
    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0
    print(f"Scope: {result['listed_mras']} latest-log MRAs + {result['organized_links']} linked organized copies")
    print(f"Gamelist: {result['gamelist_games']} games, {result['gamelist_metadata_only']} metadata-only, "
          f"{result['gamelist_nonblank_paths']} nonblank paths, {result['gamelist_unique_paths']} unique paths")
    print(f"MRA parsing: {result['mra_parse_counts']}; Degauss matches: {result['match_counts']}")
    for item in result["items"]:
        if item["match"] == "exact" and item["art_valid"]:
            continue
        art = item["entry"]["art"] if item["entry"] else ""
        target = item["target_entry"]["path"] if item.get("target_entry") else ""
        print("\t".join((item["match"], item["path"], item.get("setname", ""), art or "", target,
                         ",".join(item["local_images"]))))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, ET.ParseError, json.JSONDecodeError) as error:
        print(f"arcade artwork audit failed: {error}", file=sys.stderr)
        sys.exit(1)
