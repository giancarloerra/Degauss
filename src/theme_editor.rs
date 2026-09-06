//! Controller-only theme editing state.
//!
//! No filesystem or renderer calls live here. Every button transition is
//! testable without a framebuffer, and saving remains a separate transaction.

use crate::config::{Color, Colors};
use crate::font::Font;
use crate::theme::{validate_name, Theme, ThemeFile};

pub const EDITOR_ROWS: usize = 16;
pub const NAME_COLUMNS: usize = 8;

const NAME_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -_";
pub const NAME_SAVE: usize = NAME_CHARS.len();
pub const NAME_CANCEL: usize = NAME_SAVE + 1;
pub const NAME_CELLS: usize = NAME_CANCEL + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeRole {
    Background,
    Panel,
    Surface,
    Bar,
    Text,
    TextDim,
    Accent,
    AccentText,
    State,
    Favorite,
}

impl ThemeRole {
    pub const ALL: [ThemeRole; 10] = [
        ThemeRole::Background,
        ThemeRole::Panel,
        ThemeRole::Surface,
        ThemeRole::Bar,
        ThemeRole::Text,
        ThemeRole::TextDim,
        ThemeRole::Accent,
        ThemeRole::AccentText,
        ThemeRole::State,
        ThemeRole::Favorite,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemeRole::Background => "Background",
            ThemeRole::Panel => "Panel",
            ThemeRole::Surface => "Surface",
            ThemeRole::Bar => "Bottom bar",
            ThemeRole::Text => "Text",
            ThemeRole::TextDim => "Dim text",
            ThemeRole::Accent => "Selection",
            ThemeRole::AccentText => "Selection text",
            ThemeRole::State => "State",
            ThemeRole::Favorite => "Favourite",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|role| *role == self).unwrap_or(0)
    }

    fn get(self, palette: &Colors) -> Color {
        match self {
            ThemeRole::Background => palette.background,
            ThemeRole::Panel => palette.panel,
            ThemeRole::Surface => palette.surface,
            ThemeRole::Bar => palette.bar,
            ThemeRole::Text => palette.text,
            ThemeRole::TextDim => palette.text_dim,
            ThemeRole::Accent => palette.accent,
            ThemeRole::AccentText => palette.accent_text,
            ThemeRole::State => palette.state,
            ThemeRole::Favorite => palette.favorite,
        }
    }

    fn set(self, palette: &mut Colors, color: Color) {
        match self {
            ThemeRole::Background => palette.background = color,
            ThemeRole::Panel => palette.panel = color,
            ThemeRole::Surface => palette.surface = color,
            ThemeRole::Bar => palette.bar = color,
            ThemeRole::Text => palette.text = color,
            ThemeRole::TextDim => palette.text_dim = color,
            ThemeRole::Accent => palette.accent = color,
            ThemeRole::AccentText => palette.accent_text = color,
            ThemeRole::State => palette.state = color,
            ThemeRole::Favorite => palette.favorite = color,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeDraft {
    pub palette: Colors,
    pub logo: Option<Color>,
    pub logo_opacity: u8,
    pub font: Font,
}

impl ThemeDraft {
    pub fn file(&self) -> ThemeFile {
        let mut file = ThemeFile::complete(&self.palette, self.logo, self.logo_opacity);
        file.font = Some(self.font);
        file
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    Browse,
    Hex,
    Picker,
    Swap,
    Name,
    Discard,
    Delete,
}

impl EditorMode {
    pub fn ui_index(self) -> i32 {
        match self {
            EditorMode::Browse => 0,
            EditorMode::Hex => 1,
            EditorMode::Picker => 5,
            EditorMode::Swap => 2,
            EditorMode::Name => 3,
            EditorMode::Discard | EditorMode::Delete => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorEffect {
    None,
    PreviewChanged,
    Save(String),
    Delete(String),
    Close,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorTarget {
    Role(ThemeRole),
    Logo,
}

#[derive(Debug, Clone)]
struct Source {
    name: String,
    draft: ThemeDraft,
    created_by_editor: bool,
}

#[derive(Debug, Clone)]
pub struct ThemeEditor {
    sources: Vec<Source>,
    source: usize,
    pub draft: ThemeDraft,
    original: ThemeDraft,
    pub selected: usize,
    pub mode: EditorMode,
    hex_target: Option<ColorTarget>,
    hex_original: Option<Color>,
    hex_value: Color,
    pub hex_cursor: usize,
    pub picker_channel: usize,
    swap_source: Option<ThemeRole>,
    pub sub_selected: usize,
    pub name: String,
    existing_names: Vec<String>,
}

impl ThemeEditor {
    pub fn new(
        base: &Colors,
        themes: &[Theme],
        active_theme: Option<usize>,
        current_font: Font,
        system_font: Font,
    ) -> Self {
        let mut sources = vec![Source {
            name: "Standard".to_string(),
            draft: ThemeDraft {
                palette: base.clone(),
                logo: None,
                logo_opacity: 100,
                font: system_font,
            },
            created_by_editor: false,
        }];
        sources.extend(themes.iter().enumerate().map(|(index, theme)| Source {
            name: theme.name.clone(),
            draft: ThemeDraft {
                palette: theme.file.apply(base),
                logo: theme.file.logo,
                logo_opacity: theme.file.effective_logo_opacity(),
                // The active source starts from what the user is actually
                // looking at, including a later Text override. Other
                // sources preview their own default when they have one.
                font: if Some(index) == active_theme {
                    current_font
                } else {
                    theme.file.selected_font(system_font)
                },
            },
            created_by_editor: theme.file.created_by_editor,
        }));
        let source = active_theme
            .map_or(0, |index| index + 1)
            .min(sources.len() - 1);
        let draft = sources[source].draft.clone();
        ThemeEditor {
            existing_names: themes.iter().map(|theme| theme.name.clone()).collect(),
            sources,
            source,
            original: draft.clone(),
            draft,
            selected: 0,
            mode: EditorMode::Browse,
            hex_target: None,
            hex_original: None,
            hex_value: Color::new(0, 0, 0),
            hex_cursor: 0,
            picker_channel: 0,
            swap_source: None,
            sub_selected: 0,
            name: String::new(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.draft != self.original
    }

    pub fn selected_role(&self) -> Option<ThemeRole> {
        self.selected
            .checked_sub(1)
            .and_then(|index| ThemeRole::ALL.get(index).copied())
    }

    pub fn source_name(&self) -> &str {
        &self.sources[self.source].name
    }

    pub fn source_is_custom(&self) -> bool {
        self.sources[self.source].created_by_editor
    }

    fn delete_row(&self) -> Option<usize> {
        self.source_is_custom().then_some(15)
    }

    fn cancel_row(&self) -> usize {
        if self.source_is_custom() {
            16
        } else {
            15
        }
    }

    pub fn hex_text(&self) -> String {
        color_hex(self.hex_value)
    }

    pub fn editing_color(&self) -> Color {
        self.hex_value
    }

    pub fn selected_for_ui(&self) -> i32 {
        match self.mode {
            EditorMode::Browse => self.selected as i32,
            EditorMode::Hex | EditorMode::Picker => -1,
            EditorMode::Swap | EditorMode::Name | EditorMode::Discard | EditorMode::Delete => {
                self.sub_selected as i32
            }
        }
    }

    pub fn rows(&self) -> Vec<(String, String)> {
        match self.mode {
            EditorMode::Browse => {
                let mut rows = vec![("Starting point".to_string(), self.source_name().to_string())];
                rows.extend(ThemeRole::ALL.iter().map(|role| {
                    (
                        role.label().to_string(),
                        color_hex(role.get(&self.draft.palette)),
                    )
                }));
                rows.push((
                    "Logo colour".to_string(),
                    self.draft
                        .logo
                        .map(color_hex)
                        .unwrap_or_else(|| "Original".to_string()),
                ));
                rows.push((
                    "Logo colour mix".to_string(),
                    format!("{}%", self.draft.logo_opacity),
                ));
                rows.push((
                    "Default text".to_string(),
                    self.draft.font.shown().to_string(),
                ));
                rows.push(("Save as".to_string(), ">".to_string()));
                if self.source_is_custom() {
                    rows.push(("Delete theme".to_string(), ">".to_string()));
                }
                rows.push(("Cancel".to_string(), String::new()));
                rows
            }
            EditorMode::Hex | EditorMode::Picker => {
                vec![("Colour".to_string(), self.hex_text())]
            }
            EditorMode::Swap => ThemeRole::ALL
                .iter()
                .map(|role| {
                    (
                        role.label().to_string(),
                        color_hex(role.get(&self.draft.palette)),
                    )
                })
                .collect(),
            EditorMode::Name => NAME_CHARS
                .chars()
                .map(|character| (character.to_string(), String::new()))
                .chain([
                    ("Save".to_string(), String::new()),
                    ("Cancel".to_string(), String::new()),
                ])
                .collect(),
            EditorMode::Discard => vec![
                ("Keep editing".to_string(), String::new()),
                ("Discard changes".to_string(), String::new()),
            ],
            EditorMode::Delete => vec![
                ("Keep theme".to_string(), String::new()),
                ("Delete theme".to_string(), String::new()),
            ],
        }
    }

    pub fn vertical(&mut self, delta: isize) -> EditorEffect {
        match self.mode {
            EditorMode::Browse => {
                self.selected = stepped(self.selected, delta, self.rows().len());
                EditorEffect::None
            }
            EditorMode::Hex => self.change_hex_nibble(-delta),
            EditorMode::Picker => {
                self.picker_channel = stepped(self.picker_channel, delta, 3);
                EditorEffect::None
            }
            EditorMode::Swap => {
                self.sub_selected = stepped(self.sub_selected, delta, 10);
                EditorEffect::None
            }
            EditorMode::Name => {
                self.sub_selected =
                    step_grid_vertical(self.sub_selected, delta, NAME_COLUMNS, NAME_CELLS);
                EditorEffect::None
            }
            EditorMode::Discard | EditorMode::Delete => {
                self.sub_selected = stepped(self.sub_selected, delta, 2);
                EditorEffect::None
            }
        }
    }

    pub fn horizontal(&mut self, delta: isize) -> EditorEffect {
        match self.mode {
            EditorMode::Browse => self.change_browse_value(delta),
            EditorMode::Hex => {
                self.hex_cursor = stepped(self.hex_cursor, delta, 6);
                EditorEffect::None
            }
            EditorMode::Picker => self.change_picker_channel(delta),
            EditorMode::Swap => {
                self.sub_selected = stepped(self.sub_selected, delta, 10);
                EditorEffect::None
            }
            EditorMode::Name => {
                self.sub_selected = stepped(self.sub_selected, delta, NAME_CELLS);
                EditorEffect::None
            }
            EditorMode::Discard | EditorMode::Delete => {
                self.sub_selected = stepped(self.sub_selected, delta, 2);
                EditorEffect::None
            }
        }
    }

    fn change_browse_value(&mut self, delta: isize) -> EditorEffect {
        if self.selected == 0 {
            self.source = stepped(self.source, delta, self.sources.len());
            self.draft = self.sources[self.source].draft.clone();
            return EditorEffect::PreviewChanged;
        }
        if self.selected_role().is_some() {
            return EditorEffect::None;
        }
        match self.selected {
            11 => {
                if delta != 0 {
                    self.draft.logo = if self.draft.logo.is_some() {
                        None
                    } else {
                        Some(self.draft.palette.accent)
                    };
                    EditorEffect::PreviewChanged
                } else {
                    EditorEffect::None
                }
            }
            12 => {
                let opacity = self.draft.logo_opacity as isize + delta * 5;
                self.draft.logo_opacity = opacity.clamp(0, 100) as u8;
                EditorEffect::PreviewChanged
            }
            13 => {
                self.draft.font = if delta < 0 {
                    self.draft.font.prev()
                } else {
                    self.draft.font.next()
                };
                EditorEffect::PreviewChanged
            }
            _ => EditorEffect::None,
        }
    }

    pub fn accept(&mut self) -> EditorEffect {
        match self.mode {
            EditorMode::Browse => {
                if let Some(role) = self.selected_role() {
                    let color = role.get(&self.draft.palette);
                    self.open_color(ColorTarget::Role(role), Some(color), color);
                    return EditorEffect::None;
                }
                match self.selected {
                    0 => self.change_browse_value(1),
                    11 => {
                        let original = self.draft.logo;
                        self.open_color(
                            ColorTarget::Logo,
                            original,
                            original.unwrap_or(self.draft.palette.accent),
                        );
                        EditorEffect::None
                    }
                    12 => self.change_browse_value(1),
                    13 => self.change_browse_value(1),
                    14 => {
                        self.mode = EditorMode::Name;
                        self.name.clear();
                        self.sub_selected = 0;
                        EditorEffect::None
                    }
                    selected if Some(selected) == self.delete_row() => {
                        self.mode = EditorMode::Delete;
                        self.sub_selected = 0;
                        EditorEffect::None
                    }
                    selected if selected == self.cancel_row() => self.request_close(),
                    _ => EditorEffect::None,
                }
            }
            EditorMode::Hex | EditorMode::Picker => {
                self.mode = EditorMode::Browse;
                self.hex_target = None;
                EditorEffect::None
            }
            EditorMode::Swap => {
                let Some(source) = self.swap_source else {
                    self.mode = EditorMode::Browse;
                    return EditorEffect::None;
                };
                let target = ThemeRole::ALL[self.sub_selected];
                let source_color = source.get(&self.draft.palette);
                let target_color = target.get(&self.draft.palette);
                source.set(&mut self.draft.palette, target_color);
                target.set(&mut self.draft.palette, source_color);
                self.mode = EditorMode::Browse;
                self.swap_source = None;
                EditorEffect::PreviewChanged
            }
            EditorMode::Name => self.accept_name_cell(),
            EditorMode::Discard => {
                if self.sub_selected == 1 {
                    EditorEffect::Close
                } else {
                    self.mode = EditorMode::Browse;
                    EditorEffect::None
                }
            }
            EditorMode::Delete => {
                if self.sub_selected == 1 {
                    EditorEffect::Delete(self.source_name().to_string())
                } else {
                    self.mode = EditorMode::Browse;
                    EditorEffect::None
                }
            }
        }
    }

    pub fn back(&mut self) -> EditorEffect {
        if self.mode == EditorMode::Browse {
            self.request_close()
        } else {
            let effect = if matches!(self.mode, EditorMode::Hex | EditorMode::Picker)
                && self.restore_hex_original()
            {
                EditorEffect::PreviewChanged
            } else {
                EditorEffect::None
            };
            self.mode = EditorMode::Browse;
            self.hex_target = None;
            self.swap_source = None;
            effect
        }
    }

    pub fn x(&mut self) -> EditorEffect {
        match self.mode {
            EditorMode::Browse => {
                let Some(role) = self.selected_role() else {
                    return EditorEffect::None;
                };
                self.swap_source = Some(role);
                self.sub_selected = role.index();
                self.mode = EditorMode::Swap;
                EditorEffect::None
            }
            EditorMode::Picker => {
                self.hex_cursor = self.picker_channel * 2;
                self.mode = EditorMode::Hex;
                EditorEffect::None
            }
            EditorMode::Hex => {
                self.picker_channel = self.hex_cursor / 2;
                self.mode = EditorMode::Picker;
                EditorEffect::None
            }
            EditorMode::Name => {
                self.name.pop();
                EditorEffect::None
            }
            EditorMode::Swap | EditorMode::Discard | EditorMode::Delete => EditorEffect::None,
        }
    }

    pub fn y(&mut self) -> EditorEffect {
        match self.mode {
            EditorMode::Hex | EditorMode::Picker => {
                let changed = self.restore_hex_original();
                EditorEffect::from_preview_change(changed)
            }
            EditorMode::Name => {
                self.name.clear();
                EditorEffect::None
            }
            _ => EditorEffect::None,
        }
    }

    fn request_close(&mut self) -> EditorEffect {
        if self.dirty() {
            self.mode = EditorMode::Discard;
            self.sub_selected = 0;
            EditorEffect::None
        } else {
            EditorEffect::Close
        }
    }

    fn open_color(&mut self, target: ColorTarget, original: Option<Color>, color: Color) {
        self.hex_target = Some(target);
        self.hex_original = original;
        self.hex_value = color;
        self.hex_cursor = 0;
        self.picker_channel = 0;
        self.mode = EditorMode::Picker;
    }

    fn change_picker_channel(&mut self, delta: isize) -> EditorEffect {
        if delta == 0 {
            return EditorEffect::None;
        }
        let before = self.draft.clone();
        let mut bytes = [self.hex_value.r, self.hex_value.g, self.hex_value.b];
        bytes[self.picker_channel] =
            (bytes[self.picker_channel] as isize + delta * 5).clamp(0, 255) as u8;
        self.hex_value = Color::new(bytes[0], bytes[1], bytes[2]);
        self.apply_hex_value();
        EditorEffect::from_preview_change(self.draft != before)
    }

    fn change_hex_nibble(&mut self, delta: isize) -> EditorEffect {
        let current = self.hex_nibble() as usize;
        self.set_hex_nibble(stepped(current, delta, 16) as u8)
    }

    fn hex_nibble(&self) -> u8 {
        let bytes = [self.hex_value.r, self.hex_value.g, self.hex_value.b];
        let byte = bytes[self.hex_cursor / 2];
        if self.hex_cursor.is_multiple_of(2) {
            byte >> 4
        } else {
            byte & 0x0f
        }
    }

    fn set_hex_nibble(&mut self, nibble: u8) -> EditorEffect {
        let before = self.draft.clone();
        let component = self.hex_cursor / 2;
        let high = self.hex_cursor.is_multiple_of(2);
        let mut bytes = [self.hex_value.r, self.hex_value.g, self.hex_value.b];
        bytes[component] = if high {
            (bytes[component] & 0x0f) | ((nibble & 0x0f) << 4)
        } else {
            (bytes[component] & 0xf0) | (nibble & 0x0f)
        };
        self.hex_value = Color::new(bytes[0], bytes[1], bytes[2]);
        self.apply_hex_value();
        EditorEffect::from_preview_change(self.draft != before)
    }

    fn apply_hex_value(&mut self) {
        match self.hex_target {
            Some(ColorTarget::Role(role)) => role.set(&mut self.draft.palette, self.hex_value),
            Some(ColorTarget::Logo) => self.draft.logo = Some(self.hex_value),
            None => {}
        }
    }

    fn restore_hex_original(&mut self) -> bool {
        let before = self.draft.clone();
        match self.hex_target {
            Some(ColorTarget::Role(role)) => {
                if let Some(color) = self.hex_original {
                    role.set(&mut self.draft.palette, color);
                    self.hex_value = color;
                }
            }
            Some(ColorTarget::Logo) => {
                self.draft.logo = self.hex_original;
                self.hex_value = self.hex_original.unwrap_or(self.draft.palette.accent);
            }
            None => {}
        }
        self.draft != before
    }

    fn accept_name_cell(&mut self) -> EditorEffect {
        if self.sub_selected < NAME_SAVE {
            let character = NAME_CHARS.chars().nth(self.sub_selected).unwrap_or(' ');
            if self.name.chars().count() < 250 {
                self.name.push(character);
            }
            return EditorEffect::None;
        }
        if self.sub_selected == NAME_CANCEL {
            self.mode = EditorMode::Browse;
            return EditorEffect::None;
        }
        match validate_name(&self.name, &self.existing_names) {
            Ok(name) => EditorEffect::Save(name),
            Err(error) => EditorEffect::Error(error),
        }
    }
}

impl EditorEffect {
    fn from_preview_change(changed: bool) -> Self {
        if changed {
            EditorEffect::PreviewChanged
        } else {
            EditorEffect::None
        }
    }
}

fn stepped(current: usize, delta: isize, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    (current as isize + delta).rem_euclid(count as isize) as usize
}

fn step_grid_vertical(current: usize, delta: isize, columns: usize, cells: usize) -> usize {
    if columns == 0 || cells == 0 || delta == 0 {
        return current.min(cells.saturating_sub(1));
    }
    let rows = cells.div_ceil(columns);
    let direction = delta.signum();
    let mut selected = current.min(cells - 1);
    for _ in 0..delta.unsigned_abs() {
        let row = selected / columns;
        let column = selected % columns;
        for distance in 1..=rows {
            let next_row =
                (row as isize + direction * distance as isize).rem_euclid(rows as isize) as usize;
            let candidate = next_row * columns + column;
            if candidate < cells {
                selected = candidate;
                break;
            }
        }
    }
    selected
}

fn color_hex(color: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> ThemeEditor {
        let theme = Theme {
            name: "Existing".to_string(),
            file: ThemeFile::complete(&Colors::default(), None, 100),
        };
        ThemeEditor::new(
            &Colors::default(),
            &[theme],
            None,
            Font::Smooth,
            Font::Smooth,
        )
    }

    #[test]
    fn shipped_or_external_starting_points_become_complete_drafts() {
        let partial = Theme {
            name: "Partial".to_string(),
            file: ThemeFile::parse(r##"text = "#010203""##).unwrap(),
        };
        let base = Colors::default();
        let mut editor = ThemeEditor::new(&base, &[partial], None, Font::Smooth, Font::Smooth);
        assert_eq!(editor.horizontal(1), EditorEffect::PreviewChanged);
        assert_eq!(editor.source_name(), "Partial");
        assert_eq!(editor.draft.palette.text, Color::new(1, 2, 3));
        assert_eq!(editor.draft.palette.panel, base.panel);
        assert!(editor.dirty());
    }

    #[test]
    fn every_hex_change_previews_immediately_and_cancel_restores_the_exact_colour() {
        let mut editor = editor();
        editor.selected = 1;
        let before = editor.draft.palette.background;
        editor.accept();
        assert_eq!(editor.mode, EditorMode::Picker);
        editor.x();
        assert_eq!(editor.mode, EditorMode::Hex);
        assert_eq!(editor.vertical(-1), EditorEffect::PreviewChanged);
        assert_ne!(editor.draft.palette.background, before);
        assert_eq!(editor.back(), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.palette.background, before);

        editor.accept();
        editor.x();
        assert_eq!(editor.vertical(-1), EditorEffect::PreviewChanged);
        assert_eq!(editor.accept(), EditorEffect::None);
        assert_ne!(editor.draft.palette.background, before);
    }

    #[test]
    fn picker_and_hex_reset_update_the_live_preview() {
        let mut editor = editor();
        editor.selected = 1;
        let before = editor.draft.palette.background;
        editor.accept();
        assert_eq!(editor.horizontal(1), EditorEffect::PreviewChanged);
        assert_eq!(editor.y(), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.palette.background, before);
        assert_eq!(editor.x(), EditorEffect::None);
        assert_eq!(editor.mode, EditorMode::Hex);
        assert_eq!(editor.vertical(-1), EditorEffect::PreviewChanged);
        assert_eq!(editor.y(), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.palette.background, before);
    }

    #[test]
    fn cancelling_live_logo_hex_edit_restores_original_multicolour_mode() {
        let mut editor = editor();
        editor.selected = 11;
        assert_eq!(editor.draft.logo, None);
        editor.accept();
        assert_eq!(editor.mode, EditorMode::Picker);
        assert_eq!(editor.horizontal(-1), EditorEffect::PreviewChanged);
        assert!(editor.draft.logo.is_some());
        assert_eq!(editor.back(), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.logo, None);
    }

    #[test]
    fn continuous_picker_changes_each_rgb_channel_without_quantising_other_channels() {
        let mut editor = editor();
        editor.selected = 1;
        editor.draft.palette.background = Color::new(101, 102, 103);
        editor.accept();
        assert_eq!(editor.editing_color(), Color::new(101, 102, 103));
        assert_eq!(editor.horizontal(1), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.palette.background, Color::new(106, 102, 103));
        assert_eq!(editor.vertical(1), EditorEffect::None);
        assert_eq!(editor.picker_channel, 1);
        assert_eq!(editor.horizontal(-1), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.palette.background, Color::new(106, 97, 103));
        assert_eq!(editor.vertical(1), EditorEffect::None);
        assert_eq!(editor.horizontal(1), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.palette.background, Color::new(106, 97, 108));
        assert_eq!(editor.x(), EditorEffect::None);
        assert_eq!(editor.mode, EditorMode::Hex);
        assert_eq!(editor.hex_cursor, 4);
        assert_eq!(editor.horizontal(-1), EditorEffect::None);
        assert_eq!(editor.x(), EditorEffect::None);
        assert_eq!(editor.mode, EditorMode::Picker);
        assert_eq!(editor.picker_channel, 1);
    }

    #[test]
    fn browse_directions_never_replace_a_role_with_a_fixed_palette_colour() {
        let mut editor = editor();
        editor.selected = 1;
        let before = editor.draft.palette.background;
        assert_eq!(editor.horizontal(1), EditorEffect::None);
        assert_eq!(editor.horizontal(-1), EditorEffect::None);
        assert_eq!(editor.draft.palette.background, before);
    }

    #[test]
    fn default_text_is_explicit_editable_and_saved_with_the_theme() {
        let mut editor = editor();
        editor.selected = 13;
        assert_eq!(editor.rows()[13].1, "Smooth");
        assert_eq!(editor.horizontal(1), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.font, Font::Pixel);
        assert_eq!(editor.rows()[13].1, "Pixel");
        assert_eq!(editor.accept(), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.font, Font::Smooth2);
        assert_eq!(editor.draft.file().font, Some(Font::Smooth2));
    }

    #[test]
    fn accept_advances_logo_colour_mix_as_advertised() {
        let mut editor = editor();
        editor.selected = 12;
        editor.draft.logo_opacity = 40;
        assert_eq!(editor.accept(), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.logo_opacity, 45);
    }

    #[test]
    fn delete_is_visible_and_confirmed_only_for_editor_saved_sources() {
        let external = editor();
        assert!(!external.source_is_custom());
        assert_eq!(external.rows().len(), EDITOR_ROWS);
        assert!(!external.rows().iter().any(|row| row.0 == "Delete theme"));

        let mut file = ThemeFile::complete(&Colors::default(), None, 100);
        file.created_by_editor = true;
        let custom = Theme {
            name: "My Theme".to_string(),
            file,
        };
        let mut custom = ThemeEditor::new(
            &Colors::default(),
            &[custom],
            Some(0),
            Font::Smooth,
            Font::Smooth,
        );
        assert!(custom.source_is_custom());
        assert_eq!(custom.rows().len(), EDITOR_ROWS + 1);
        assert_eq!(custom.rows()[15].0, "Delete theme");
        custom.selected = 15;
        assert_eq!(custom.accept(), EditorEffect::None);
        assert_eq!(custom.mode, EditorMode::Delete);
        assert_eq!(
            custom.accept(),
            EditorEffect::None,
            "Keep theme is the default"
        );
        assert_eq!(custom.mode, EditorMode::Browse);
        custom.selected = 15;
        custom.accept();
        custom.sub_selected = 1;
        assert_eq!(
            custom.accept(),
            EditorEffect::Delete("My Theme".to_string())
        );
    }

    #[test]
    fn the_active_source_starts_from_a_users_later_font_override() {
        let mut file = ThemeFile::complete(&Colors::default(), None, 100);
        file.font = Some(Font::Pixel2);
        let theme = Theme {
            name: "Defaulted".to_string(),
            file,
        };
        let editor = ThemeEditor::new(
            &Colors::default(),
            &[theme],
            Some(0),
            Font::Smooth,
            Font::Pixel,
        );
        assert_eq!(editor.source_name(), "Defaulted");
        assert_eq!(
            editor.draft.font,
            Font::Smooth,
            "opening the editor must not undo a later Text override"
        );
    }

    #[test]
    fn logo_colour_can_return_to_original_without_a_fixed_palette() {
        let mut editor = editor();
        editor.selected = 11;
        assert_eq!(editor.draft.logo, None);
        assert_eq!(editor.horizontal(1), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.logo, Some(editor.draft.palette.accent));
        assert_eq!(editor.horizontal(-1), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.logo, None);
    }

    #[test]
    fn swap_exchanges_exact_role_values() {
        let mut editor = editor();
        editor.selected = 1;
        let background = editor.draft.palette.background;
        let panel = editor.draft.palette.panel;
        editor.x();
        editor.sub_selected = ThemeRole::Panel.index();
        assert_eq!(editor.accept(), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.palette.background, panel);
        assert_eq!(editor.draft.palette.panel, background);

        editor.x();
        editor.sub_selected = ThemeRole::Panel.index();
        assert_eq!(editor.accept(), EditorEffect::PreviewChanged);
        assert_eq!(editor.draft.palette.background, background);
        assert_eq!(editor.draft.palette.panel, panel);
    }

    #[test]
    fn hex_edit_always_contains_exactly_six_hexadecimal_digits() {
        let mut editor = editor();
        editor.selected = 1;
        editor.accept();
        editor.x();
        for _ in 0..20 {
            editor.vertical(-1);
            editor.horizontal(1);
            let text = editor.hex_text();
            assert_eq!(text.len(), 7);
            assert_eq!(text.as_bytes()[0], b'#');
            assert!(text[1..].bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn leaving_a_changed_editor_requires_explicit_discard() {
        let mut editor = editor();
        assert_eq!(editor.back(), EditorEffect::Close);
        editor.selected = 1;
        editor.accept();
        editor.horizontal(1);
        editor.accept();
        assert_eq!(editor.back(), EditorEffect::None);
        assert_eq!(editor.mode, EditorMode::Discard);
        assert_eq!(editor.accept(), EditorEffect::None);
        assert_eq!(editor.mode, EditorMode::Browse);
        editor.back();
        editor.sub_selected = 1;
        assert_eq!(editor.accept(), EditorEffect::Close);
    }

    #[test]
    fn name_validation_is_case_insensitive_and_fat_safe() {
        let existing = vec!["Green Mono".to_string()];
        assert_eq!(
            validate_name("  New Theme  ", &existing).unwrap(),
            "New Theme"
        );
        assert!(validate_name("green mono", &existing).is_err());
        assert!(validate_name("../theme", &existing).is_err());
        assert!(validate_name("NUL", &existing).is_err());
        assert!(validate_name("COM1", &existing).is_err());
        assert!(validate_name("COM0", &existing).is_ok());
    }

    #[test]
    fn name_grid_back_cancels_and_x_and_y_edit_the_name() {
        let mut editor = editor();
        editor.selected = 14;
        editor.accept();
        editor.accept();
        assert_eq!(editor.name, "A");
        editor.x();
        assert!(editor.name.is_empty());
        editor.accept();
        editor.y();
        assert!(editor.name.is_empty());
        editor.back();
        assert_eq!(editor.mode, EditorMode::Browse);
    }

    #[test]
    fn name_grid_vertical_movement_keeps_its_column_across_the_short_last_row() {
        assert_eq!(
            NAME_CELLS, 41,
            "the regression requires a partial final row"
        );
        assert_eq!(step_grid_vertical(0, -1, NAME_COLUMNS, NAME_CELLS), 40);
        assert_eq!(step_grid_vertical(40, 1, NAME_COLUMNS, NAME_CELLS), 0);
        assert_eq!(step_grid_vertical(1, -1, NAME_COLUMNS, NAME_CELLS), 33);
        assert_eq!(step_grid_vertical(33, 1, NAME_COLUMNS, NAME_CELLS), 1);
        assert_eq!(step_grid_vertical(39, 1, NAME_COLUMNS, NAME_CELLS), 7);
        assert_eq!(step_grid_vertical(7, -1, NAME_COLUMNS, NAME_CELLS), 39);
    }
}
