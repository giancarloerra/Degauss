#!/usr/bin/env python3
"""Audit Degauss artwork for complete systems and plan exact local reuse.

The default mode is read-only. ``--output-dir`` writes reviewed-input reports
and pinned manifests locally, but never changes the MiSTer card. The generated
manifests are installed separately with ``degauss_artwork_apply.py``.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shlex
import subprocess
import unicodedata

from arcade_artwork_audit import degauss_lookup, read_gamelist


SYSTEMS = {
    "Amiga": {
        "root": "/media/fat/games/Amiga",
        "platform_id": "64",
        "extensions": ["mgl"],
        "virtual": True,
    },
    "Commodore 64": {
        "root": "/media/fat/games/C64",
        "platform_id": "66",
        "extensions": ["d64", "g64", "t64", "d81", "prg", "crt", "reu", "tap", "mgl"],
    },
    "Genesis": {
        "root": "/media/fat/games/MegaDrive",
        "platform_id": "1",
        "extensions": ["gen", "bin", "md", "mgl"],
    },
    "SNES": {
        "root": "/media/fat/games/SNES",
        "platform_id": "4",
        "extensions": ["sfc", "smc", "bin", "bs", "mgl"],
    },
    "Nintendo 64": {
        "root": "/media/fat/games/N64",
        "platform_id": "14",
        "extensions": ["n64", "z64", "v64", "mgl"],
    },
}

REGIONS = {
    "usa", "europe", "world", "japan", "germany", "france", "spain", "italy",
    "australia", "brazil", "korea", "china", "taiwan", "canada", "uk", "sweden",
    "netherlands", "russia", "asia",
}
LANGUAGES = {"en", "fr", "de", "es", "it", "ja", "pt", "nl", "sv", "ko", "zh", "ru"}
HARDWARE_TAGS = {"ocs", "ecs", "aga", "aga-cd", "ocs-mt-32", "cd32", "ntsc", "pal", "unl"}
GAME_EXTENSIONS = {
    extension
    for config in SYSTEMS.values()
    for extension in config["extensions"]
}
UNCERTAIN_WORDS = {
    "alpha", "beta", "bios", "demo", "diagnostic", "hack", "intro", "kiosk",
    "overdump", "preview", "program", "proto", "prototype", "sample", "trainer",
}


def run(args: list[str], *, input_text: str | None = None) -> str:
    result = subprocess.run(args, input=input_text, text=True, capture_output=True, check=False)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or f"command failed: {args[0]}")
    return result.stdout


def ssh(command: str, *, input_text: str | None = None) -> str:
    return run(["ssh", "-o", "BatchMode=yes", "mister", command], input_text=input_text)


def terminal_tags(value: str) -> tuple[str, list[str]]:
    value = os.path.basename(value)
    stem, extension = os.path.splitext(value)
    if extension[1:].casefold() in GAME_EXTENSIONS:
        value = stem
    tags: list[str] = []
    while True:
        match = re.search(r"\s*(?:\(([^()]*)\)|\[([^\[\]]*)\])\s*$", value)
        if not match:
            break
        tag = (match.group(1) if match.group(1) is not None else match.group(2)).strip()
        tags.append(tag)
        value = value[:match.start()].rstrip()
    return value, tags


def metadata_tag(value: str) -> bool:
    folded = value.casefold().strip()
    values = {part.strip() for part in re.split(r"[,+]", folded) if part.strip()}
    if values and values <= REGIONS:
        return True
    language_values = {part for part in re.split(r"[-,+]", folded) if part}
    if language_values and language_values <= LANGUAGES:
        return True
    if folded in HARDWARE_TAGS:
        return True
    return bool(
        re.fullmatch(r"rev(?:ision)?\s*[a-z0-9.]+", folded)
        or re.fullmatch(r"v(?:er(?:sion)?)?\s*\d[\w. -]*", folded)
        or re.fullmatch(r"(?:disc|disk|side|tape|part)\s*[a-z0-9]+", folded)
    )


def public_title(value: str) -> str:
    title, tags = terminal_tags(value)
    retained = [tag for tag in reversed(tags) if not metadata_tag(tag)]
    if retained:
        title += " " + " ".join(f"({tag})" for tag in retained)
    return title.strip()


def identity_key(value: str) -> str:
    value = unicodedata.normalize("NFKD", public_title(value)).encode("ascii", "ignore").decode()
    value = value.casefold().replace("&", " and ")
    value = re.sub(r"\b(?:the|a|an)\b", " ", value)
    for roman, digit in (("iii", "3"), ("ii", "2"), ("iv", "4")):
        value = re.sub(rf"\b{roman}\b", f" {digit} ", value)
    return re.sub(r"[^a-z0-9]+", "", value)


def uncertain(path: str, category: str = "") -> bool:
    if category.casefold() == "demos":
        return True
    components = [component.casefold() for component in PurePosixPath(path).parts[:-1]]
    if any(any(word in component for word in UNCERTAIN_WORDS) for component in components):
        return True
    _, tags = terminal_tags(path)
    for tag in tags:
        words = set(re.findall(r"[a-z]+", tag.casefold()))
        if words & UNCERTAIN_WORDS:
            return True
        if re.fullmatch(r"(?:cr|tr|h|b|o|p|f|a)\d*", tag.casefold().strip()):
            return True
    basename = os.path.basename(path).casefold()
    words = "|".join(sorted(UNCERTAIN_WORDS | {"docs?"}))
    return bool(re.search(rf"\b(?:{words})\b", basename))


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def live_snapshot(config: dict) -> dict:
    root = config["root"]
    code = r'''import hashlib, json, os, sys
root = sys.argv[1]
extensions = set(json.loads(sys.argv[2]))
virtual = sys.argv[3] == "1"
games = []
if virtual:
    for category, filename in (("Games", "games.txt"), ("Demos", "demos.txt")):
        path = os.path.join(root, "listings", filename)
        with open(path, "rb") as handle:
            for raw in handle.read().decode("latin-1").splitlines():
                if raw:
                    games.append({"path": category + "/" + raw, "lookup": raw, "category": category})
    for name in os.listdir(root):
        path = os.path.join(root, name)
        if os.path.isfile(path) and name.rsplit(".", 1)[-1].lower() in extensions:
            games.append({"path": name, "lookup": name, "category": ""})
else:
    for current, directories, files in os.walk(root):
        directories[:] = [name for name in directories if name not in ("media", ".removed-duplicates")]
        for name in files:
            if ".bak-artwork-" in name or ".part-artwork-" in name:
                continue
            if name.rsplit(".", 1)[-1].lower() in extensions:
                rel = os.path.relpath(os.path.join(current, name), root).replace(os.sep, "/")
                games.append({"path": rel, "lookup": rel, "category": ""})
images = []
for current, directories, files in os.walk(root):
    for name in files:
        if name.rsplit(".", 1)[-1].lower() in ("png", "jpg", "jpeg", "webp"):
            images.append(os.path.relpath(os.path.join(current, name), root).replace(os.sep, "/"))
print(json.dumps({"games": sorted(games, key=lambda item: item["path"].casefold()), "images": images}))
'''
    output = ssh(
        "python3 - " + shlex.quote(root) + " " + shlex.quote(json.dumps(config["extensions"]))
        + " " + ("1" if config.get("virtual") else "0"),
        input_text=code,
    )
    return json.loads(output)


def existing_digests(root: str, paths: list[str]) -> dict[str, dict]:
    code = r'''import hashlib, json, os, sys
root = sys.argv[1]
paths = PATHS
result = {}
for rel in paths:
    if rel.startswith("/") or ".." in rel.split("/"):
        raise ValueError("unsafe relative image path")
    path = os.path.join(root, rel)
    with open(path, "rb") as handle:
        data = handle.read()
    result[rel] = {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
print(json.dumps(result))
'''
    output = ssh(
        "python3 - " + shlex.quote(root),
        input_text=code.replace("PATHS", repr(paths)),
    )
    return json.loads(output)


def audit(system: str) -> dict:
    config = SYSTEMS[system]
    root = config["root"]
    gamelist = ssh("cat " + shlex.quote(root + "/gamelist.xml"))
    entries, tables = read_gamelist(gamelist)
    snapshot = live_snapshot(config)
    images = {path.casefold(): path for path in snapshot["images"]}
    by_identity: dict[str, set[str]] = collections.defaultdict(set)
    for entry in entries:
        art = (entry.get("art") or "").strip()
        rel = art[2:] if art.startswith("./") else art
        if art and rel.casefold() in images:
            actual = "./" + images[rel.casefold()]
            if entry.get("name"):
                by_identity[identity_key(entry["name"])].add(actual)
    missing = []
    candidates = []
    skipped = []
    unresolved = []
    for item in snapshot["games"]:
        entry, match = degauss_lookup(item["lookup"], tables)
        art = (entry or {}).get("art") or ""
        rel = art[2:] if art.startswith("./") else art
        if entry and art and rel.casefold() in images:
            continue
        record = {**item, "match": match, "title": public_title(item["lookup"])}
        missing.append(record)
        if uncertain(item["path"], item["category"]):
            skipped.append({**record, "reason": "uncertain_variant"})
            continue
        arts = sorted(by_identity.get(identity_key(item["lookup"]), set()))
        if len(arts) == 1:
            candidates.append({**record, "artwork": arts[0]})
        else:
            unresolved.append({**record, "reason": "no_unique_existing_identity", "candidate_count": len(arts)})
    return {
        "system": system,
        "root": root,
        "platform_id": config["platform_id"],
        "gamelist_sha256": sha256(gamelist.encode()),
        "game_count": len(snapshot["games"]),
        "covered_count": len(snapshot["games"]) - len(missing),
        "missing_count": len(missing),
        "local_reuse_count": len(candidates),
        "skipped_count": len(skipped),
        "unresolved_count": len(unresolved),
        "candidates": candidates,
        "skipped": skipped,
        "unresolved": unresolved,
    }


def manifest_for(result: dict) -> dict:
    root = result["root"]
    paths = sorted({item["artwork"][2:] for item in result["candidates"]})
    digests = existing_digests(root, paths)
    operations = []
    for item in result["candidates"]:
        rel = item["artwork"][2:]
        digest = digests[rel]
        operations.append({
            "path": "./" + item["path"],
            "name": item["title"],
            "artwork": item["artwork"],
            "image": {"kind": "existing", "path": item["artwork"], **digest},
            "source": "existing exact same-title card artwork",
        })
    return {
        "version": 1,
        "system": result["system"],
        "root": root,
        "gamelist_sha256": result["gamelist_sha256"],
        "expected_operations": len(operations),
        "operations": operations,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--system", action="append", choices=SYSTEMS, help="repeat for multiple systems")
    parser.add_argument("--output-dir", type=Path, help="write local reports and local-reuse manifests")
    args = parser.parse_args()
    systems = args.system or list(SYSTEMS)
    if args.output_dir:
        args.output_dir.mkdir(parents=True, exist_ok=True)
    for system in systems:
        result = audit(system)
        summary = {key: result[key] for key in (
            "system", "game_count", "covered_count", "missing_count", "local_reuse_count",
            "skipped_count", "unresolved_count",
        )}
        print(json.dumps(summary, ensure_ascii=False))
        if args.output_dir:
            stem = re.sub(r"[^a-z0-9]+", "-", system.casefold()).strip("-")
            (args.output_dir / f"{stem}-audit.json").write_text(
                json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
            )
            if result["candidates"]:
                manifest = manifest_for(result)
                (args.output_dir / f"{stem}-local-reuse-manifest.json").write_text(
                    json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
                )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
