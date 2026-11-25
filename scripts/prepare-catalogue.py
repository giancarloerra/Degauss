"""Prepare the metadata for an independently reviewed catalogue tag. Never pushes."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import tomllib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--revision", type=int, required=True)
    parser.add_argument("--minimum", required=True, help="Oldest frontend/Main pair verified to launch the affected media")
    parser.add_argument("--summary", required=True)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    if (args.revision < 1 or not re.fullmatch(r"\d+\.\d+\.\d+", args.minimum)
            or not 0 < len(args.summary.strip()) <= 160 or not args.summary.isprintable()):
        parser.error("Expected a positive revision, x.y.z minimum and a short summary")
    body = (args.root / "assets/systems.toml").read_bytes()
    if (args.root / "deploy/Scripts/.config/degauss/systems.toml").read_bytes() != body:
        parser.error("The release and Update All catalogues differ")
    systems = tomllib.loads(body.decode())["systems"]
    if not systems or len({s["id"] for s in systems}) != len(systems) or len(body) > 4 * 1024 * 1024:
        parser.error("Empty, duplicate or oversized catalogue")
    meta = dict(revision=args.revision, format=1, min_app=args.minimum, bytes=len(body),
                sha256=hashlib.sha256(body).hexdigest(), summary=args.summary.strip())
    dest = args.root / "catalogue/revision.json"
    dest.parent.mkdir(exist_ok=True)
    dest.write_text(json.dumps(meta, indent=2) + "\n", encoding="utf-8")
    print("Prepared", dest, "for lightweight tag", f"catalogue-v1-{args.revision}")


if __name__ == "__main__":
    main()
