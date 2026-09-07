#!/usr/bin/env python3
"""Package corresponding RA Main source from a completed, verified local build."""
import gzip
import hashlib
import pathlib
import sys
import tarfile


def package(build):
    build = pathlib.Path(build)
    required = ('MiSTer_RA_Degauss', 'MiSTer_RA_Degauss.cacert.pem',
                'SOURCE-PINS.txt', 'source/LICENSE', 'source/Makefile',
                'source/ra_http.cpp', 'rebuild/scripts/build-ra-main.sh',
                'rebuild/scripts/package-ra-main.py', 'rebuild/support/ra-main/frontend.patch',
                'rebuild/support/ra-main/tls.patch', 'rebuild/support/ra-main/controller.patch', 'rebuild/support/ra-main/Dockerfile')
    for name in required:
        if not (build / name).is_file():
            raise SystemExit(f'package-ra-main: missing {name}')
    archive = build / 'MiSTer_RA_Degauss-source.tar.gz'
    def source_filter(info):
        parts = pathlib.PurePosixPath(info.name).parts
        if parts[:3] == ('ra-main-source', 'source', 'bin') or '.git' in parts or '__pycache__' in parts:
            return None
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        info.mtime = 0
        if info.isdir():
            info.mode = 0o755
        elif info.isfile() or info.islnk():
            info.mode = 0o755 if info.mode & 0o111 else 0o644
        return info
    with archive.open('wb') as output, gzip.GzipFile(filename='', fileobj=output, mode='wb', mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode='w') as tar:
            for name in ('source', 'rebuild', 'SOURCE-PINS.txt', 'MiSTer_RA_Degauss.cacert.pem'):
                tar.add(build / name, arcname=f'ra-main-source/{name}', filter=source_filter)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (build / (archive.name + '.sha256')).write_text(f'{digest}  {archive.name}\n')
    notice = '''MiSTer_RA_Degauss is derived from RA Main and MiSTer Main (GPL-3.0-or-later).
Degauss frontend, TLS and controller integration source and build instructions:
  MiSTer_RA_Degauss-source.tar.gz, attached to the same Degauss release.
Upstream: https://github.com/odelot/Main_MiSTer
Degauss releases: https://github.com/giancarloerra/Degauss/releases

The adjacent Mozilla CA bundle is MPL-2.0, from https://curl.se/docs/caextract.html.
The source archive includes its bundle and source attribution.

'''
    (build / 'MiSTer_RA_Degauss.SOURCE.txt').write_text(notice + (build / 'SOURCE-PINS.txt').read_text())
    print(archive)


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit('Usage: package-ra-main.py BUILD_DIRECTORY')
    package(sys.argv[1])
