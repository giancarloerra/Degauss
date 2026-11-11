use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable version-one commands. Targets are server-issued identities, never
/// client-supplied filesystem paths. Read commands do not move native selection.
#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Command {
    State,
    WebSettings,
    WebSettingsSave {
        revision: String,
        config: super::Config,
        password: Option<String>,
    },
    WebCertificateRenew,
    IniProfiles,
    IniSwitch {
        slot: u8,
    },
    Systems,
    Browse {
        system: String,
        folder: Option<String>,
        #[serde(default)]
        offset: usize,
        #[serde(default = "page_size")]
        limit: usize,
        #[serde(default)]
        search: String,
        #[serde(default)]
        filters: crate::explore::Query,
    },
    Game {
        id: String,
    },
    Explore {
        query: crate::explore::Query,
        #[serde(default)]
        offset: usize,
        #[serde(default = "page_size")]
        limit: usize,
    },
    Home {
        folder: Option<String>,
    },
    HomeArtwork {
        folder: Option<String>,
    },
    HomeEdit {
        revision: String,
        edit: HomeEdit,
    },
    Collections,
    CollectionSave {
        revision: String,
        id: Option<String>,
        name: String,
        query: crate::explore::Query,
    },
    CollectionDelete {
        revision: String,
        id: String,
    },
    Views {
        target: Option<String>,
    },
    ViewSave {
        revision: String,
        id: Option<String>,
        definition: crate::custom_view::Definition,
    },
    ViewDelete {
        revision: String,
        id: String,
    },
    ViewAssign {
        revision: String,
        target: Option<String>,
        view: String,
    },
    Options,
    Reset {
        revision: String,
        kind: Reset,
    },
    FolderOptions {
        target: String,
    },
    FolderOptionsSave {
        revision: String,
        target: String,
        preferences: crate::settings::FolderPreferences,
    },
    Hide {
        revision: String,
        target: String,
        hidden: bool,
    },
    Themes,
    ThemeSave {
        revision: String,
        name: String,
        text: String,
        #[serde(default)]
        replace: bool,
    },
    ThemeDelete {
        revision: String,
        name: String,
    },
    ThemeSelect {
        revision: String,
        name: Option<String>,
    },
    Controllers,
    ScraperSettings,
    ScraperSettingsSave {
        revision: String,
        values: Value,
        password: Option<String>,
        #[serde(default)]
        accept_plaintext_storage: bool,
    },
    ScraperSearch {
        game: String,
        term: String,
    },
    ScraperPreview {
        match_id: String,
    },
    Scrape {
        target: Option<String>,
        match_id: Option<String>,
    },
    GameData {
        system: String,
    },
    GameDataLocations {
        system: String,
        picture_type: crate::artwork_pack::PictureType,
        directory: Option<String>,
    },
    GameDataSave {
        revision: String,
        system: String,
        source: GameDataSource,
        location: Option<String>,
        picture_type: Option<crate::artwork_pack::PictureType>,
    },
    PackMatches {
        game: String,
        #[serde(default)]
        search: String,
    },
    PackMatchSave {
        revision: String,
        game: String,
        choice: Option<String>,
    },
    ControllerSave {
        revision: String,
        id: String,
        profile: crate::controller::Profile,
    },
    Cores {
        category: Option<String>,
    },
    CoreChoices {
        system: String,
        game: Option<String>,
    },
    CoreSelect {
        revision: String,
        system: String,
        game: Option<String>,
        family: Option<String>,
        version: Option<String>,
    },
    Job {
        id: u64,
    },
    OptionAdjust {
        revision: String,
        option: String,
        delta: i8,
    },
    Launch {
        id: String,
    },
    Random {
        #[serde(default)]
        query: crate::explore::Query,
        folder: Option<String>,
        #[serde(default)]
        favorites_only: bool,
    },
    Remote {
        button: RemoteButton,
    },
    Unlock {
        pin: String,
    },
    Attract,
    Rebuild {
        system: Option<String>,
    },
    Cancel {
        id: u64,
    },
    CoreUpdates {
        #[serde(default)]
        refresh: bool,
        #[serde(default)]
        begin: bool,
    },
    CoreUpdate {
        key: String,
    },
    CoreUpdatesSeen {
        snapshot: String,
    },
    History,
    FavoriteFolders,
    FavoriteAdd {
        game: String,
        folder: Option<String>,
        name: Option<String>,
    },
    FavoriteRemove {
        game: String,
    },
    FavoriteFolderCreate {
        name: String,
    },
    FavoriteFolderRename {
        folder: String,
        name: String,
    },
    FavoriteFolderDelete {
        folder: String,
    },
    Images,
    ImageChoose {
        revision: String,
        target: String,
        image: Option<String>,
    },
    Scripts {
        folder: Option<String>,
    },
}

fn page_size() -> usize {
    60
}

impl Command {
    pub fn changes(&self) -> bool {
        if matches!(self, Self::WebSettings | Self::IniProfiles) {
            return false;
        }
        !matches!(
            self,
            Self::State
                | Self::Systems
                | Self::Browse { .. }
                | Self::Game { .. }
                | Self::Explore { .. }
                | Self::Home { .. }
                | Self::HomeArtwork { .. }
                | Self::Collections
                | Self::Views { .. }
                | Self::Options
                | Self::Job { .. }
                | Self::History
                | Self::Scripts { .. }
                | Self::CoreUpdates { refresh: false, .. }
                | Self::CoreUpdate { .. }
                | Self::Themes
                | Self::Controllers
                | Self::Cores { .. }
                | Self::CoreChoices { .. }
                | Self::FolderOptions { .. }
                | Self::ScraperSettings
                | Self::GameData { .. }
                | Self::GameDataLocations { .. }
                | Self::PackMatches { .. }
                | Self::ScraperSearch { .. }
                | Self::ScraperPreview { .. }
                | Self::FavoriteFolders
                | Self::Images
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum HomeEdit {
    Add {
        name: String,
        target: String,
        parent: Option<String>,
    },
    Folder {
        name: String,
        parent: Option<String>,
    },
    Rename {
        id: String,
        name: String,
    },
    Remove {
        id: String,
    },
    Move {
        id: String,
        parent: Option<String>,
    },
    Reorder {
        folder: Option<String>,
        keys: Vec<String>,
    },
    Visibility {
        key: String,
        visible: bool,
    },
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteButton {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
    Actions,
    Menu,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reset {
    ViewSelections,
    Hidden,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GameDataSource {
    Automatic,
    Gamelist,
    ArtworkPack,
}

#[derive(Debug, Serialize)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub enum Reply {
    Json(Value),
    Media { path: std::path::PathBuf },
}

pub type Result = std::result::Result<Reply, Error>;
