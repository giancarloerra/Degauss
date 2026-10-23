use crate::settings::FolderPreferences;

const FOLDER_ROTATION: &str = "Screen Rotation";
const FOLDER_POSITION: &str = "Folder Position";
const FOLDER_NAMES: &str = "Game Name Display";

fn set_folder_preferences(
    settings: &mut Settings,
    system: &str,
    place: &str,
    preferences: FolderPreferences,
) {
    if preferences.is_empty() {
        if let Some(places) = settings.folder_preferences.get_mut(system) {
            places.remove(place);
            if places.is_empty() {
                settings.folder_preferences.remove(system);
            }
        }
    } else {
        settings
            .folder_preferences
            .entry(system.into())
            .or_default()
            .insert(place.into(), preferences);
    }
}

#[cfg(test)]
mod folder_preference_tests {
    use super::*;

    #[test]
    fn legacy_settings_stay_global_and_explicit_choices_roundtrip_independently() {
        let mut settings: Settings = toml::from_str("folders_last = true\ngame_name_display = 'keep-region'\nscreen_rotation = 'clockwise'\n").unwrap();
        assert!(settings.folder_preferences.is_empty());
        assert!(!toml::to_string(&settings)
            .unwrap()
            .contains("folder_preferences"));
        let preferences = FolderPreferences {
            folders_last: Some(false),
            ..Default::default()
        };
        set_folder_preferences(
            &mut settings,
            "System: one",
            "d:/games/Shared",
            preferences.clone(),
        );
        set_folder_preferences(
            &mut settings,
            "System: two",
            "d:/games/Shared",
            FolderPreferences {
                screen_rotation: Some(ScreenRotation::Off),
                game_name_display: Some(GameNameDisplay::Full),
                ..Default::default()
            },
        );
        let roundtrip: Settings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(roundtrip, settings);
        assert_eq!(
            roundtrip.folder_preferences["System: one"]["d:/games/Shared"],
            preferences
        );
        set_folder_preferences(
            &mut settings,
            "System: one",
            "d:/games/Shared",
            FolderPreferences::default(),
        );
        assert!(!settings.folder_preferences.contains_key("System: one"));
        assert_eq!(
            settings.folder_preferences["System: two"]["d:/games/Shared"].screen_rotation,
            Some(ScreenRotation::Off)
        );
        assert_eq!(settings.folders_last, Some(true));
        assert_eq!(settings.screen_rotation, Some(ScreenRotation::Clockwise));
        assert_eq!(
            settings.game_name_display,
            Some(GameNameDisplay::KeepRegion)
        );
    }
}

/// Only successful in-app Favourites operations call this. Missing storage
/// and failed operations leave all saved destinations intact.
fn relocate_folder_preferences(
    settings: &mut Settings,
    system: &str,
    source: &Path,
    destination: Option<&Path>,
) -> bool {
    let Some(places) = settings.folder_preferences.get_mut(system) else {
        return false;
    };
    let changed: Vec<_> = places
        .keys()
        .filter_map(|key| {
            let path = Path::new(key.strip_prefix("d:")?);
            let suffix = path.strip_prefix(source).ok()?;
            Some((
                key.clone(),
                destination.map(|destination| {
                    let path = if suffix.as_os_str().is_empty() {
                        destination.to_path_buf()
                    } else {
                        destination.join(suffix)
                    };
                    Place::Dir(path).key()
                }),
            ))
        })
        .collect();
    for (old, new) in &changed {
        let preferences = places.remove(old).unwrap();
        if let Some(new) = new {
            places.insert(new.clone(), preferences);
        }
    }
    if places.is_empty() {
        settings.folder_preferences.remove(system);
    }
    !changed.is_empty()
}

impl App {
    fn current_folder_place(&self) -> Option<(String, String)> {
        if self.browsing != Browsing::Games
            || self.explore.active
            || self.last_played_open
            || self.in_cores_browser()
            || self.in_misterzine_browser()
        {
            return None;
        }
        match self.current_view_place()? {
            ViewPlace::Games { system, place } => Some((system, place)),
            _ => None,
        }
    }

    fn folder_preferences(&self) -> Option<&FolderPreferences> {
        let (system, place) = self.current_folder_place()?;
        self.settings.folder_preferences.get(&system)?.get(&place)
    }

    fn resolve_folder_preferences(&mut self) {
        let preferences = self.folder_preferences().cloned().unwrap_or_default();
        self.folders_last = preferences
            .folders_last
            .unwrap_or_else(|| self.settings.folders_last.unwrap_or(false));
        self.game_name_display = preferences
            .game_name_display
            .unwrap_or_else(|| self.settings.game_name_display.unwrap_or_default());
        if self.rotation_preview_deadline.is_none() {
            self.apply_screen_rotation(
                self.rotation_override
                    .or(preferences.screen_rotation)
                    .unwrap_or(self.global_screen_rotation),
            );
        }
    }

    fn context_page_label(&self, page: ContextPage) -> &'static str {
        if page == ContextPage::Appearance && self.current_folder_place().is_some() {
            "Folder Options"
        } else {
            page.label()
        }
    }

    fn context_page_help(&self, page: ContextPage) -> &'static str {
        if page == ContextPage::Appearance && self.current_folder_place().is_some() {
            "View, rotation, folder position and game names for the open folder."
        } else {
            page.help()
        }
    }

    fn folder_preference_value(&self, action: &str) -> String {
        let preferences = self.folder_preferences().cloned().unwrap_or_default();
        match action {
            FOLDER_ROTATION => preferences.screen_rotation.map(|rotation| rotation.label()),
            FOLDER_POSITION => {
                preferences
                    .folders_last
                    .map(|last| if last { "After Games" } else { "Before Games" })
            }
            FOLDER_NAMES => preferences.game_name_display.map(|mode| mode.label()),
            _ => None,
        }
        .unwrap_or("Use Global")
        .into()
    }

    fn adjust_folder_preference(&mut self, action: &str, delta: isize) {
        let Some((system, place)) = self.current_folder_place() else {
            return;
        };
        let mut preferences = self.folder_preferences().cloned().unwrap_or_default();
        match action {
            FOLDER_ROTATION => {
                let choices = [
                    None,
                    Some(ScreenRotation::Off),
                    Some(ScreenRotation::Clockwise),
                    Some(ScreenRotation::CounterClockwise),
                ];
                let at = choices
                    .iter()
                    .position(|choice| *choice == preferences.screen_rotation)
                    .unwrap();
                let setting = choices[step(at, delta, choices.len())];
                self.preview_rotation(
                    setting.unwrap_or(self.global_screen_rotation),
                    setting,
                    Some((system, place)),
                );
                return;
            }
            FOLDER_POSITION => {
                let choices = [None, Some(false), Some(true)];
                let at = choices
                    .iter()
                    .position(|choice| *choice == preferences.folders_last)
                    .unwrap();
                preferences.folders_last = choices[step(at, delta, choices.len())];
            }
            FOLDER_NAMES => {
                let at = preferences
                    .game_name_display
                    .map_or(0, |mode| mode.index() + 1);
                let next = step(at, delta, GameNameDisplay::ALL.len() + 1);
                preferences.game_name_display =
                    next.checked_sub(1).map(|at| GameNameDisplay::ALL[at]);
            }
            _ => return,
        }
        let mut settings = self.settings.clone();
        set_folder_preferences(&mut settings, &system, &place, preferences);
        match settings.save(&self.settings_path) {
            Ok(outcome) => {
                self.settings = settings;
                self.resolve_folder_preferences();
                self.refresh_name_presentation();
                if let SaveOutcome::InstalledWithWarning(warning) = outcome {
                    self.message = Some(format!("Folder options were saved, but durability could not be confirmed: {warning}"));
                }
            }
            Err(error) => self.message = Some(format!("Folder options were not saved: {error}")),
        }
        self.dirty = true;
    }

    fn update_favorite_folder_preferences(
        &mut self,
        source: &Path,
        destination: Option<&Path>,
    ) -> Option<String> {
        let system = self.open_system.clone()?;
        if !relocate_folder_preferences(&mut self.settings, &system, source, destination) {
            return None;
        }
        match self.settings.save(&self.settings_path) {
            Ok(SaveOutcome::Durable) => None,
            Ok(SaveOutcome::InstalledWithWarning(warning)) => Some(format!(
                "Folder options were saved, but durability could not be confirmed: {warning}"
            )),
            Err(error) => Some(format!(
                "The folder changed, but its options were not saved: {error}\n\nThey remain active only in this session."
            )),
        }
    }
}
