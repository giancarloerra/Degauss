#!/usr/bin/env python3
"""Write the Downloader database that installs and updates Degauss.

MiSTer's Downloader reads a database of files, each with its hash, size and
where to fetch it. Adding one line to `downloader.ini` therefore makes
`update_all` keep Degauss current alongside everything else on the card.

A database can only place files. It cannot edit `MiSTer.ini`, so the `main=`
line stays a one-off by hand, and it cannot run anything after installing.

Usage: make-db.py <tag> <deploy-dir> <mister-degauss-binary> <out.json>
"""

import hashlib
import json
import os
import sys
import urllib.parse
import time

OWNER = "giancarloerra"
REPO = "Degauss"

# Files that ship as release assets because they are built, not committed.
RELEASE_ASSETS = {"Scripts/.degauss/degauss", "degauss/MiSTer_Degauss"}


def md5(path):
    digest = hashlib.md5()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def quoted(path):
    """Percent-encode a path for a URL, leaving the separators alone.

    Filenames on a card carry spaces, ampersands and hashes. Left raw in a
    URL those either break the request or silently truncate it at the hash.
    """
    return urllib.parse.quote(path, safe="/")


def url_for(card_path, tag):
    """Where Downloader fetches one file from.

    Built binaries come from the release assets for the tag. Everything else
    is committed, so it comes from the repository at that same tag: one
    source of truth per file, and no release asset per logo.
    """
    if card_path in RELEASE_ASSETS:
        name = os.path.basename(card_path)
        return f"https://github.com/{OWNER}/{REPO}/releases/download/{quoted(tag)}/{quoted(name)}"
    if card_path.startswith("Scripts/.degauss/logos/"):
        name = os.path.basename(card_path)
        return f"https://raw.githubusercontent.com/{OWNER}/{REPO}/{quoted(tag)}/assets/logos/{quoted(name)}"
    known = REPO_SOURCED
    if card_path not in known:
        # Loudly, because a file nobody can fetch would install as a silent
        # gap on somebody's card.
        raise SystemExit(f"make-db: no source known for {card_path!r}")
    return f"https://raw.githubusercontent.com/{OWNER}/{REPO}/{quoted(tag)}/{quoted(known[card_path])}"


# For files served from the repository, the hash has to be of the repository
# file, not of the copy the build made into deploy/. They are the same today,
# but nothing enforces that, and a database whose hash does not match what the
# URL serves makes Downloader fetch the same file forever.
REPO_SOURCED = {
    "Scripts/degauss.sh": "deploy/Scripts/degauss.sh",
    "Scripts/.degauss/degauss.toml": "degauss.toml",
    "Scripts/.degauss/systems.toml": "assets/systems.toml",
    "Scripts/.degauss/LICENSE": "LICENSE",
    "Scripts/.degauss/DejaVuSans-LICENSE.txt": "assets/fonts/DejaVuSans-LICENSE.txt",
    "Scripts/.degauss/Px437-LICENSE.txt": "assets/fonts/Px437-LICENSE.txt",
    "Scripts/.degauss/RobotoCondensed-LICENSE.txt": "assets/fonts/RobotoCondensed-LICENSE.txt",
    "Scripts/.degauss/Tamzen-LICENSE.txt": "assets/fonts/Tamzen-LICENSE.txt",
}


def main():
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    tag, deploy, fork_binary, out = sys.argv[1:]

    files = {}
    folders = {}

    def add(card_path, real_path):
        # Hash the file the URL will serve, not the staged copy of it.
        if card_path in REPO_SOURCED:
            real_path = REPO_SOURCED[card_path]
        elif card_path.startswith("Scripts/.degauss/logos/"):
            real_path = os.path.join("assets/logos", os.path.basename(card_path))
        files[card_path] = {
            "hash": md5(real_path),
            "size": os.path.getsize(real_path),
            "url": url_for(card_path, tag),
        }
        folder = os.path.dirname(card_path)
        while folder:
            folders[folder] = {}
            folder = os.path.dirname(folder)

    for root, _, names in os.walk(os.path.join(deploy, "Scripts")):
        for name in sorted(names):
            real = os.path.join(root, name)
            card = os.path.relpath(real, deploy).replace(os.sep, "/")
            add(card, real)

    add("degauss/MiSTer_Degauss", fork_binary)

    database = {
        "v": 1,
        "db_id": "degauss",
        "timestamp": int(time.time()),
        "files": files,
        "folders": folders,
    }
    with open(out, "w", encoding="utf-8") as handle:
        json.dump(database, handle, indent=1, sort_keys=True)
    print(f"{out}: {len(files)} files, {len(folders)} folders, tag {tag}")


if __name__ == "__main__":
    main()
