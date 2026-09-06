//! Named palettes read from the themes folder.
//!
//! One `.toml` per theme, in `themes/` beside `degauss.toml`, the file stem
//! being the name shown in Options. Each file names any subset of the
//! colour roles the `[colors]` block of `degauss.toml` takes, plus one
//! more, `logo`, which paints the wordmark as a flat silhouette, and
//! `logo_opacity`, which mixes that colour over the original wordmark. The roles
//! go in as bare keys or under a `[colors]` header, so that block pastes
//! over from `degauss.toml` unchanged. A theme is an overlay over the
//! user's configured palette: what it does not name shows through from
//! `degauss.toml`, never from whichever theme was on before. A top-level
//! `font` is an optional default applied when the theme is selected; old
//! themes without it use the user's system-wide Text choice.
//!
//! Files created by the on-device editor are complete themes. Existing
//! partial themes remain overlays and retain their original meaning.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::{Color, Colors};
use crate::error::{DegaussError, Result};
use crate::font::Font;

/// The colours one theme file names. Every field is optional, so a theme
/// can say "amber text on a dark ground" in three lines and leave the rest
/// alone. Unknown keys are rejected, exactly as in `degauss.toml`: a
/// typo'd role that silently did nothing would read as a theme that does
/// not work.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeFile {
    /// Typeface selected with this theme. Absent uses the system-wide Text
    /// choice, preserving old theme files without inheriting another theme.
    pub font: Option<Font>,
    /// Editor-created files carry this marker. Canonical editor output that
    /// predates the marker is also recognised during parsing.
    pub created_by_editor: bool,
    /// A bad optional font does not discard an otherwise usable palette. It
    /// is reported while the effective font follows the same system-font path
    /// as a missing key.
    font_problem: Option<String>,
    pub background: Option<Color>,
    pub panel: Option<Color>,
    pub surface: Option<Color>,
    pub bar: Option<Color>,
    pub text: Option<Color>,
    pub text_dim: Option<Color>,
    pub accent: Option<Color>,
    pub accent_text: Option<Color>,
    pub state: Option<Color>,
    pub favorite: Option<Color>,
    /// Draw the wordmark as a flat silhouette in this colour. Absent means
    /// the wordmark keeps its own three colours.
    pub logo: Option<Color>,
    /// Amount of the flat logo colour mixed over the original artwork, as a
    /// percentage. Absent means 100 percent, preserving the fully monochrome
    /// silhouette produced by themes written before this key existed.
    pub logo_opacity: Option<u8>,
}

/// The colour roles as they may appear under a `[colors]` header: the
/// exact set that block takes in `degauss.toml`, so it pastes in
/// unchanged. `logo` is not among them there and is not here either.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColorsTable {
    background: Option<Color>,
    panel: Option<Color>,
    surface: Option<Color>,
    bar: Option<Color>,
    text: Option<Color>,
    text_dim: Option<Color>,
    accent: Option<Color>,
    accent_text: Option<Color>,
    state: Option<Color>,
    favorite: Option<Color>,
}

/// A theme file as written: the roles as bare keys, or the same roles
/// under a `[colors]` header, whichever the author reached for.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTheme {
    font: Option<String>,
    #[serde(default)]
    created_by_editor: bool,
    background: Option<Color>,
    panel: Option<Color>,
    surface: Option<Color>,
    bar: Option<Color>,
    text: Option<Color>,
    text_dim: Option<Color>,
    accent: Option<Color>,
    accent_text: Option<Color>,
    state: Option<Color>,
    favorite: Option<Color>,
    logo: Option<Color>,
    logo_opacity: Option<u8>,
    colors: Option<ColorsTable>,
}

impl ThemeFile {
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let raw: RawTheme = toml::from_str(text).map_err(|e| e.to_string())?;
        let (font, font_problem) = match raw.font.as_deref() {
            None => (None, None),
            Some(name) => match Font::parse(name) {
                Some(font) => (Some(font), None),
                None => (
                    None,
                    Some(format!("unknown font {name:?}; using the Text setting")),
                ),
            },
        };
        let table = raw.colors.unwrap_or_default();
        // A role named both bare and under [colors] is refused rather
        // than one copy quietly winning: whichever the author meant, the
        // other is a lie in the file.
        let mut twice = Vec::new();
        let mut pick = |role: &str, bare: Option<Color>, tabled: Option<Color>| {
            if bare.is_some() && tabled.is_some() {
                twice.push(role.to_string());
            }
            bare.or(tabled)
        };
        let mut file = ThemeFile {
            font,
            created_by_editor: raw.created_by_editor,
            font_problem,
            background: pick("background", raw.background, table.background),
            panel: pick("panel", raw.panel, table.panel),
            surface: pick("surface", raw.surface, table.surface),
            bar: pick("bar", raw.bar, table.bar),
            text: pick("text", raw.text, table.text),
            text_dim: pick("text_dim", raw.text_dim, table.text_dim),
            accent: pick("accent", raw.accent, table.accent),
            accent_text: pick("accent_text", raw.accent_text, table.accent_text),
            state: pick("state", raw.state, table.state),
            favorite: pick("favorite", raw.favorite, table.favorite),
            logo: raw.logo,
            logo_opacity: raw.logo_opacity,
        };
        if !twice.is_empty() {
            return Err(format!(
                "{} set both bare and under [colors]; keep one",
                twice.join(" and ")
            ));
        }
        if file.logo_opacity.is_some_and(|opacity| opacity > 100) {
            return Err("logo_opacity must be between 0 and 100".to_string());
        }
        // Earlier editor output was complete and canonical but predated the
        // provenance marker. Recognise only that exact
        // serialization. A merely complete hand-written theme must not gain
        // a destructive control because its fields happen to resemble one.
        if !file.created_by_editor && text == file.to_toml(&Colors::default()) {
            file.created_by_editor = true;
        }
        Ok(file)
    }

    /// The palette this theme puts on screen: its own colours where it
    /// names one, `base` everywhere else. `base` is the user's parsed
    /// `[colors]`, so a partial theme inherits local edits rather than a
    /// factory palette nobody configured.
    pub fn apply(&self, base: &Colors) -> Colors {
        Colors {
            background: self.background.unwrap_or(base.background),
            panel: self.panel.unwrap_or(base.panel),
            surface: self.surface.unwrap_or(base.surface),
            bar: self.bar.unwrap_or(base.bar),
            text: self.text.unwrap_or(base.text),
            text_dim: self.text_dim.unwrap_or(base.text_dim),
            accent: self.accent.unwrap_or(base.accent),
            accent_text: self.accent_text.unwrap_or(base.accent_text),
            state: self.state.unwrap_or(base.state),
            favorite: self.favorite.unwrap_or(base.favorite),
        }
    }

    /// The effective flat-colour mix. Existing files omit the key and retain
    /// the fully monochrome logo selected by their `logo` value.
    pub fn effective_logo_opacity(&self) -> u8 {
        self.logo_opacity.unwrap_or(100)
    }

    /// Font this theme resolves to. A missing or invalid optional key uses
    /// the system-wide Text choice, never the previously selected theme.
    pub fn selected_font(&self, system: Font) -> Font {
        self.font.unwrap_or(system)
    }

    /// A self-contained theme. Editor output never inherits colours from a
    /// different installation's `degauss.toml`.
    pub fn complete(palette: &Colors, logo: Option<Color>, logo_opacity: u8) -> Self {
        assert!(logo_opacity <= 100, "logo colour mix must be 0 through 100");
        ThemeFile {
            font: None,
            created_by_editor: false,
            font_problem: None,
            background: Some(palette.background),
            panel: Some(palette.panel),
            surface: Some(palette.surface),
            bar: Some(palette.bar),
            text: Some(palette.text),
            text_dim: Some(palette.text_dim),
            accent: Some(palette.accent),
            accent_text: Some(palette.accent_text),
            state: Some(palette.state),
            favorite: Some(palette.favorite),
            logo,
            logo_opacity: Some(logo_opacity),
        }
    }

    /// Stable, human-editable output from the on-device editor.
    pub fn to_toml(&self, base: &Colors) -> String {
        let palette = self.apply(base);
        let mut body = if self.created_by_editor {
            "created_by_editor = true\n".to_string()
        } else {
            String::new()
        };
        body.push_str(
            &self
                .font
                .map(|font| format!("font = {:?}\n", font.label()))
                .unwrap_or_default(),
        );
        body.push_str(&format!(
            "background = \"{}\"\npanel = \"{}\"\nsurface = \"{}\"\nbar = \"{}\"\n\
             text = \"{}\"\ntext_dim = \"{}\"\naccent = \"{}\"\naccent_text = \"{}\"\n\
             state = \"{}\"\nfavorite = \"{}\"\n",
            color_hex(palette.background),
            color_hex(palette.panel),
            color_hex(palette.surface),
            color_hex(palette.bar),
            color_hex(palette.text),
            color_hex(palette.text_dim),
            color_hex(palette.accent),
            color_hex(palette.accent_text),
            color_hex(palette.state),
            color_hex(palette.favorite),
        ));
        if let Some(logo) = self.logo {
            body.push_str(&format!("logo = \"{}\"\n", color_hex(logo)));
        }
        body.push_str(&format!(
            "logo_opacity = {}\n",
            self.effective_logo_opacity()
        ));
        body
    }
}

fn color_hex(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// Validate the part before `.toml` against the FAT/exFAT filename contract
/// used by MiSTer cards. The returned name is trimmed and ready to write.
pub fn validate_name(name: &str, existing: &[String]) -> std::result::Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Theme name cannot be empty".to_string());
    }
    // FAT32 and exFAT allow 255 characters for the complete filename. The
    // extension consumes five of them, including its dot.
    if name.chars().count() > 250 {
        return Err("Theme name is too long for a 255-character filename".to_string());
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, ' ' | '-' | '_'))
    {
        return Err("Theme name may contain letters, numbers, spaces, - and _".to_string());
    }
    let upper = name.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0');
    if reserved {
        return Err(format!("{name:?} is a reserved filename"));
    }
    if existing.iter().any(|held| held.eq_ignore_ascii_case(name)) {
        return Err(format!("A theme named {name:?} already exists"));
    }
    Ok(name.to_string())
}

/// A theme that loaded, under the name its file carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// The file stem: `amber.toml` is the theme called `amber`.
    pub name: String,
    pub file: ThemeFile,
}

/// What the themes folder held: the themes that loaded, and a line for
/// each file that did not. Problems are carried alongside rather than
/// returned as an error, because one broken theme must not stop Degauss
/// starting; they are shown on screen so the typo is found rather than
/// wondered about.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeSet {
    pub themes: Vec<Theme>,
    pub problems: Vec<String>,
}

/// Read every theme in a folder. A folder that is not there means no
/// themes, the same as an empty one: the folder is optional and most cards
/// will not have it.
pub fn load(dir: &Path) -> ThemeSet {
    let mut set = ThemeSet::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return set,
        Err(e) => {
            set.problems.push(format!("themes: {e}"));
            return set;
        }
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry.path()),
            // An entry the filesystem would not hand over is a theme that
            // silently would not exist; say so instead.
            Err(e) => {
                set.problems.push(format!("themes: {e}"));
                None
            }
        })
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
        })
        .collect();
    // Walked in name order so the problem lines come out the same way
    // twice, whatever order the directory hands the entries back in.
    paths.sort();
    for path in paths {
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            set.problems
                .push("themes: a file name that is not text was skipped".to_string());
            continue;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                set.problems.push(format!("{name}.toml: {e}"));
                continue;
            }
        };
        match ThemeFile::parse(&text) {
            Ok(file) => {
                if let Some(problem) = &file.font_problem {
                    set.problems.push(format!("{name}.toml: {problem}"));
                }
                set.themes.push(Theme {
                    name: name.to_string(),
                    file,
                });
            }
            Err(e) => set.problems.push(format!("{name}.toml: {e}")),
        }
    }
    order_and_collide(&mut set);
    set
}

/// Read card themes and add the shipped palettes. A card file owns its name,
/// even when malformed, so a built-in cannot conceal an error in that file.
pub fn load_available(dir: &Path) -> ThemeSet {
    let reserved = names_on_disk(dir);
    let mut set = load(dir);
    for theme in builtins() {
        if !reserved
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&theme.name))
        {
            set.themes.push(theme);
        }
    }
    order_and_collide(&mut set);
    set
}

fn names_on_disk(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
        })
        .filter_map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
        })
        .collect()
}

fn builtins() -> Vec<Theme> {
    let partial = |palette: Colors, logo: Color, font: Font| {
        let mut file = ThemeFile::complete(&palette, Some(logo), 100);
        // Existing shipped themes deliberately inherit the installation's
        // favourite colour. New shipped palettes keep that contract.
        file.favorite = None;
        file.font = Some(font);
        file
    };
    vec![
        Theme {
            name: "Green Mono".to_string(),
            file: partial(
                Colors {
                    background: Color::new(0x06, 0x14, 0x0b),
                    panel: Color::new(0x0a, 0x24, 0x12),
                    surface: Color::new(0x0d, 0x30, 0x18),
                    bar: Color::new(0x04, 0x10, 0x08),
                    text: Color::new(0xb8, 0xff, 0xd0),
                    text_dim: Color::new(0x68, 0xb8, 0x80),
                    accent: Color::new(0x33, 0xff, 0x66),
                    accent_text: Color::new(0x03, 0x10, 0x06),
                    favorite: Colors::default().favorite,
                    state: Color::new(0x42, 0xe8, 0x78),
                },
                Color::new(0x33, 0xff, 0x66),
                Font::Pixel2,
            ),
        },
        Theme {
            name: "Modern".to_string(),
            file: partial(
                Colors {
                    background: Color::new(0x11, 0x16, 0x1c),
                    panel: Color::new(0x18, 0x23, 0x2d),
                    surface: Color::new(0x22, 0x31, 0x3d),
                    bar: Color::new(0x0b, 0x10, 0x15),
                    text: Color::new(0xf2, 0xf5, 0xf7),
                    text_dim: Color::new(0xa8, 0xb4, 0xbe),
                    accent: Color::new(0x74, 0xb4, 0xff),
                    accent_text: Color::new(0x07, 0x13, 0x1e),
                    favorite: Colors::default().favorite,
                    state: Color::new(0x55, 0xd6, 0xbe),
                },
                Color::new(0x74, 0xb4, 0xff),
                Font::Smooth2,
            ),
        },
        Theme {
            name: "Neon".to_string(),
            file: partial(
                Colors {
                    background: Color::new(0x0b, 0x06, 0x14),
                    panel: Color::new(0x24, 0x10, 0x3c),
                    surface: Color::new(0x17, 0x10, 0x2b),
                    bar: Color::new(0x07, 0x04, 0x0d),
                    text: Color::new(0xf7, 0xf0, 0xff),
                    text_dim: Color::new(0xb9, 0xa3, 0xd9),
                    accent: Color::new(0x00, 0xf5, 0xff),
                    accent_text: Color::new(0x05, 0x07, 0x0a),
                    favorite: Colors::default().favorite,
                    state: Color::new(0xff, 0x3e, 0xb5),
                },
                Color::new(0xff, 0x3e, 0xb5),
                Font::Pixel,
            ),
        },
    ]
}

/// Write a new theme without replacing any existing file. The destination is
/// opened exclusively: Linux exFAT does not implement `RENAME_NOREPLACE`, but
/// `O_EXCL` is supported and gives the same no-overwrite guarantee.
pub fn save_new(dir: &Path, name: &str, file: &ThemeFile, base: &Colors) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .map_err(|error| DegaussError::io("creating themes directory", dir, error))?;
    let mut reserved = names_on_disk(dir);
    reserved.extend(builtins().into_iter().map(|theme| theme.name));
    let name = validate_name(name, &reserved)
        .map_err(|error| DegaussError::unsupported("saving theme", error))?;

    let final_path = dir.join(format!("{name}.toml"));
    let mut editor_file = file.clone();
    editor_file.created_by_editor = true;
    editor_file.font_problem = None;
    let body = editor_file.to_toml(base);
    let mut handle = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&final_path)
        .map_err(|error| DegaussError::io("creating theme", &final_path, error))?;
    let write_result: Result<()> = (|| {
        handle
            .write_all(body.as_bytes())
            .map_err(|error| DegaussError::io("writing theme", &final_path, error))?;
        handle
            .sync_all()
            .map_err(|error| DegaussError::io("flushing theme", &final_path, error))?;
        Ok(())
    })();
    if let Err(error) = write_result {
        drop(handle);
        return match std::fs::remove_file(&final_path) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(DegaussError::unsupported(
                "saving theme",
                format!(
                    "{error}; removing the incomplete file {} also failed: {cleanup}",
                    final_path.display()
                ),
            )),
        };
    }
    Ok(final_path)
}

/// A custom theme renamed out of the loader's `.toml` namespace while its
/// deletion transaction is completed. The original can be restored if saving
/// the corresponding settings change fails.
#[derive(Debug)]
pub struct StagedThemeDeletion {
    original: PathBuf,
    staged: PathBuf,
}

impl StagedThemeDeletion {
    pub fn restore(&self) -> Result<()> {
        std::fs::rename(&self.staged, &self.original)
            .map_err(|error| DegaussError::io("restoring theme", &self.original, error))
    }

    pub fn commit(&self) -> Result<()> {
        std::fs::remove_file(&self.staged)
            .map_err(|error| DegaussError::io("deleting theme", &self.original, error))
    }
}

/// Remove only a file recognised as custom editor output, including the
/// canonical format written before the marker. The rename is
/// same-directory so a failed settings save can restore the exact original
/// bytes without relying on a second copy.
pub fn stage_editor_theme_deletion(dir: &Path, theme: &Theme) -> Result<StagedThemeDeletion> {
    if !theme.file.created_by_editor {
        return Err(DegaussError::unsupported(
            "deleting theme",
            "only themes saved by the Theme editor can be deleted here",
        ));
    }
    let original = dir.join(format!("{}.toml", theme.name));
    if !original.is_file() {
        return Err(DegaussError::unsupported(
            "deleting theme",
            format!("{} is not a custom theme file", original.display()),
        ));
    }
    for attempt in 0..32_u8 {
        let staged = dir.join(format!(
            ".{}.{}.{}.degauss-delete",
            theme.name,
            std::process::id(),
            attempt
        ));
        if staged.exists() {
            continue;
        }
        match std::fs::rename(&original, &staged) {
            Ok(()) => return Ok(StagedThemeDeletion { original, staged }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(DegaussError::io("staging theme deletion", &original, error)),
        }
    }
    Err(DegaussError::unsupported(
        "deleting theme",
        "could not reserve a temporary deletion name",
    ))
}

/// Sort the themes for the Options row, without regard to case, and refuse
/// names that differ only by case. The card's filesystem does not tell
/// `Amber` from `amber`, so two such files cannot both exist once the
/// folder is on a card; refusing them on every filesystem means a theme
/// never works on the desk and then vanishes on the machine.
fn order_and_collide(set: &mut ThemeSet) {
    set.themes.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut kept = Vec::new();
    let mut at = 0;
    while at < set.themes.len() {
        let mut end = at + 1;
        while end < set.themes.len()
            && set.themes[end].name.to_lowercase() == set.themes[at].name.to_lowercase()
        {
            end += 1;
        }
        if end - at > 1 {
            let names: Vec<&str> = set.themes[at..end]
                .iter()
                .map(|theme| theme.name.as_str())
                .collect();
            set.problems.push(format!(
                "themes {} differ only by case; all of them ignored",
                names.join(" and ")
            ));
        } else {
            kept.push(set.themes[at].clone());
        }
        at = end;
    }
    set.themes = kept;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("degauss-themes-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_theme_naming_three_colours_leaves_the_rest_to_the_base() {
        let theme = ThemeFile::parse(
            r##"
            background = "#000000"
            text = "#33ff33"
            accent = "#33ff33"
            "##,
        )
        .expect("a partial theme parses");
        let base = Colors::default();
        let applied = theme.apply(&base);
        assert_eq!(applied.background, Color::new(0, 0, 0));
        assert_eq!(applied.text, Color::new(0x33, 0xff, 0x33));
        assert_eq!(
            applied.panel, base.panel,
            "unnamed roles come from the base"
        );
        assert_eq!(applied.favorite, base.favorite);
    }

    #[test]
    fn a_theme_naming_every_colour_replaces_the_whole_palette() {
        let theme = ThemeFile::parse(
            r##"
            background = "#010101"
            panel = "#020202"
            surface = "#030303"
            bar = "#040404"
            text = "#050505"
            text_dim = "#060606"
            accent = "#070707"
            accent_text = "#080808"
            state = "#090909"
            favorite = "#0a0a0a"
            "##,
        )
        .expect("a full theme parses");
        let applied = theme.apply(&Colors::default());
        assert_eq!(applied.background, Color::new(1, 1, 1));
        assert_eq!(applied.bar, Color::new(4, 4, 4));
        assert_eq!(applied.favorite, Color::new(0x0a, 0x0a, 0x0a));
    }

    #[test]
    fn the_submission_template_stays_complete_and_parseable() {
        let theme = ThemeFile::parse(include_str!("../docs/theme-template.toml"))
            .expect("the documented submission template must parse as a real theme");

        assert!(theme.background.is_some());
        assert!(theme.panel.is_some());
        assert!(theme.surface.is_some());
        assert!(theme.bar.is_some());
        assert!(theme.text.is_some());
        assert!(theme.text_dim.is_some());
        assert!(theme.accent.is_some());
        assert!(theme.accent_text.is_some());
        assert!(theme.state.is_some());
        assert!(theme.favorite.is_some());
        assert_eq!(
            theme.logo, None,
            "the original wordmark remains the template default"
        );
    }

    #[test]
    fn a_typo_in_a_role_name_is_rejected_with_the_key() {
        let err = ThemeFile::parse(r##"textt = "#ffffff""##).expect_err("must reject a typo");
        assert!(err.contains("textt"), "got: {err}");
        let err = ThemeFile::parse("[colors]\ntextt = \"#ffffff\"")
            .expect_err("must reject a typo under the header too");
        assert!(err.contains("textt"), "got: {err}");
    }

    #[test]
    fn the_colors_block_from_the_config_pastes_in_unchanged() {
        // The readme sends people to copy their [colors] block into a
        // theme file. Copied with its header, it has to mean exactly what
        // the bare keys mean, logo still alongside.
        let theme = ThemeFile::parse(
            r##"
            logo = "#ffb000"

            [colors]
            background = "#000000"
            text = "#33ff33"
            "##,
        )
        .expect("a pasted block parses");
        assert_eq!(theme.background, Some(Color::new(0, 0, 0)));
        assert_eq!(theme.text, Some(Color::new(0x33, 0xff, 0x33)));
        assert_eq!(theme.logo, Some(Color::new(0xff, 0xb0, 0x00)));
        assert_eq!(theme.panel, None, "unnamed roles stay unnamed");
    }

    #[test]
    fn a_role_set_both_bare_and_under_the_header_is_refused_by_name() {
        // Two values for one role cannot both be meant. Refused with the
        // role's name, not resolved by a precedence rule nobody wrote
        // down.
        let err = ThemeFile::parse(
            r##"
            text = "#ffffff"

            [colors]
            text = "#000000"
            "##,
        )
        .expect_err("must refuse the double");
        assert!(err.contains("text"), "got: {err}");
    }

    #[test]
    fn a_bad_colour_is_rejected_with_the_offending_value() {
        let err =
            ThemeFile::parse(r##"text = "green""##).expect_err("must reject a non-hex colour");
        assert!(err.contains("rrggbb"), "got: {err}");
    }

    #[test]
    fn the_logo_colour_is_only_there_when_a_theme_names_one() {
        let with = ThemeFile::parse(r##"logo = "#ffb000""##).expect("parses");
        assert_eq!(with.logo, Some(Color::new(0xff, 0xb0, 0x00)));
        let without = ThemeFile::parse(r##"text = "#ffffff""##).expect("parses");
        assert_eq!(without.logo, None);
    }

    #[test]
    fn old_and_invalid_theme_fonts_use_the_system_font() {
        let old = ThemeFile::parse(r##"text = "#ffffff""##).expect("old theme parses");
        assert_eq!(old.font, None);
        assert_eq!(old.selected_font(Font::Pixel2), Font::Pixel2);

        let pixel = ThemeFile::parse("font = \"PIXEL 2\"").expect("font parses");
        assert_eq!(pixel.font, Some(Font::Pixel2));
        assert_eq!(pixel.selected_font(Font::Smooth), Font::Pixel2);

        let invalid = ThemeFile::parse("font = \"future font\"")
            .expect("an invalid optional font must not discard the palette");
        assert_eq!(invalid.font, None);
        assert_eq!(invalid.selected_font(Font::Pixel), Font::Pixel);
        assert!(invalid
            .font_problem
            .as_deref()
            .is_some_and(|problem| problem.contains("future font")));
    }

    #[test]
    fn old_themes_keep_their_full_logo_colour_and_new_mix_is_bounded() {
        let old = ThemeFile::parse(r##"text = "#ffffff""##).expect("old theme parses");
        assert_eq!(old.logo_opacity, None);
        assert_eq!(old.effective_logo_opacity(), 100);

        for opacity in 0..=100 {
            let parsed = ThemeFile::parse(&format!("logo_opacity = {opacity}"))
                .expect("every percentage in the documented range parses");
            assert_eq!(parsed.effective_logo_opacity(), opacity);
        }
        assert!(ThemeFile::parse("logo_opacity = -1").is_err());
        let error = ThemeFile::parse("logo_opacity = 101").expect_err("range is enforced");
        assert!(error.contains("0 and 100"), "got: {error}");
    }

    #[test]
    fn a_complete_theme_serialises_and_round_trips_every_role() {
        let palette = Colors::default();
        let mut file = ThemeFile::complete(&palette, Some(Color::new(1, 2, 3)), 45);
        file.font = Some(Font::Smooth2);
        let text = file.to_toml(&Colors {
            background: Color::new(9, 9, 9),
            ..Colors::default()
        });
        let parsed = ThemeFile::parse(&text).expect("editor output parses in production parser");
        assert_eq!(
            parsed.apply(&Colors::default()).background,
            palette.background
        );
        assert_eq!(parsed.apply(&Colors::default()).favorite, palette.favorite);
        assert_eq!(parsed.logo, Some(Color::new(1, 2, 3)));
        assert_eq!(parsed.effective_logo_opacity(), 45);
        assert_eq!(parsed.font, Some(Font::Smooth2));
        assert!(text.starts_with("font = \"smooth 2\"\n"));
        assert!(parsed.created_by_editor);
    }

    #[test]
    fn a_missing_folder_means_no_themes_rather_than_an_error() {
        let set = load(Path::new("/definitely/not/here/themes"));
        assert!(set.themes.is_empty());
        assert!(set.problems.is_empty(), "absence is normal, not a problem");
    }

    #[test]
    fn shipped_themes_exist_without_a_themes_folder() {
        let set = load_available(Path::new("/definitely/not/here/themes"));
        let names: Vec<&str> = set.themes.iter().map(|theme| theme.name.as_str()).collect();
        assert_eq!(names, ["Green Mono", "Modern", "Neon"]);
        assert!(set.problems.is_empty());
        let custom_favorite = Color::new(1, 2, 3);
        let base = Colors {
            favorite: custom_favorite,
            ..Colors::default()
        };
        for theme in &set.themes {
            assert!(theme.file.background.is_some());
            assert!(theme.file.panel.is_some());
            assert!(theme.file.surface.is_some());
            assert!(theme.file.bar.is_some());
            assert!(theme.file.text.is_some());
            assert!(theme.file.text_dim.is_some());
            assert!(theme.file.accent.is_some());
            assert!(theme.file.accent_text.is_some());
            assert!(theme.file.state.is_some());
            assert_eq!(theme.file.favorite, None);
            assert!(theme.file.logo.is_some());
            assert_eq!(theme.file.effective_logo_opacity(), 100);
            assert_eq!(
                theme.file.apply(&base).favorite,
                custom_favorite,
                "{} must preserve the installation's favourite colour",
                theme.name
            );
        }
        assert_eq!(set.themes[0].file.font, Some(Font::Pixel2));
        assert_eq!(set.themes[1].file.font, Some(Font::Smooth2));
        assert_eq!(set.themes[2].file.font, Some(Font::Pixel));
    }

    #[test]
    fn a_card_file_owns_a_shipped_theme_name_without_regard_to_case() {
        let dir = temp_dir("builtin-override");
        std::fs::write(dir.join("green mono.toml"), r##"text = "#010203""##).unwrap();
        let set = load_available(&dir);
        let matching: Vec<&Theme> = set
            .themes
            .iter()
            .filter(|theme| theme.name.eq_ignore_ascii_case("Green Mono"))
            .collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].name, "green mono");
        assert_eq!(matching[0].file.text, Some(Color::new(1, 2, 3)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_card_file_is_not_concealed_by_a_shipped_theme() {
        let dir = temp_dir("builtin-broken");
        std::fs::write(dir.join("Neon.toml"), "not toml [").unwrap();
        let set = load_available(&dir);
        assert!(!set
            .themes
            .iter()
            .any(|theme| theme.name.eq_ignore_ascii_case("Neon")));
        assert_eq!(set.problems.len(), 1);
        assert!(set.problems[0].starts_with("Neon.toml"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saving_creates_exclusively_reloads_and_never_overwrites() {
        let root = temp_dir("save-new");
        let dir = root.join("nested/themes");
        let palette = Colors::default();
        let file = ThemeFile::complete(&palette, None, 100);
        let path = save_new(&dir, "My Theme", &file, &palette).expect("first save succeeds");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("My Theme.toml")
        );
        let parsed = ThemeFile::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(parsed.created_by_editor);
        assert_eq!(parsed.apply(&palette).accent, palette.accent);
        let loaded = load_available(&dir);
        assert!(loaded
            .themes
            .iter()
            .any(|theme| theme.name == "My Theme" && theme.file == parsed));
        assert!(save_new(&dir, "my theme", &file, &palette).is_err());
        assert!(
            save_new(&dir, "neon", &file, &palette).is_err(),
            "the editor must not create a file that shadows a compiled theme"
        );
        assert!(save_new(&dir, "../outside", &file, &palette).is_err());
        let files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, ["My Theme.toml"]);

        let blocked = dir.join("Blocked.toml");
        std::fs::create_dir(&blocked).unwrap();
        assert!(
            save_new(&dir, "Blocked", &file, &palette).is_err(),
            "exclusive creation must report an occupied final path"
        );
        assert!(
            blocked.is_dir(),
            "a failed save must not remove what occupied the name"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_an_editor_marked_theme_can_be_staged_deleted_and_restored() {
        let dir = temp_dir("delete-custom");
        let palette = Colors::default();
        let file = ThemeFile::complete(&palette, None, 100);
        let path = save_new(&dir, "Custom", &file, &palette).expect("editor save succeeds");
        let custom = load(&dir).themes.remove(0);
        assert!(custom.file.created_by_editor);

        let staged = stage_editor_theme_deletion(&dir, &custom).expect("custom can be staged");
        assert!(!path.exists());
        assert!(load(&dir).themes.is_empty());
        staged.restore().expect("failed transaction can restore");
        assert!(path.exists());

        let custom = load(&dir).themes.remove(0);
        stage_editor_theme_deletion(&dir, &custom)
            .expect("custom can be staged again")
            .commit()
            .expect("staged deletion commits");
        assert!(!path.exists());

        let hand_path = dir.join("Hand written.toml");
        std::fs::write(&hand_path, r##"text = "#ffffff""##).unwrap();
        let hand_written = load(&dir).themes.remove(0);
        assert!(!hand_written.file.created_by_editor);
        assert!(stage_editor_theme_deletion(&dir, &hand_written).is_err());
        assert!(hand_path.exists());

        let complete_path = dir.join("Complete by hand.toml");
        std::fs::write(
            &complete_path,
            format!("# not written by the editor\n{}", file.to_toml(&palette)),
        )
        .unwrap();
        let complete = load(&dir)
            .themes
            .into_iter()
            .find(|theme| theme.name == "Complete by hand")
            .unwrap();
        assert!(!complete.file.created_by_editor);
        assert!(stage_editor_theme_deletion(&dir, &complete).is_err());
        assert!(complete_path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_invalid_font_is_reported_without_hiding_the_theme() {
        let dir = temp_dir("invalid-font");
        std::fs::write(
            dir.join("Old.toml"),
            "font = \"future font\"\ntext = \"#ffffff\"\n",
        )
        .unwrap();
        let set = load(&dir);
        assert_eq!(set.themes.len(), 1);
        assert_eq!(set.themes[0].file.font, None);
        assert_eq!(set.problems.len(), 1);
        assert!(set.problems[0].contains("unknown font"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_malformed_file_is_reported_and_the_rest_still_load() {
        let dir = temp_dir("malformed");
        std::fs::write(dir.join("good.toml"), r##"text = "#ffffff""##).unwrap();
        std::fs::write(dir.join("bad.toml"), "not toml at all [").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();

        let set = load(&dir);
        assert_eq!(set.themes.len(), 1);
        assert_eq!(set.themes[0].name, "good");
        assert_eq!(set.problems.len(), 1, "only the broken theme is reported");
        assert!(
            set.problems[0].starts_with("bad.toml"),
            "got: {:?}",
            set.problems
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn themes_come_back_sorted_by_name_without_regard_to_case() {
        let mut set = ThemeSet {
            themes: vec![
                Theme {
                    name: "Zebra".into(),
                    file: ThemeFile::default(),
                },
                Theme {
                    name: "amber".into(),
                    file: ThemeFile::default(),
                },
            ],
            problems: Vec::new(),
        };
        order_and_collide(&mut set);
        let names: Vec<&str> = set.themes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["amber", "Zebra"],
            "capital letters must not sort first"
        );
    }

    #[test]
    fn two_names_differing_only_by_case_are_both_refused() {
        // Built by hand rather than on disk: the development machine's own
        // filesystem may be case-insensitive too, in which case two such
        // files cannot even be written for the test.
        let mut set = ThemeSet {
            themes: vec![
                Theme {
                    name: "Amber".into(),
                    file: ThemeFile::default(),
                },
                Theme {
                    name: "amber".into(),
                    file: ThemeFile::default(),
                },
                Theme {
                    name: "green".into(),
                    file: ThemeFile::default(),
                },
            ],
            problems: Vec::new(),
        };
        order_and_collide(&mut set);
        let names: Vec<&str> = set.themes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["green"], "both colliding names must go");
        assert_eq!(set.problems.len(), 1);
        assert!(
            set.problems[0].contains("Amber") && set.problems[0].contains("amber"),
            "the message must name the colliding files: {:?}",
            set.problems
        );
    }
}
