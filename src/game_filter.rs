//! Temporary, in-memory filters over the metadata already attached to browse rows.

use std::collections::BTreeMap;

use crate::browse::{Kind, Row};

/// One metadata field offered by the filter screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Genre,
    Year,
    Players,
    Language,
    Developer,
    Publisher,
}

impl Field {
    pub const ALL: [Self; 6] = [
        Self::Genre,
        Self::Year,
        Self::Players,
        Self::Language,
        Self::Developer,
        Self::Publisher,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|field| *field == self).unwrap()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Genre => "Genre",
            Self::Year => "Year",
            Self::Players => "Players",
            Self::Language => "Language",
            Self::Developer => "Developer",
            Self::Publisher => "Publisher",
        }
    }

    fn value(self, row: &Row) -> Option<&str> {
        let value = match self {
            Self::Genre => row.genre.as_deref()?,
            Self::Year => presented_year(&row.details.released)?,
            Self::Players => &row.details.players,
            Self::Language => &row.details.lang,
            Self::Developer => &row.details.developer,
            Self::Publisher => &row.details.publisher,
        };
        let value = value.trim();
        (!value.is_empty()).then_some(value)
    }
}

/// The exact value selected for one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Criterion {
    Known { label: String, key: String },
    Unknown,
}

impl Criterion {
    pub fn label(&self) -> &str {
        match self {
            Self::Known { label, .. } => label,
            Self::Unknown => "Unknown",
        }
    }
}

/// One row in a field's value chooser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Any,
    Known { label: String, key: String },
    Unknown,
}

impl Choice {
    pub fn label(&self) -> &str {
        match self {
            Self::Any => "Any",
            Self::Known { label, .. } => label,
            Self::Unknown => "Unknown",
        }
    }

    fn criterion(&self) -> Option<Criterion> {
        match self {
            Self::Any => None,
            Self::Known { label, key } => Some(Criterion::Known {
                label: label.clone(),
                key: key.clone(),
            }),
            Self::Unknown => Some(Criterion::Unknown),
        }
    }
}

/// All active metadata filters for the current browse place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    selected: [Option<Criterion>; 6],
}

impl Filters {
    pub fn is_active(&self) -> bool {
        self.selected.iter().any(Option::is_some)
    }

    pub fn clear(&mut self) {
        self.selected = Default::default();
    }

    pub fn label(&self, field: Field) -> &str {
        self.selected[field.index()]
            .as_ref()
            .map(Criterion::label)
            .unwrap_or("Any")
    }

    pub fn choose(&mut self, field: Field, choice: &Choice) {
        self.selected[field.index()] = choice.criterion();
    }

    pub fn choice_is_selected(&self, field: Field, choice: &Choice) -> bool {
        self.selected[field.index()] == choice.criterion()
    }

    /// Whether a playable row satisfies every selected field.
    ///
    /// Folder rows are deliberately never tested here. The caller keeps them
    /// visible so a filter cannot make a nested library unreachable.
    pub fn matches(&self, row: &Row) -> bool {
        debug_assert!(matches!(row.kind, Kind::Play(_)));
        Field::ALL.into_iter().all(|field| {
            let Some(criterion) = self.selected[field.index()].as_ref() else {
                return true;
            };
            match (criterion, field.value(row)) {
                (Criterion::Known { key, .. }, Some(value)) => normalise(value) == *key,
                (Criterion::Unknown, None) => true,
                _ => false,
            }
        })
    }
}

/// Build a chooser from the complete, unfiltered playable rows.
///
/// `None` means the source supplies no known value for this field. A field
/// where every game is missing metadata is shown as Unavailable rather than
/// opening a list containing only Any and Unknown.
pub fn choices(rows: &[Row], field: Field) -> Option<Vec<Choice>> {
    let mut known = BTreeMap::<String, String>::new();
    let mut missing = false;
    for row in rows {
        if !matches!(row.kind, Kind::Play(_)) {
            continue;
        }
        match field.value(row) {
            Some(value) => {
                known
                    .entry(normalise(value))
                    .or_insert_with(|| value.to_string());
            }
            None => missing = true,
        }
    }
    if known.is_empty() {
        return None;
    }
    let mut values = Vec::with_capacity(known.len() + 2);
    values.push(Choice::Any);
    values.extend(
        known
            .into_iter()
            .map(|(key, label)| Choice::Known { label, key }),
    );
    if missing {
        values.push(Choice::Unknown);
    }
    Some(values)
}

/// The same four-digit prefix already presented in Degauss's compact details.
pub fn presented_year(released: &str) -> Option<&str> {
    released
        .trim()
        .get(..4)
        .filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
}

fn normalise(value: &str) -> String {
    value.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::browse::{Details, Launch};

    fn game(name: &str) -> Row {
        Row {
            name: name.to_string(),
            sort_key: name.to_lowercase(),
            kind: Kind::Play(Launch::File(PathBuf::from(name))),
            cover: None,
            genre: None,
            favorite: false,
            below: None,
            details: Details::default(),
        }
    }

    #[test]
    fn choices_are_distinct_trimmed_case_insensitive_and_keep_original_text() {
        let mut rows = vec![game("one"), game("two"), game("three")];
        rows[0].genre = Some("  Shoot 'em up ".to_string());
        rows[1].genre = Some("shoot 'EM UP".to_string());

        assert_eq!(
            choices(&rows, Field::Genre),
            Some(vec![
                Choice::Any,
                Choice::Known {
                    label: "Shoot 'em up".to_string(),
                    key: "shoot 'em up".to_string(),
                },
                Choice::Unknown,
            ])
        );
    }

    #[test]
    fn all_missing_is_unavailable_and_year_uses_presented_prefix() {
        let mut rows = vec![game("one"), game("two")];
        assert_eq!(choices(&rows, Field::Publisher), None);
        rows[0].details.released = "1994-12-03".to_string();
        rows[1].details.released = "not known".to_string();
        assert_eq!(
            choices(&rows, Field::Year),
            Some(vec![
                Choice::Any,
                Choice::Known {
                    label: "1994".to_string(),
                    key: "1994".to_string(),
                },
                Choice::Unknown,
            ])
        );
    }

    #[test]
    fn fields_combine_with_and_and_unknown_matches_only_missing_values() {
        let mut one = game("one");
        one.genre = Some("Action".to_string());
        one.details.players = "2".to_string();
        let mut two = game("two");
        two.genre = Some("Action".to_string());

        let mut filters = Filters::default();
        filters.choose(
            Field::Genre,
            &Choice::Known {
                label: "Action".to_string(),
                key: "action".to_string(),
            },
        );
        filters.choose(Field::Players, &Choice::Unknown);

        assert!(!filters.matches(&one));
        assert!(filters.matches(&two));
    }

    #[test]
    fn resolved_metadata_is_filtered_without_regard_to_launch_or_favourite_identity() {
        let mut archived = game("archived");
        archived.kind = Kind::Play(Launch::File(PathBuf::from("games.zip/archived.rom")));
        archived.genre = Some("Action".to_string());
        archived.favorite = true;
        let mut virtual_title = game("virtual");
        virtual_title.kind = Kind::Play(Launch::AmigaVision {
            install: PathBuf::from("AmigaVision.vhd"),
            title: "virtual".to_string(),
        });
        virtual_title.genre = Some("action".to_string());
        let rows = [archived, virtual_title];

        assert_eq!(
            choices(&rows, Field::Genre),
            Some(vec![
                Choice::Any,
                Choice::Known {
                    label: "Action".to_string(),
                    key: "action".to_string(),
                },
            ])
        );
        let mut filters = Filters::default();
        filters.choose(
            Field::Genre,
            &Choice::Known {
                label: "Action".to_string(),
                key: "action".to_string(),
            },
        );
        assert!(rows.iter().all(|row| filters.matches(row)));
    }
}
