# Degauss artwork agent kit

Distribute these files together:

- `DEGAUSS_AI_ARTWORK_FAQ.md`
- `arcade_artwork_audit.py`
- `arcade_artwork_source.py`
- `degauss_artwork_apply.py`
- `test_arcade_artwork_audit.py`
- `test_arcade_artwork_source.py`
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
python3 -m unittest -v test_arcade_artwork_audit.py test_arcade_artwork_source.py test_degauss_artwork_apply.py
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

The Arcade audit and candidate-discovery scripts are deliberately limited to MRAs from the latest Update All run. `degauss_artwork_apply.py` is the generic reviewed-manifest installer for any system. The FAQ defines the matching, review, import, and manifest requirements that must be completed before that installer is used.
