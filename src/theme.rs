//! Named palettes read from the themes folder.
//!
//! One `.toml` per theme, in `themes/` beside `degauss.toml`, the file stem
//! being the name shown in Options. Each file names any subset of the
//! colour roles the `[colors]` block of `degauss.toml` takes, plus one
//! more, `logo`, which paints the wordmark as a flat silhouette. The roles
//! go in as bare keys or under a `[colors]` header, so that block pastes
//! over from `degauss.toml` unchanged. A theme is an overlay over the
//! user's configured palette: what it does not name shows through from
//! `degauss.toml`, never from whichever theme was on before.
//!
//! Read, never written. Choosing a colour with a stick and four buttons is
//! a poor interface for a job a text editor does well, and the card is
//! already in your hand.

use std::path::Path;

use serde::Deserialize;

use crate::config::{Color, Colors};

/// The colours one theme file names. Every field is optional, so a theme
/// can say "amber text on a dark ground" in three lines and leave the rest
/// alone. Unknown keys are rejected, exactly as in `degauss.toml`: a
/// typo'd role that silently did nothing would read as a theme that does
/// not work.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeFile {
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
    colors: Option<ColorsTable>,
}

impl ThemeFile {
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let raw: RawTheme = toml::from_str(text).map_err(|e| e.to_string())?;
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
        let file = ThemeFile {
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
        };
        if !twice.is_empty() {
            return Err(format!(
                "{} set both bare and under [colors]; keep one",
                twice.join(" and ")
            ));
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
            Ok(file) => set.themes.push(Theme {
                name: name.to_string(),
                file,
            }),
            Err(e) => set.problems.push(format!("{name}.toml: {e}")),
        }
    }
    order_and_collide(&mut set);
    set
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
    fn a_missing_folder_means_no_themes_rather_than_an_error() {
        let set = load(Path::new("/definitely/not/here/themes"));
        assert!(set.themes.is_empty());
        assert!(set.problems.is_empty(), "absence is normal, not a problem");
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
