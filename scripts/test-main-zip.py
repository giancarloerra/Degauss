#!/usr/bin/env python3
"""Compare Degauss ZIP listing with Main's actual ZIP iterator, entirely locally.

Usage: python3 scripts/test-main-zip.py /path/to/Main_MiSTer/lib/miniz
The source must already be downloaded at MAIN_REVISION. No network calls,
external services, card access, game collection or additional Python packages.
"""

import argparse
import hashlib
import json
import pathlib
import shutil
import struct
import subprocess
import tempfile
import zipfile
import zlib

MAIN_REVISION = "f8dc68e3dcf4694f5593e6552aea56cd852982af"
SOURCE_HASHES = {
    "miniz.c": "bd2a5ae69adc3bc7d73fa37bf65dbabe84846758446a26009a827ff5d69a9c9c",
    "miniz.h": "e9bc0ad11e0d67283b7fb254d907e7011509560f50072840d3c10973c2855795",
}

C_HARNESS = r'''
#include "miniz.h"
#include <stdio.h>
int main(int argc, char **argv) {
    if (argc != 2) return 10;
    mz_zip_archive archive = {0};
    if (!mz_zip_reader_init_file(&archive, argv[1], 0)) return 1;
    for (mz_uint n = 0; n < mz_zip_reader_get_num_files(&archive); ++n) {
        mz_zip_archive_file_stat st;
        if (!mz_zip_reader_file_stat(&archive, n, &st)) return 2;
        if (st.m_is_directory) continue;
        if (mz_zip_reader_locate_file(&archive, st.m_filename, NULL, 0) != (int)n) return 3;
        mz_zip_reader_extract_iter_state *iter = mz_zip_reader_extract_iter_new(&archive, n, 0);
        if (!iter) return 4;
        unsigned char block[4096];
        size_t got;
        mz_uint64 length = 0;
        mz_ulong crc = 0;
        while ((got = mz_zip_reader_extract_iter_read(iter, block, sizeof(block)))) {
            length += got;
            crc = mz_crc32(crc, block, got);
        }
        if (!mz_zip_reader_extract_iter_free(iter)) return 5;
        if (length != st.m_uncomp_size || crc != st.m_crc32) return 6;
        printf("%s\t%llu\t%08x\n", st.m_filename, (unsigned long long)length, (unsigned)crc);
    }
    mz_zip_reader_end(&archive);
    return 0;
}
'''


def forced_zip64(original):
    """Keep real compressed payloads; replace central/end records with ZIP64."""
    eocd = original.rfind(b"PK\x05\x06")
    count = struct.unpack_from("<H", original, eocd + 10)[0]
    offset = struct.unpack_from("<I", original, eocd + 16)[0]
    central = bytearray()
    cursor = offset
    for _ in range(count):
        header = bytearray(original[cursor:cursor + 46])
        name_length, extra_length, comment_length = struct.unpack_from("<HHH", header, 28)
        compressed, uncompressed = struct.unpack_from("<II", header, 20)
        local = struct.unpack_from("<I", header, 42)[0]
        if extra_length or comment_length:
            raise ValueError("fixture unexpectedly has central extra fields or comments")
        struct.pack_into("<H", header, 6, 45)
        struct.pack_into("<II", header, 20, 0xFFFFFFFF, 0xFFFFFFFF)
        struct.pack_into("<H", header, 30, 28)
        struct.pack_into("<I", header, 42, 0xFFFFFFFF)
        central.extend(header)
        central.extend(original[cursor + 46:cursor + 46 + name_length])
        central.extend(struct.pack("<HHQQQ", 1, 24, uncompressed, compressed, local))
        cursor += 46 + name_length
    result = bytearray(original[:offset])
    result.extend(central)
    end64 = len(result)
    result.extend(struct.pack("<IQHHIIQQQQ", 0x06064B50, 44, 45, 45, 0, 0,
                              count, count, len(central), offset))
    result.extend(struct.pack("<IIQI", 0x07064B50, 0, end64, 1))
    result.extend(struct.pack("<IHHHHIIH", 0x06054B50, 0, 0, 0xFFFF, 0xFFFF,
                              0xFFFFFFFF, 0xFFFFFFFF, 0))
    return result


def run_reader(binary, archive, *extra):
    return subprocess.run([str(binary), str(archive), *extra], check=False,
                          text=True, capture_output=True, timeout=30)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("main_source", type=pathlib.Path, help="pinned Main lib/miniz directory")
    args = parser.parse_args()
    source = args.main_source.resolve()
    for name, digest in SOURCE_HASHES.items():
        if hashlib.sha256((source / name).read_bytes()).hexdigest() != digest:
            raise SystemExit(f"{name} does not match Main revision {MAIN_REVISION}")
    compiler = shutil.which("cc")
    rustc = shutil.which("rustc")
    if not rustc:
        candidate = pathlib.Path.home() / ".cargo/bin/rustc"
        rustc = str(candidate) if candidate.is_file() else None
    if not compiler or not rustc:
        raise SystemExit("A local C compiler and rustc are required")
    repository = pathlib.Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="degauss-mainzip-") as temporary:
        work = pathlib.Path(temporary)
        c_source = work / "reader.c"
        c_source.write_text(C_HARNESS)
        main_reader = work / "main-reader"
        subprocess.run([compiler, "-O2", "-I", str(source), str(c_source),
                        str(source / "miniz.c"), "-o", str(main_reader)], check=True)
        rust_source = work / "reader.rs"
        rust_source.write_text(
            '#![allow(dead_code)]\n'
            f'#[path={json.dumps(str(repository / "src/error.rs"), ensure_ascii=False)}] mod error;\n'
            f'#[path={json.dumps(str(repository / "src/zip.rs"), ensure_ascii=False)}] mod zip;\n'
            # The reader logs the members it leaves out through the crate
            # root; here that is stderr, so stdout stays the listing.
            'pub fn note(line: &str) { eprintln!("{line}"); }\n'
            'fn main() { let args: Vec<_> = std::env::args().collect(); let path = std::path::Path::new(&args[1]);\n'
            'let result = if let Some(member) = args.get(2) { zip::validate_member_for_launch(&path.join(member)).map(|e| vec![e]) } else { zip::entries(path) };\n'
            'match result {\n'
            'Ok(entries) => for e in entries { println!("{}\\t{}\\t{:08x}", e.name,e.size,e.crc32); },\n'
            'Err(_) => std::process::exit(1), } }\n')
        degauss_reader = work / "degauss-reader"
        subprocess.run([rustc, "--edition=2021", str(rust_source), "-o", str(degauss_reader)], check=True)
        files = {"Root.rom": b"root fixture bytes\x00" * 32,
                 "Nested/Game.rom": b"nested fixture bytes\xff" * 64}
        fixtures = []
        for name, method, contents in [
            ("stored.zip", zipfile.ZIP_STORED, files),
            ("deflated.zip", zipfile.ZIP_DEFLATED, files),
            ("empty.zip", zipfile.ZIP_STORED, {"Empty.rom": b""}),
            ("directories.zip", zipfile.ZIP_STORED, {**files, "empty/": b""}),
        ]:
            archive = work / name
            with zipfile.ZipFile(archive, "w", compression=method) as writer:
                for member, payload in contents.items():
                    writer.writestr(member, payload)
            fixtures.append((archive, contents))
        archive64 = work / "deflated64.zip"
        archive64.write_bytes(forced_zip64((work / "deflated.zip").read_bytes()))
        fixtures.append((archive64, files))
        members = 0
        for archive, contents in fixtures:
            expected = "".join(f"{name}\t{len(payload)}\t{zlib.crc32(payload):08x}\n"
                               for name, payload in contents.items() if not name.endswith("/"))
            for label, binary in [("Degauss", degauss_reader), ("Main iterator", main_reader)]:
                result = run_reader(binary, archive)
                if result.returncode or result.stdout != expected:
                    raise SystemExit(f"{label} failed {archive.name}: exit {result.returncode}, output {result.stdout!r}")
            members += sum(not name.endswith("/") for name in contents)
            print(f"PASS {archive.name}: exact names, sizes and CRCs through both readers")
        # The mutation touches the first central-directory header, Root.rom.
        # Degauss leaves only that member out and still lists the other;
        # Main's iterator cannot read the member at all.
        original = (work / "stored.zip").read_bytes()
        central = original.find(b"PK\x01\x02")
        remaining = "".join(f"{name}\t{len(payload)}\t{zlib.crc32(payload):08x}\n"
                            for name, payload in files.items() if name != "Root.rom")
        for name, offset, value in [("unsupported-method.zip", 10, 12), ("encrypted.zip", 8, 1)]:
            malformed = bytearray(original)
            malformed[central + offset] = value
            archive = work / name
            archive.write_bytes(malformed)
            result = run_reader(degauss_reader, archive)
            if result.returncode or result.stdout != remaining:
                raise SystemExit(f"Degauss did not keep the supported member of {name}: "
                                 f"exit {result.returncode}, output {result.stdout!r}")
            if run_reader(degauss_reader, archive, "Root.rom").returncode == 0:
                raise SystemExit(f"Degauss offered the unsupported member of {name} for launch")
            if run_reader(main_reader, archive).returncode == 0:
                raise SystemExit(f"Main iterator incorrectly accepted {name}")
            print(f"PASS {name}: unsupported member skipped and refused for launch, Main rejects")
        # A masked local header (general-purpose bit 13) is refused by Main
        # when it reads the central directory, before any member is looked
        # up, so the whole archive has to fail in Degauss as well.
        masked = bytearray(original)
        masked[central + 9] = 32
        archive = work / "masked.zip"
        archive.write_bytes(masked)
        if run_reader(degauss_reader, archive).returncode == 0:
            raise SystemExit("Degauss listed an archive with a masked local header")
        if run_reader(main_reader, archive).returncode != 1:
            raise SystemExit("Pinned Main no longer refuses a masked local header at open; review the member-level skip")
        print("PASS masked.zip: refused whole by both readers")
        # A member disk number that is the ZIP64 sentinel is refused by Main
        # as multi-disk without resolving the ZIP64 value, so Degauss never
        # resolves it either and fails the archive.
        sentinel = bytearray(original)
        sentinel[central + 34:central + 36] = b"\xff\xff"
        archive = work / "sentinel-disk.zip"
        archive.write_bytes(sentinel)
        if run_reader(degauss_reader, archive).returncode == 0:
            raise SystemExit("Degauss listed an archive with a ZIP64 sentinel member disk number")
        if run_reader(main_reader, archive).returncode != 1:
            raise SystemExit("Pinned Main no longer refuses a sentinel member disk number at open; review the disk check")
        print("PASS sentinel-disk.zip: refused whole by both readers")
        comment_archive = work / "comment-signature.zip"
        with zipfile.ZipFile(comment_archive, "w") as writer:
            writer.writestr("Root.rom", files["Root.rom"])
            writer.comment = b"comment PK\x05\x06 with a false signature"
        if run_reader(degauss_reader, comment_archive).returncode:
            raise SystemExit("Degauss failed to list a structurally valid archive comment")
        if run_reader(main_reader, comment_archive).returncode == 0:
            raise SystemExit("Pinned Main comment behavior changed; review the launch compatibility gate")
        if run_reader(degauss_reader, comment_archive, "Root.rom").returncode == 0:
            raise SystemExit("Degauss did not block the known unsupported Main comment launch")
        print("PASS comment-signature.zip: valid listing retained, incompatible Main launch blocked")
        print(f"Verified {members} members in {len(fixtures)} small archives; 2 unsupported members skipped.")
        print(f"Main source: {MAIN_REVISION}. Local host coverage only; no card or core launch.")


if __name__ == "__main__":
    main()
