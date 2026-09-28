#!/usr/bin/env python3
"""Fetch and prepare exact-title artwork for audited Degauss systems.

ScreenScraper credentials are read in process and never written to output.
The default mode only reports work. ``--fetch`` saves exact-title candidates
and a resumable local cache. ``--manifest`` emits a pinned installer manifest
from candidate records after their contact sheets have been reviewed.
"""

from __future__ import annotations

import argparse
import collections
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import unicodedata
import urllib.parse

from PIL import Image, ImageDraw, ImageFont

import arcade_artwork_source as source
from system_artwork_audit import SYSTEMS, identity_key


DEFAULT_DEVELOPER_CONFIG = Path(
    os.environ.get(
        "SCREENSCRAPER_DEVELOPER_CONFIG",
        str(Path.home() / ".config" / "degauss-artwork-agent" / "screenscraper-developer.toml"),
    )
)

LIBRETRO_REPOSITORIES = {
    "Amiga": "Commodore_-_Amiga",
    "Commodore 64": "Commodore_-_64",
    "Genesis": "Sega_-_Mega_Drive_-_Genesis",
    "SNES": "Nintendo_-_Super_Nintendo_Entertainment_System",
    "Nintendo 64": "Nintendo_-_Nintendo_64",
}
LIBRETRO_INVALID = str.maketrans({character: "_" for character in '&*/:`<>?\\|"'})
LIBRETRO_FALLBACK_STATUSES = {"no_exact_title", "no_screenshot", "ambiguous_exact_title"}


def groups_for(audit: dict) -> list[dict]:
    grouped: dict[str, list[dict]] = collections.defaultdict(list)
    for item in audit["unresolved"]:
        grouped[identity_key(item["title"])].append(item)
    groups = []
    for _, items in sorted(grouped.items()):
        titles = collections.Counter(item["title"] for item in items)
        title = sorted(titles, key=lambda value: (-titles[value], len(value), value.casefold()))[0]
        groups.append({
            "title": title,
            "paths": sorted({item["path"] for item in items}),
            "setnames": [],
        })
    return groups


def atomic_json(path: Path, value) -> None:
    temporary = path.with_name(path.name + ".part-" + str(os.getpid()))
    temporary.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def fetch_system(audit: dict, output: Path, developer_config: Path, limit: int | None, workers: int) -> list[dict]:
    system = audit["system"]
    groups = groups_for(audit)
    cache_path = output / (re.sub(r"[^a-z0-9]+", "-", system.casefold()).strip("-") + "-screenscraper.json")
    records = json.loads(cache_path.read_text(encoding="utf-8")) if cache_path.exists() else []
    by_key = {identity_key(record["title"]): record for record in records}
    pending = [
        group for group in groups
        if identity_key(group["title"]) not in by_key
        or by_key[identity_key(group["title"])].get("status") == "error"
    ]
    if limit is not None:
        pending = pending[:limit]
    print(f"{system}: {len(groups)} title identities, {len(pending)} pending", flush=True)
    if not pending:
        return records
    auth = source.credentials(developer_config)
    source.ARCADE_SYSTEM_ID = audit["platform_id"]
    system_output = output / re.sub(r"[^a-z0-9]+", "-", system.casefold()).strip("-")
    system_output.mkdir(parents=True, exist_ok=True)

    stopped = False
    completed = 0
    iterator = iter(pending)
    active: dict[Future, dict] = {}
    with ThreadPoolExecutor(max_workers=workers) as executor:
        for _ in range(workers):
            try:
                group = next(iterator)
            except StopIteration:
                break
            active[executor.submit(source.fetch_group, group, auth, system_output)] = group
        while active:
            done, _ = wait(active, return_when=FIRST_COMPLETED)
            for future in done:
                group = active.pop(future)
                try:
                    record = future.result()
                except Exception as error:
                    message = str(error)
                    record = {**group, "status": "error", "error": f"{type(error).__name__}: {message}"}
                    if message.startswith("HTTP 429") or message.startswith("HTTP 430") or "network failure" in message:
                        stopped = True
                by_key[identity_key(group["title"])] = record
                completed += 1
                atomic_json(cache_path, sorted(by_key.values(), key=lambda item: item["title"].casefold()))
                if completed % 20 == 0 or record["status"] == "error":
                    counts = collections.Counter(item["status"] for item in by_key.values())
                    print(f"{system}: {completed}/{len(pending)} new, statuses {dict(sorted(counts.items()))}", flush=True)
            if stopped:
                for future in active:
                    future.cancel()
                break
            while len(active) < workers:
                try:
                    group = next(iterator)
                except StopIteration:
                    break
                active[executor.submit(source.fetch_group, group, auth, system_output)] = group
    if stopped:
        raise RuntimeError(f"{system}: source retrieval stopped after a network or rate-limit error")
    return sorted(by_key.values(), key=lambda item: item["title"].casefold())


def contact_sheets(records: list[dict], output: Path, system: str, per_sheet: int = 40) -> list[Path]:
    candidates = [record for record in records if record.get("status") == "candidate"]
    sheets = []
    cell_w, cell_h = 240, 210
    columns = 4
    font = ImageFont.load_default()
    safe_system = re.sub(r"[^a-z0-9]+", "-", system.casefold()).strip("-")
    for page, start in enumerate(range(0, len(candidates), per_sheet), start=1):
        batch = candidates[start:start + per_sheet]
        rows = (len(batch) + columns - 1) // columns
        sheet = Image.new("RGB", (columns * cell_w, rows * cell_h), "white")
        draw = ImageDraw.Draw(sheet)
        for index, record in enumerate(batch):
            image = Image.open(record["image"]).convert("RGB")
            image.thumbnail((cell_w - 12, cell_h - 48))
            x = (index % columns) * cell_w + (cell_w - image.width) // 2
            y = (index // columns) * cell_h + 4
            sheet.paste(image, (x, y))
            label = f"{start + index + 1}. {record['title']}"
            draw.text(((index % columns) * cell_w + 5, (index // columns) * cell_h + cell_h - 40), label[:38], fill="black", font=font)
            draw.text(((index % columns) * cell_w + 5, (index // columns) * cell_h + cell_h - 25), label[38:76], fill="black", font=font)
        path = output / f"{safe_system}-contact-{page:03d}.jpg"
        sheet.save(path, quality=90)
        sheets.append(path)
    return sheets


def manifest_for(audit: dict, records: list[dict]) -> dict:
    groups = {identity_key(group["title"]): group for group in groups_for(audit)}
    operations = []
    for record in records:
        if record.get("status") != "candidate":
            continue
        group = groups.get(identity_key(record["title"]))
        if group is None:
            raise ValueError(f"Candidate no longer belongs to the audit: {record['title']}")
        image = Path(record["image"])
        data = image.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        if digest != record["sha256"] or len(data) != record["bytes"]:
            raise ValueError(f"Candidate image changed: {image}")
        extension = image.suffix.casefold().lstrip(".")
        target = "./media/screenshot/sourced-" + source.safe_filename(record["title"]) + "-" + str(record["game_id"]) + "." + extension
        descriptor = {"kind": "local", "path": str(image), "size": len(data), "sha256": digest}
        for path in group["paths"]:
            operations.append({
                "path": "./" + path,
                "name": record["title"],
                "artwork": target,
                "image": descriptor,
                "source": f"ScreenScraper {audit['system']} game {record['game_id']}",
            })
    return {
        "version": 1,
        "system": audit["system"],
        "root": audit["root"],
        "gamelist_sha256": audit["gamelist_sha256"],
        "expected_operations": len(operations),
        "operations": operations,
    }


def libretro_filename_key(value: str) -> str:
    return unicodedata.normalize("NFC", value.translate(LIBRETRO_INVALID)).casefold()


def game_stem(path: str, system: str) -> str:
    basename = os.path.basename(path)
    stem, extension = os.path.splitext(basename)
    if extension[1:].casefold() in SYSTEMS[system]["extensions"]:
        return stem
    return basename


def libretro_title(stem: str, system: str) -> str:
    if system == "Commodore 64":
        match = re.search(r"\s*\((?:19|20)\d{2}\)", stem)
        if match:
            stem = stem[:match.start()]
        return stem.strip()
    from system_artwork_audit import public_title
    return public_title(stem)


def tree_json_inventory(path: Path) -> tuple[str, list[dict]]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if value.get("truncated") is not False:
        raise ValueError(f"Libretro tree is incomplete: {path}")
    commit = value.get("sha", "")
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError(f"Libretro tree has no pinned commit: {path}")
    entries = [
        {"path": entry["path"], "blob_sha1": entry["sha"]}
        for entry in value.get("tree", [])
        if entry.get("type") == "blob"
        and entry.get("path", "").startswith("Named_Snaps/")
        and entry.get("path", "").casefold().endswith(".png")
    ]
    return commit, entries


def git_inventory(path: Path) -> tuple[str, list[dict]]:
    commit = subprocess.run(
        ["git", "-C", str(path), "rev-parse", "HEAD"],
        capture_output=True, check=True, text=True,
    ).stdout.strip()
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError(f"Libretro checkout has no pinned commit: {path}")
    raw = subprocess.run(
        ["git", "-C", str(path), "ls-tree", "-rz", "HEAD", "--", "Named_Snaps"],
        capture_output=True, check=True,
    ).stdout
    entries = []
    for record in raw.split(b"\0"):
        if not record:
            continue
        metadata, encoded_path = record.split(b"\t", 1)
        fields = metadata.split()
        tree_path = encoded_path.decode("utf-8", "surrogateescape")
        if fields[1] == b"blob" and tree_path.casefold().endswith(".png"):
            entries.append({"path": tree_path, "blob_sha1": fields[2].decode("ascii")})
    return commit, entries


def load_libretro_inventory(system: str, config: dict) -> tuple[str, str, list[dict]]:
    expected_repository = LIBRETRO_REPOSITORIES[system]
    repository = config.get("repository")
    if repository != expected_repository:
        raise ValueError(f"Unexpected Libretro repository for {system}: {repository!r}")
    methods = [key for key in ("tree_json", "git_repository") if config.get(key)]
    if len(methods) != 1:
        raise ValueError(f"{system} requires exactly one complete Libretro index")
    if methods[0] == "tree_json":
        commit, entries = tree_json_inventory(Path(config["tree_json"]))
    else:
        commit, entries = git_inventory(Path(config["git_repository"]))
    if not entries:
        raise ValueError(f"Libretro index has no Named_Snaps for {system}")
    return repository, commit, entries


def select_libretro_candidates(audit: dict, screen_records: list[dict], entries: list[dict]) -> tuple[list[dict], dict]:
    system = audit["system"]
    prior = {identity_key(record["title"]): record for record in screen_records}
    by_filename: dict[str, list[dict]] = collections.defaultdict(list)
    by_identity: dict[str, list[dict]] = collections.defaultdict(list)
    for entry in entries:
        stem = Path(entry["path"]).name[:-4]
        by_filename[libretro_filename_key(stem)].append(entry)
        by_identity[identity_key(libretro_title(stem, system))].append(entry)
    selected: dict[str, dict] = {}
    summary = collections.Counter()
    for item in audit["unresolved"]:
        key = identity_key(item["title"])
        previous = prior.get(key)
        if previous is None:
            summary["no_recorded_screenscraper_result"] += 1
            continue
        status = previous.get("status")
        if status not in LIBRETRO_FALLBACK_STATUSES:
            summary["ineligible_screenscraper_status"] += 1
            continue
        matches = by_filename.get(libretro_filename_key(game_stem(item["path"], system)), [])
        match_kind = "exact_rom_filename"
        if not matches:
            matches = by_identity.get(key, [])
            match_kind = "unique_public_identity"
        unique_blobs = {entry["blob_sha1"] for entry in matches}
        if not matches:
            summary["no_libretro_match"] += 1
            continue
        if len(unique_blobs) != 1:
            summary["ambiguous_libretro_match"] += 1
            continue
        blob_sha1 = next(iter(unique_blobs))
        representative = sorted(
            (entry for entry in matches if entry["blob_sha1"] == blob_sha1),
            key=lambda entry: (len(entry["path"]), entry["path"].casefold()),
        )[0]
        record = selected.setdefault(blob_sha1, {
            "status": "selected",
            "blob_sha1": blob_sha1,
            "tree_path": representative["path"],
            "tree_paths": sorted({entry["path"] for entry in matches if entry["blob_sha1"] == blob_sha1}),
            "operations": [],
        })
        record["operations"].append({
            "path": item["path"],
            "title": item["title"],
            "match_kind": match_kind,
            "screenscraper_status": status,
        })
        summary[match_kind] += 1
    candidates = sorted(selected.values(), key=lambda record: record["tree_path"].casefold())
    for record in candidates:
        record["operations"] = sorted(record["operations"], key=lambda item: item["path"].casefold())
        titles = sorted({item["title"] for item in record["operations"]}, key=str.casefold)
        record["title"] = titles[0] + (f" (+{len(titles) - 1})" if len(titles) > 1 else "")
    return candidates, dict(sorted(summary.items()))


def git_blob_sha1(data: bytes) -> str:
    return hashlib.sha1(f"blob {len(data)}\0".encode("ascii") + data).hexdigest()


def fetch_libretro_record(record: dict, repository: str, commit: str, folder: Path) -> dict:
    tree_path = record["tree_path"]
    quoted_path = urllib.parse.quote(tree_path, safe="/")
    url = f"https://raw.githubusercontent.com/libretro-thumbnails/{repository}/{commit}/{quoted_path}"
    data = source.read_url(url, 10_000_000)
    if source.image_format(data) != "png":
        raise ValueError(f"Libretro snapshot is not PNG: {tree_path}")
    if git_blob_sha1(data) != record["blob_sha1"]:
        raise ValueError(f"Libretro snapshot differs from pinned tree: {tree_path}")
    destination = folder / (
        "libretro-" + source.safe_filename(record["operations"][0]["title"])
        + "-" + record["blob_sha1"][:12] + ".png"
    )
    if destination.exists() and destination.read_bytes() != data:
        raise RuntimeError(f"candidate filename collision: {destination.name}")
    destination.write_bytes(data)
    return {
        **record,
        "status": "candidate",
        "image": str(destination),
        "sha256": hashlib.sha256(data).hexdigest(),
        "bytes": len(data),
        "repository": repository,
        "commit": commit,
    }


def fetch_libretro_system(audit: dict, screen_records: list[dict], config: dict, output: Path, workers: int) -> dict:
    system = audit["system"]
    repository, commit, entries = load_libretro_inventory(system, config)
    candidates, summary = select_libretro_candidates(audit, screen_records, entries)
    stem = re.sub(r"[^a-z0-9]+", "-", system.casefold()).strip("-")
    cache_path = output / f"{stem}-libretro.json"
    folder = output / f"{stem}-libretro"
    folder.mkdir(parents=True, exist_ok=True)
    prior_cache = json.loads(cache_path.read_text(encoding="utf-8")) if cache_path.exists() else {}
    if prior_cache and (prior_cache.get("repository"), prior_cache.get("commit")) != (repository, commit):
        raise ValueError(f"Pinned Libretro source changed for {system}")
    cached = {record["blob_sha1"]: record for record in prior_cache.get("records", [])}
    pending = [record for record in candidates if cached.get(record["blob_sha1"], {}).get("status") != "candidate"]
    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = {
            executor.submit(fetch_libretro_record, record, repository, commit, folder): record
            for record in pending
        }
        completed = 0
        for future in futures:
            record = future.result()
            cached[record["blob_sha1"]] = record
            completed += 1
            value = {
                "system": system, "repository": repository, "commit": commit,
                "summary": summary,
                "records": sorted(cached.values(), key=lambda item: item["tree_path"].casefold()),
            }
            atomic_json(cache_path, value)
            if completed % 20 == 0:
                print(f"{system}: downloaded {completed}/{len(pending)} Libretro snapshots", flush=True)
    current_blobs = {record["blob_sha1"] for record in candidates}
    value = {
        "system": system, "repository": repository, "commit": commit, "summary": summary,
        "records": sorted(
            (record for blob, record in cached.items() if blob in current_blobs),
            key=lambda item: item["tree_path"].casefold(),
        ),
    }
    atomic_json(cache_path, value)
    return value


def libretro_manifest(audit: dict, value: dict, review: dict) -> dict:
    available = {record["blob_sha1"] for record in value["records"] if record.get("status") == "candidate"}
    approved = set(review.get("approved_blob_sha1", []))
    rejected = set(review.get("rejected", {}))
    if approved & rejected:
        raise ValueError("A Libretro blob cannot be both approved and rejected")
    if approved | rejected != available:
        raise ValueError("Libretro visual review does not account for every current candidate")
    if any(not str(reason).strip() for reason in review.get("rejected", {}).values()):
        raise ValueError("Every rejected Libretro blob requires a reason")
    operations = []
    seen = set()
    for record in value["records"]:
        if record.get("status") != "candidate":
            continue
        if record["blob_sha1"] not in approved:
            continue
        image = Path(record["image"])
        data = image.read_bytes()
        if len(data) != record["bytes"] or hashlib.sha256(data).hexdigest() != record["sha256"]:
            raise ValueError(f"Libretro candidate changed: {image}")
        if git_blob_sha1(data) != record["blob_sha1"]:
            raise ValueError(f"Libretro candidate no longer matches its pinned blob: {image}")
        target = (
            "./media/screenshot/libretro-" + source.safe_filename(record["operations"][0]["title"])
            + "-" + record["blob_sha1"][:12] + ".png"
        )
        descriptor = {"kind": "local", "path": str(image), "size": len(data), "sha256": record["sha256"]}
        provenance = (
            f"Libretro {value['repository']} Named_Snaps at {value['commit']}: {record['tree_path']}"
        )
        for item in record["operations"]:
            path = "./" + item["path"]
            if path in seen:
                raise ValueError(f"Duplicate Libretro operation: {path}")
            seen.add(path)
            operations.append({
                "path": path, "name": item["title"], "artwork": target,
                "image": descriptor, "source": provenance,
            })
    return {
        "version": 1, "system": audit["system"], "root": audit["root"],
        "gamelist_sha256": audit["gamelist_sha256"],
        "expected_operations": len(operations), "operations": operations,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--audits", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--system", action="append", choices=SYSTEMS)
    parser.add_argument("--developer-config", type=Path, default=DEFAULT_DEVELOPER_CONFIG)
    parser.add_argument("--fetch", action="store_true")
    parser.add_argument("--limit", type=int)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--contact-sheets", action="store_true")
    parser.add_argument("--manifest", action="store_true")
    parser.add_argument("--libretro-config", type=Path)
    parser.add_argument("--libretro-review", type=Path)
    args = parser.parse_args()
    if args.workers < 1 or args.workers > 8:
        parser.error("--workers must be between 1 and 8")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    selected = set(args.system or SYSTEMS)
    audit_files = sorted(args.audits.glob("*-audit.json"))
    found = set()
    for path in audit_files:
        audit = json.loads(path.read_text(encoding="utf-8"))
        system = audit["system"]
        if system not in selected:
            continue
        found.add(system)
        stem = re.sub(r"[^a-z0-9]+", "-", system.casefold()).strip("-")
        if args.libretro_config:
            config = json.loads(args.libretro_config.read_text(encoding="utf-8"))
            if system not in config:
                raise ValueError(f"Missing Libretro source configuration for {system}")
            screen_cache = args.output_dir / f"{stem}-screenscraper.json"
            if not screen_cache.exists():
                raise ValueError(f"No recorded ScreenScraper results for {system}")
            screen_records = json.loads(screen_cache.read_text(encoding="utf-8"))
            libretro_cache = args.output_dir / f"{stem}-libretro.json"
            if args.fetch:
                value = fetch_libretro_system(
                    audit, screen_records, config[system], args.output_dir, args.workers,
                )
            elif libretro_cache.exists():
                value = json.loads(libretro_cache.read_text(encoding="utf-8"))
            else:
                raise ValueError(f"No Libretro candidate cache for {system}")
            print(f"{system}: Libretro {value['summary']}", flush=True)
            if args.contact_sheets:
                for sheet in contact_sheets(value["records"], args.output_dir, system + " Libretro"):
                    print(sheet)
            if args.manifest:
                if not args.libretro_review:
                    raise ValueError("--libretro-review is required for a Libretro manifest")
                review = json.loads(args.libretro_review.read_text(encoding="utf-8"))
                manifest = libretro_manifest(audit, value, review)
                if not manifest["operations"]:
                    print(f"{system}: no Libretro candidate operations", file=sys.stderr)
                else:
                    atomic_json(args.output_dir / f"{stem}-libretro-manifest.json", manifest)
            continue
        cache = args.output_dir / f"{stem}-screenscraper.json"
        records = fetch_system(audit, args.output_dir, args.developer_config, args.limit, args.workers) if args.fetch else (
            json.loads(cache.read_text(encoding="utf-8")) if cache.exists() else []
        )
        counts = collections.Counter(record["status"] for record in records)
        print(f"{system}: {dict(sorted(counts.items()))}", flush=True)
        if args.contact_sheets:
            for sheet in contact_sheets(records, args.output_dir, system):
                print(sheet)
        if args.manifest:
            manifest = manifest_for(audit, records)
            if not manifest["operations"]:
                print(f"{system}: no candidate operations", file=sys.stderr)
            else:
                atomic_json(args.output_dir / f"{stem}-screenscraper-manifest.json", manifest)
    missing = selected - found
    if missing:
        raise ValueError("Missing audit files for: " + ", ".join(sorted(missing)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
