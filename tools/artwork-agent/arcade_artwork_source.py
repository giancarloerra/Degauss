#!/usr/bin/env python3
"""Find candidate gameplay screenshots for the latest Update All Arcade gaps.

This never changes the MiSTer card. Search requests contain only public game
titles and the Arcade platform ID. Candidate images require visual review
before inclusion in an arcade_artwork_audit.py installation manifest.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import unicodedata
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET

import arcade_artwork_audit as arcade


DEFAULT_DEVELOPER_CONFIG = Path(
    os.environ.get(
        "SCREENSCRAPER_DEVELOPER_CONFIG",
        str(Path.home() / ".config" / "degauss-artwork-agent" / "screenscraper-developer.toml"),
    )
)
USER_CONFIG = "/media/fat/Scripts/.config/degauss/screenscraper.toml"
API = "https://api.screenscraper.fr/api2/jeuRecherche.php"
ARCADE_SYSTEM_ID = "75"
LIBRETRO_SNAPS = "https://raw.githubusercontent.com/libretro-thumbnails/mame2010-thumbnail-sources/master/snap/"

TITLE_OVERRIDES = {
    "f1superb": "F1 Super Battle",  # ScreenScraper omits the hyphen.
    "triviasp": "Trivial Pursuit All Star Sports Edition",
    "triviabb": "Trivial Pursuit Baby Boomer Edition",
    "triviag2": "Trivial Pursuit Genus II Edition",
    "triviag1": "Trivial Pursuit Genus Edition",
    "triviag1a": "Trivial Pursuit Genus Edition",
    "triviaes": "Trivial Pursuit Volumen III",
    "triviaes2": "Trivial Pursuit Volumen II",
    "triviayp": "Trivial Pursuit Young Players Edition",
}


def match_key(value: str) -> str:
    value = unicodedata.normalize("NFKD", value)
    return "".join(char.lower() for char in value if char.isascii() and char.isalnum())


def title_for(item: dict) -> str:
    setname = item["setname"]
    return TITLE_OVERRIDES.get(setname, arcade.without_tags(item["name"]))


def grouped_gaps(items: list[dict]) -> list[dict]:
    """Group only same-title sets; different editions retain separate names."""
    by_setname = {}
    for item in items:
        if item["art_valid"] or not item["exists"]:
            continue
        if item["setname"] not in by_setname or not item["path"].startswith("_Organized/"):
            by_setname[item["setname"]] = item
    groups = collections.defaultdict(list)
    for setname, item in sorted(by_setname.items()):
        groups[title_for(item)].append(setname)
    return [{"title": title, "setnames": sorted(setnames)} for title, setnames in sorted(groups.items())]


def credentials(developer_config: Path) -> dict[str, str]:
    with developer_config.open("rb") as handle:
        dev = tomllib.load(handle)
    user = tomllib.loads(arcade.ssh_bytes(f"cat {USER_CONFIG}").decode("utf-8"))
    return {
        "devid": dev["developer_id"],
        "devpassword": dev["developer_password"],
        "ssid": user["username"],
        "sspassword": user["password"],
        "softname": "Degauss",
        "output": "xml",
    }


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, newurl):
        return None


def read_url(url: str, max_bytes: int, authenticated: bool = False) -> bytes:
    try:
        request = urllib.request.Request(url, headers={"User-Agent": "Degauss artwork management"})
        opener = urllib.request.build_opener(NoRedirect()) if authenticated else urllib.request.build_opener()
        with opener.open(request, timeout=25) as response:
            if response.status != 200:
                raise RuntimeError(f"HTTP {response.status}")
            body = response.read(max_bytes + 1)
            if len(body) > max_bytes:
                raise RuntimeError("response exceeded size limit")
            return body
    except urllib.error.HTTPError as error:
        raise RuntimeError(f"HTTP {error.code}") from None
    except urllib.error.URLError as error:
        # urllib exceptions may embed authenticated URLs. Never surface them.
        raise RuntimeError(f"network failure ({type(error.reason).__name__})") from None


def exact_games(root: ET.Element, title: str) -> list[ET.Element]:
    target = match_key(title)
    games = []
    for game in root.findall(".//jeu"):
        names = [(name.text or "").strip() for name in game.findall("./noms/nom")]
        if any(match_key(name) == target for name in names):
            games.append(game)
    return games


def media_candidates(game: ET.Element) -> list[tuple[str, str]]:
    found = []
    for media in game.findall("./medias/media"):
        if media.get("type") != "ss" or not (media.text or "").strip():
            continue
        url = media.text.strip()
        parsed = urllib.parse.urlparse(url)
        host = (parsed.hostname or "").lower()
        if parsed.scheme != "https" or parsed.username or parsed.password or parsed.port not in (None, 443) or not (host == "screenscraper.fr" or host.endswith(".screenscraper.fr")):
            continue
        found.append((media.get("region", "").lower(), url))
    priority = {"wor": 0, "us": 1, "eu": 2, "jp": 3}
    return sorted(found, key=lambda pair: priority.get(pair[0], 4))


def image_format(data: bytes) -> str:
    # Full decode catches truncated/corrupt files, not just a plausible header.
    from PIL import Image
    with Image.open(io.BytesIO(data)) as image:
        image.verify()
        fmt = image.format
    if fmt not in ("PNG", "JPEG"):
        raise ValueError(f"unsupported image format {fmt}")
    return "png" if fmt == "PNG" else "jpg"


def safe_filename(title: str) -> str:
    candidate = re.sub(r"[^a-z0-9]+", "-", unicodedata.normalize("NFKD", title).lower()).strip("-")
    return candidate[:64] or "untitled"


def fetch_group(group: dict, auth: dict, folder: Path) -> dict:
    title = group["title"]
    params = {**auth, "systemeid": ARCADE_SYSTEM_ID, "recherche": title}
    # Never print the URL: it contains authentication parameters.
    body = read_url(API + "?" + urllib.parse.urlencode(params), 8_000_000, authenticated=True)
    try:
        root = ET.fromstring(body)
    except ET.ParseError:
        raise RuntimeError("ScreenScraper returned non-XML or malformed XML") from None
    matches = exact_games(root, title)
    ids = {game.get("id") or game.findtext("id") for game in matches}
    if not matches:
        alternatives = []
        for game in root.findall(".//jeu")[:5]:
            alternatives.extend((name.text or "").strip() for name in game.findall("./noms/nom")[:1])
        return {**group, "status": "no_exact_title", "alternatives": alternatives}
    if len(ids) != 1:
        return {**group, "status": "ambiguous_exact_title", "ids": sorted(str(x) for x in ids)}
    game = matches[0]
    media = media_candidates(game)
    if not media:
        return {**group, "status": "no_screenshot", "game_id": next(iter(ids))}
    region, url = media[0]
    data = read_url(url, 10_000_000)
    if data.strip().upper() == b"NOMEDIA":
        return {**group, "status": "no_screenshot", "game_id": next(iter(ids))}
    fmt = image_format(data)
    destination = folder / f"{safe_filename(title)}-{next(iter(ids))}.{fmt}"
    if destination.exists() and destination.read_bytes() != data:
        raise RuntimeError(f"candidate filename collision: {destination.name}")
    destination.write_bytes(data)
    return {**group, "status": "candidate", "game_id": next(iter(ids)), "region": region,
            "image": str(destination), "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}


def fetch_libretro(group: dict, folder: Path) -> dict:
    """Fallback only after a recorded ScreenScraper miss or ambiguity."""
    for setname in group["setnames"]:
        if not re.fullmatch(r"[A-Za-z0-9_.-]+", setname):
            raise ValueError(f"Unsafe public setname: {setname!r}")
        url = LIBRETRO_SNAPS + setname + ".png"
        try:
            data = read_url(url, 10_000_000)
        except RuntimeError as error:
            if str(error) == "HTTP 404":
                continue
            raise
        fmt = image_format(data)
        if fmt != "png":
            raise ValueError(f"Libretro snapshot is not PNG for {setname}")
        destination = folder / f"libretro-{setname}.png"
        if destination.exists() and destination.read_bytes() != data:
            raise RuntimeError(f"Libretro candidate filename collision: {destination.name}")
        destination.write_bytes(data)
        return {**group, "status": "candidate", "setname_source": setname,
                "image": str(destination), "sha256": hashlib.sha256(data).hexdigest(),
                "bytes": len(data), "source": f"Libretro MAME 2010 snap/{setname}.png"}
    return {**group, "status": "no_libretro_setname_snapshot"}


def prepare_manifest(groups: list[dict], items: list[dict], log: bytes, gamelist: bytes,
                     candidates: list[dict], approvals: dict) -> dict:
    by_title = {record["title"]: record for record in candidates}
    missing = collections.Counter(
        item["setname"] for item in items if item["match"] == "none" and not item["art_valid"]
    )
    manifest_groups = []
    for group in groups:
        title = group["title"]
        if title not in approvals:
            continue
        approval = approvals[title]
        record = by_title.get(title)
        if isinstance(approval, str):
            if not record or record["status"] != "candidate":
                raise ValueError(f"No reviewed candidate for {title}")
            image, expected_hash = record["image"], approval
            source = record.get("source") or f"ScreenScraper Arcade game {record['game_id']}"
            target = f"media/screenshot/artwork-{group['setnames'][0]}.{Path(image).suffix.lstrip('.')}"
        elif isinstance(approval, dict):
            image, expected_hash, target = approval["image"], approval["sha256"], approval["target"]
            source = approval["source"]
        else:
            raise ValueError(f"Invalid approval for {title}")
        if hashlib.sha256(Path(image).read_bytes()).hexdigest() != expected_hash:
            raise ValueError(f"Reviewed image changed for {title}")
        count = sum(missing[setname] for setname in group["setnames"])
        if count < 1:
            raise ValueError(f"No missing paths remain for {title}")
        manifest_groups.append({"setnames": group["setnames"], "image": image,
                                "sha256": expected_hash, "target": target,
                                "source": source, "expected_missing_paths": count})
    if not manifest_groups:
        raise ValueError("No reviewed image group is eligible")
    return {"log_sha256": arcade.sha256(log), "gamelist_sha256": arcade.sha256(gamelist),
            "groups": manifest_groups,
            "expected_total_new_entries": sum(group["expected_missing_paths"] for group in manifest_groups)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--folder", required=True, help="local output directory")
    parser.add_argument(
        "--developer-config",
        type=Path,
        default=DEFAULT_DEVELOPER_CONFIG,
        help="protected TOML containing developer_id and developer_password",
    )
    parser.add_argument("--fetch", action="store_true", help="contact ScreenScraper and save exact-title candidates")
    parser.add_argument("--libretro", action="store_true", help="try exact-setname Libretro snaps only for recorded ScreenScraper exceptions")
    parser.add_argument("--limit", type=int, help="process at most this many new titles")
    parser.add_argument("--approved", help="JSON title-to-reviewed-SHA map; requires --manifest-output")
    parser.add_argument("--manifest-output", help="write a pinned installation manifest for only reviewed images")
    args = parser.parse_args()
    if bool(args.approved) != bool(args.manifest_output):
        parser.error("--approved and --manifest-output are required together")
    folder = Path(args.folder).resolve()
    log = arcade.ssh_bytes(f"cat {arcade.LOG}")
    gamelist = arcade.ssh_bytes(f"cat {arcade.ARCADE}/gamelist.xml")
    result = arcade.audit(log.decode(), gamelist.decode())
    groups = grouped_gaps(result["items"])
    print(f"{len(groups)} distinct public titles covering {sum(len(x['setnames']) for x in groups)} setnames")
    if args.approved:
        cache = folder / "candidates.json"
        candidates = json.loads(cache.read_text()) if cache.exists() else []
        fallback_cache = folder / "libretro_candidates.json"
        if fallback_cache.exists():
            candidates.extend(json.loads(fallback_cache.read_text()))
        approvals = json.loads(Path(args.approved).read_text())
        manifest = prepare_manifest(groups, result["items"], log, gamelist, candidates, approvals)
        Path(args.manifest_output).write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
        print(f"Prepared {len(manifest['groups'])} reviewed image groups for {manifest['expected_total_new_entries']} exact paths")
        return 0
    if args.libretro:
        folder.mkdir(parents=True, exist_ok=True)
        screenscraper_cache = folder / "candidates.json"
        if not screenscraper_cache.exists():
            raise ValueError("Run and record ScreenScraper searches before Libretro fallback")
        by_title = {record["title"]: record for record in json.loads(screenscraper_cache.read_text())}
        cache = folder / "libretro_candidates.json"
        completed = {record["title"]: record for record in json.loads(cache.read_text())} if cache.exists() else {}
        processed = 0
        for group in groups:
            title = group["title"]
            prior = by_title.get(title)
            if title in completed or not prior or prior["status"] == "candidate":
                continue
            if args.limit is not None and processed >= args.limit:
                break
            record = fetch_libretro(group, folder)
            record["screenscraper_status"] = prior["status"]
            completed[title] = record
            cache.write_text(json.dumps(list(completed.values()), ensure_ascii=False, indent=2) + "\n")
            print(f"{title}: {record['status']}", flush=True)
            processed += 1
        print("Libretro inventory:", collections.Counter(item["status"] for item in completed.values()))
        return 0
    if not args.fetch:
        for group in groups:
            print(f"{group['title']}\t{','.join(group['setnames'])}")
        return 0
    folder.mkdir(parents=True, exist_ok=True)
    cache = folder / "candidates.json"
    old = json.loads(cache.read_text()) if cache.exists() else []
    completed = {record["title"]: record for record in old}
    auth = credentials(args.developer_config)
    processed = 0
    for group in groups:
        title = group["title"]
        if title in completed:
            continue
        if args.limit is not None and processed >= args.limit:
            break
        record = fetch_group(group, auth, folder)
        completed[title] = record
        cache.write_text(json.dumps(list(completed.values()), ensure_ascii=False, indent=2) + "\n")
        print(f"{title}: {record['status']}", flush=True)
        processed += 1
    print("Candidate inventory:", collections.Counter(item["status"] for item in completed.values()))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, ET.ParseError) as error:
        print(f"arcade artwork source failed: {error}", file=sys.stderr)
        sys.exit(1)
