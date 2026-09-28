#!/usr/bin/env python3
"""Dry-run and apply a reviewed Degauss artwork manifest.

The manifest pins one live system root, its gamelist hash, every artwork file,
and every exact game path to edit or append. The default mode is read-only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import subprocess
import tempfile
import time
import uuid
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape


ART_FIELDS = ("image", "screenshot", "thumbnail")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def run(args: list[str], *, data: bytes | None = None) -> bytes:
    result = subprocess.run(args, input=data, capture_output=True, check=False)
    if result.returncode:
        message = result.stderr.decode(errors="replace").strip()
        raise RuntimeError(f"Command failed ({result.returncode}): {message}")
    return result.stdout


def ssh(command: str) -> bytes:
    return run(["ssh", "-o", "BatchMode=yes", "mister", command])


def remote_digest(path: str) -> tuple[int, str] | None:
    command = (
        f"if test -f {shlex.quote(path)}; then "
        f"wc -c < {shlex.quote(path)}; sha256sum {shlex.quote(path)} | cut -d' ' -f1; "
        "else printf 'MISSING\\n'; fi"
    )
    lines = ssh(command).decode().splitlines()
    if lines == ["MISSING"]:
        return None
    if len(lines) != 2:
        raise RuntimeError(f"Could not read size and hash for {path}")
    return int(lines[0].strip()), lines[1].strip()


def remote_digests(paths: list[str]) -> dict[str, tuple[int, str] | None]:
    unique = list(dict.fromkeys(paths))
    if not unique:
        return {}
    code = '''import hashlib, json, os
paths = PATHS
result = {}
for path in paths:
    if not os.path.isfile(path):
        result[path] = None
        continue
    digest = hashlib.sha256()
    size = 0
    with open(path, "rb") as handle:
        while chunk := handle.read(1024 * 1024):
            size += len(chunk)
            digest.update(chunk)
    result[path] = [size, digest.hexdigest()]
print(json.dumps(result))
'''.replace("PATHS", repr(unique))
    raw = run(["ssh", "-o", "BatchMode=yes", "mister", "python3 -"], data=code.encode())
    parsed = json.loads(raw)
    return {path: (int(value[0]), value[1]) if value is not None else None for path, value in parsed.items()}


def safe_root(value: str) -> str:
    path = PurePosixPath(value)
    if not path.is_absolute() or ".." in path.parts or not value.startswith("/media/fat/"):
        raise ValueError(f"Unsafe MiSTer root: {value!r}")
    return str(path)


def safe_relative(value: str, *, label: str) -> str:
    if not value.startswith("./"):
        raise ValueError(f"{label} must start with './': {value!r}")
    path = PurePosixPath(value[2:])
    if path.is_absolute() or not path.parts or ".." in path.parts:
        raise ValueError(f"Unsafe {label}: {value!r}")
    return "./" + str(path)


def remote_from_relative(root: str, value: str) -> str:
    return root.rstrip("/") + "/" + safe_relative(value, label="artwork path")[2:]


def load_manifest(path: str) -> dict:
    manifest = json.loads(Path(path).read_text(encoding="utf-8"))
    if manifest.get("version") != 1:
        raise ValueError("Manifest version must be 1")
    manifest["root"] = safe_root(manifest.get("root", ""))
    digest = manifest.get("gamelist_sha256", "")
    if not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ValueError("Manifest has no valid gamelist_sha256")
    operations = manifest.get("operations")
    if not isinstance(operations, list) or not operations:
        raise ValueError("Manifest operations must be a non-empty list")
    if manifest.get("expected_operations") != len(operations):
        raise ValueError("Manifest operation count does not match expected_operations")
    return manifest


def verify_image(image: dict, root: str) -> tuple[str, tuple[int, str]]:
    expected = (int(image.get("size", -1)), image.get("sha256", ""))
    if expected[0] < 1 or not re.fullmatch(r"[0-9a-f]{64}", expected[1]):
        raise ValueError("Image has no valid size/SHA-256")
    kind = image.get("kind")
    if kind == "existing":
        remote = remote_from_relative(root, image.get("path", ""))
        if remote_digest(remote) != expected:
            raise ValueError(f"Existing image differs from manifest: {remote}")
        return remote, expected
    if kind == "local":
        local = Path(image.get("path", ""))
        data = local.read_bytes()
        if (len(data), sha256(data)) != expected:
            raise ValueError(f"Local image differs from manifest: {local}")
        return str(local), expected
    raise ValueError(f"Unsupported image kind: {kind!r}")


def read_entries(root: ET.Element) -> dict[str, list[ET.Element]]:
    entries: dict[str, list[ET.Element]] = {}
    for game in root.findall("game"):
        path = (game.findtext("path") or "").strip()
        if path:
            entries.setdefault(path, []).append(game)
    return entries


def update_xml(xml_data: bytes, operations: list[dict]) -> bytes:
    text = xml_data.decode("utf-8")
    xml_root = ET.fromstring(text)
    if xml_root.tag != "gameList":
        raise ValueError("Gamelist root is not <gameList>")
    entries = read_entries(xml_root)
    matches = list(re.finditer(r"<game\b[^>]*>.*?</game>", text, re.S))
    if len(matches) != len(xml_root.findall("game")):
        raise ValueError("Cannot locate every existing game without rewriting the XML")
    fragments: dict[str, list[tuple[re.Match[str], ET.Element]]] = {}
    for match in matches:
        game = ET.fromstring(match.group())
        path = (game.findtext("path") or "").strip()
        if path:
            fragments.setdefault(path, []).append((match, game))
    seen: set[str] = set()
    replacements: list[tuple[int, int, str]] = []
    additions: list[str] = []
    for operation in operations:
        path = safe_relative(operation.get("path", ""), label="game path")
        artwork = safe_relative(operation.get("artwork", ""), label="artwork path")
        if path in seen:
            raise ValueError(f"Duplicate manifest game path: {path}")
        seen.add(path)
        matches = entries.get(path, [])
        if len(matches) > 1:
            raise ValueError(f"Gamelist contains duplicate exact path: {path}")
        if matches:
            located = fragments.get(path, [])
            if len(located) != 1:
                raise ValueError(f"Could not locate one exact XML fragment for: {path}")
            match, game = located[0]
            fragment = match.group()
            field_name = next((name for name in ART_FIELDS if game.find(name) is not None), None)
            value = escape(artwork)
            if field_name is None:
                position = fragment.rfind("</game>")
                changed = fragment[:position] + f"<screenshot>{value}</screenshot>" + fragment[position:]
            else:
                paired = re.compile(rf"<{field_name}\b([^>]*)>.*?</{field_name}>", re.S)
                empty = re.compile(rf"<{field_name}\b([^>]*)/>", re.S)
                if paired.search(fragment):
                    changed, count = paired.subn(rf"<{field_name}\1>{value}</{field_name}>", fragment, count=1)
                else:
                    changed, count = empty.subn(rf"<{field_name}\1>{value}</{field_name}>", fragment, count=1)
                if count != 1:
                    raise ValueError(f"Could not replace the existing {field_name} field for: {path}")
            replacements.append((match.start(), match.end(), changed))
        else:
            name = str(operation.get("name", "")).strip()
            if not name:
                raise ValueError(f"New gamelist entry requires a name: {path}")
            additions.append(
                f"  <game><path>{escape(path)}</path><name>{escape(name)}</name>"
                f"<screenshot>{escape(artwork)}</screenshot></game>\n"
            )
    for start, end, changed in sorted(replacements, reverse=True):
        text = text[:start] + changed + text[end:]
    if additions:
        closing = text.rfind("</gameList>")
        if closing >= 0 and not text[closing + len("</gameList>"):].strip():
            text = text[:closing] + "".join(additions) + text[closing:]
        else:
            empty_root = list(re.finditer(r"<gameList\b([^>]*)/\s*>", text))
            if len(empty_root) != 1 or text[empty_root[0].end():].strip():
                raise ValueError("Gamelist has no unambiguous closing tag")
            match = empty_root[0]
            expanded = f"<gameList{match.group(1)}>\n{''.join(additions)}</gameList>"
            text = text[:match.start()] + expanded + text[match.end():]
    updated = text.encode("utf-8")
    ET.fromstring(updated)
    return updated


def assert_no_writer() -> None:
    processes = ssh("ps ww").decode(errors="replace").splitlines()
    pattern = re.compile(r"(?i)(update_all|downloader\.sh|screenscraper|scrap(?:e|ing)|gamelist.*(?:write|edit))")
    busy = [line for line in processes if pattern.search(line) and "ps ww" not in line]
    if busy:
        raise RuntimeError("Another updater, scraper, or gamelist writer appears to be running")


def copy_local(local: str, remote: str, expected: tuple[int, str]) -> None:
    existing = remote_digest(remote)
    if existing == expected:
        return
    if existing is not None:
        raise RuntimeError(f"Refusing to replace a different existing artwork file: {remote}")
    part = remote + ".part-artwork-" + uuid.uuid4().hex
    try:
        ssh(f"mkdir -p {shlex.quote(str(PurePosixPath(remote).parent))}")
        run(["scp", "-q", local, f"mister:{part}"])
        if remote_digest(part) != expected:
            raise RuntimeError(f"Transferred artwork differs from manifest: {remote}")
        ssh(f"mv {shlex.quote(part)} {shlex.quote(remote)} && sync")
        if remote_digest(remote) != expected:
            raise RuntimeError(f"Final artwork differs from manifest: {remote}")
    finally:
        ssh(f"if test -e {shlex.quote(part)}; then rm {shlex.quote(part)}; fi")


def copy_local_batch(items: list[tuple[str, str, tuple[int, str]]]) -> None:
    unique: dict[str, tuple[str, tuple[int, str]]] = {}
    for local, remote, expected in items:
        prior = unique.get(remote)
        if prior is not None and prior != (local, expected):
            raise RuntimeError(f"Conflicting local images target the same path: {remote}")
        unique[remote] = (local, expected)
    if not unique:
        return
    current = remote_digests(list(unique))
    pending = {
        remote: value for remote, value in unique.items()
        if current.get(remote) != value[1]
    }
    for remote, (_, expected) in pending.items():
        if current.get(remote) is not None:
            raise RuntimeError(f"Refusing to replace a different existing artwork file: {remote}")
    if not pending:
        return
    parts = {remote: remote + ".part-artwork-" + uuid.uuid4().hex for remote in pending}
    try:
        directories = sorted({str(PurePosixPath(remote).parent) for remote in pending})
        ssh("mkdir -p " + " ".join(shlex.quote(path) for path in directories))
        for remote, (local, _) in pending.items():
            run(["scp", "-q", local, f"mister:{parts[remote]}"])
        part_actual = remote_digests(list(parts.values()))
        for remote, (_, expected) in pending.items():
            if part_actual.get(parts[remote]) != expected:
                raise RuntimeError(f"Transferred artwork differs from manifest: {remote}")
        moves = [(parts[remote], remote) for remote in pending]
        code = '''import os
moves = MOVES
existing = [target for _, target in moves if os.path.exists(target)]
if existing:
    raise RuntimeError("an artwork target appeared during upload")
for part, target in moves:
    os.replace(part, target)
os.sync()
'''.replace("MOVES", repr(moves))
        run(["ssh", "-o", "BatchMode=yes", "mister", "python3 -"], data=code.encode())
        final = remote_digests(list(pending))
        for remote, (_, expected) in pending.items():
            if final.get(remote) != expected:
                raise RuntimeError(f"Final artwork differs from manifest: {remote}")
    finally:
        ssh("for path in " + " ".join(shlex.quote(path) for path in parts.values())
            + "; do if test -e \"$path\"; then rm \"$path\"; fi; done")


def write_gamelist(path: str, original: bytes, updated: bytes) -> str:
    old_digest = len(original), sha256(original)
    if remote_digest(path) != old_digest:
        raise RuntimeError("Live gamelist changed before write")
    stamp = time.strftime("%Y%m%d-%H%M%S")
    backup = f"{path}.bak-artwork-{stamp}"
    ssh(f"cp {shlex.quote(path)} {shlex.quote(backup)} && sync")
    if remote_digest(backup) != old_digest:
        raise RuntimeError("Gamelist backup verification failed")
    part = path + ".part-artwork-" + uuid.uuid4().hex
    local_part = ""
    try:
        with tempfile.NamedTemporaryFile(prefix="degauss-gamelist-", suffix=".xml", delete=False) as handle:
            handle.write(updated)
            local_part = handle.name
        run(["scp", "-q", local_part, f"mister:{part}"])
        expected = len(updated), sha256(updated)
        if remote_digest(part) != expected:
            raise RuntimeError("Transferred gamelist differs from reviewed bytes")
        if remote_digest(path) != old_digest:
            raise RuntimeError("Live gamelist changed during upload")
        ssh(f"mv {shlex.quote(part)} {shlex.quote(path)} && sync")
        if remote_digest(path) != expected or ssh(f"cat {shlex.quote(path)}") != updated:
            raise RuntimeError("Final gamelist reread differs from reviewed bytes")
        return backup
    finally:
        if local_part:
            os.unlink(local_part)
        ssh(f"if test -e {shlex.quote(part)}; then rm {shlex.quote(part)}; fi")


def plan(manifest: dict) -> tuple[bytes, bytes, list[tuple[dict, str, tuple[int, str]]]]:
    root = manifest["root"]
    gamelist = root.rstrip("/") + "/gamelist.xml"
    original = ssh(f"cat {shlex.quote(gamelist)}")
    if sha256(original) != manifest["gamelist_sha256"]:
        raise ValueError("Live gamelist SHA-256 differs from manifest")
    verified = []
    image_cache: dict[str, tuple[str, tuple[int, str]]] = {}
    existing_descriptors: dict[str, tuple[str, tuple[int, str]]] = {}
    for operation in manifest["operations"]:
        descriptor = operation.get("image", {})
        if descriptor.get("kind") != "existing":
            continue
        expected = (int(descriptor.get("size", -1)), descriptor.get("sha256", ""))
        if expected[0] < 1 or not re.fullmatch(r"[0-9a-f]{64}", expected[1]):
            raise ValueError("Image has no valid size/SHA-256")
        remote = remote_from_relative(root, descriptor.get("path", ""))
        existing_descriptors[json.dumps(descriptor, sort_keys=True, separators=(",", ":"))] = (remote, expected)
    existing_actual = remote_digests([item[0] for item in existing_descriptors.values()])
    for cache_key, (remote, expected) in existing_descriptors.items():
        if existing_actual.get(remote) != expected:
            raise ValueError(f"Existing image differs from manifest: {remote}")
        image_cache[cache_key] = (remote, expected)
    for operation in manifest["operations"]:
        artwork = safe_relative(operation.get("artwork", ""), label="artwork path")
        descriptor = operation.get("image", {})
        cache_key = json.dumps(descriptor, sort_keys=True, separators=(",", ":"))
        if cache_key not in image_cache:
            image_cache[cache_key] = verify_image(descriptor, root)
        source, digest = image_cache[cache_key]
        if operation["image"]["kind"] == "existing" and operation["image"]["path"] != artwork:
            raise ValueError("Existing image path must equal the operation artwork path")
        verified.append((operation, source, digest))
    updated = update_xml(original, manifest["operations"])
    ET.fromstring(updated)
    return original, updated, verified


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    manifest = load_manifest(args.manifest)
    original, updated, verified = plan(manifest)
    print(json.dumps({
        "system": manifest.get("system", ""),
        "root": manifest["root"],
        "operations": len(verified),
        "new_gamelist_sha256": sha256(updated),
        "images_to_upload": sum(item[0]["image"]["kind"] == "local" for item in verified),
    }, indent=2))
    for operation, source, _ in verified:
        print(f"{operation['path']} -> {operation['artwork']} ({source})")
    if not args.apply:
        print("Dry run only; no files changed")
        return 0
    assert_no_writer()
    root = manifest["root"]
    copies: list[tuple[str, str, tuple[int, str]]] = []
    for operation, source, digest in verified:
        if operation["image"]["kind"] == "local":
            target = remote_from_relative(root, operation["artwork"])
            copies.append((source, target, digest))
    copy_local_batch(copies)
    gamelist = root.rstrip("/") + "/gamelist.xml"
    backup = write_gamelist(gamelist, original, updated)
    final_root = ET.fromstring(ssh(f"cat {shlex.quote(gamelist)}"))
    final_entries = read_entries(final_root)
    final_expected: dict[str, tuple[int, str]] = {}
    for operation, _, digest in verified:
        path = operation["path"]
        game = final_entries[path][0]
        value = next((game.findtext(field) for field in ART_FIELDS if game.find(field) is not None), "")
        if value != operation["artwork"]:
            raise RuntimeError(f"Final artwork reference differs for {path}")
        remote = remote_from_relative(root, value)
        if remote in final_expected and final_expected[remote] != digest:
            raise RuntimeError(f"Conflicting expected artwork hashes for {remote}")
        final_expected[remote] = digest
    final_images = remote_digests(list(final_expected))
    for remote, digest in final_expected.items():
        if final_images.get(remote) != digest:
            raise RuntimeError(f"Final artwork file differs for {remote}")
    print(f"Applied {len(verified)} operations; backup: {backup}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
