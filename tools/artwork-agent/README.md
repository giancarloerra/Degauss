# Degauss artwork agent kit

Distribute these files together:

- `DEGAUSS_AI_ARTWORK_FAQ.md`
- `arcade_artwork_audit.py`
- `arcade_artwork_source.py`
- `system_artwork_audit.py`
- `system_artwork_source.py`
- `degauss_artwork_apply.py`
- `test_arcade_artwork_audit.py`
- `test_arcade_artwork_source.py`
- `test_system_artwork_audit.py`
- `test_system_artwork_source.py`
- `test_degauss_artwork_apply.py`
- `requirements.txt`
- `LICENSE`

The scripts contain no user or developer credentials. They expect the passwordless `mister` SSH alias described in the FAQ.

Install the local image decoder dependency:

```bash
python3 -m pip install -r requirements.txt
```

Run the regression tests before use:

```bash
python3 -m unittest -v test_*.py
```

ScreenScraper candidate lookup is optional. It requires a protected developer configuration at the default path:

```text
~/.config/degauss-artwork-agent/screenscraper-developer.toml
```

The file format is:

```toml
developer_id = "..."
developer_password = "..."
```

It can instead be selected with `--developer-config /protected/path/file.toml` or the `SCREENSCRAPER_DEVELOPER_CONFIG` environment variable. The user's ScreenScraper account remains in Degauss's protected MiSTer configuration. Never include either credential file in this kit.

The Arcade tools are deliberately limited to MRAs from the latest Update All run. The system tools implement complete-system adapters for Amiga, Commodore 64, Genesis, SNES, and Nintendo 64. `degauss_artwork_apply.py` is the reviewed-manifest installer used by both workflows. The FAQ defines the matching, review, import, and manifest requirements that must be completed before that installer is used.

The kit never launches an emulator or core to create artwork. It uses only pre-existing card art, user-owned frontend collections, and exact images from verified public online sources.
