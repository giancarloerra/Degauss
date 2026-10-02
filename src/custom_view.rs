//! Named, proportional browser templates. There is one selection and one renderer.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Template {
    Columns,
    SplitRight,
    SplitLeft,
    Stacked,
}

impl Template {
    pub const ALL: [Self; 4] = [
        Self::Columns,
        Self::SplitRight,
        Self::SplitLeft,
        Self::Stacked,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Columns => "Two columns",
            Self::SplitRight => "Left + split right",
            Self::SplitLeft => "Split left + right",
            Self::Stacked => "Two stacked panels",
        }
    }
    pub fn count(self) -> usize {
        if matches!(self, Self::SplitRight | Self::SplitLeft) {
            3
        } else {
            2
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Content {
    List,
    Artwork,
    ShortInformation,
    FullInformation,
}

impl Content {
    pub const ALL: [Self; 4] = [
        Self::List,
        Self::Artwork,
        Self::ShortInformation,
        Self::FullInformation,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::List => "Browser list",
            Self::Artwork => "Artwork",
            Self::ShortInformation => "Short information",
            Self::FullInformation => "Full information",
        }
    }
    pub fn index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|value| *value == self)
            .expect("panel content") as i32
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    pub name: String,
    pub template: Template,
    pub panels: Vec<Content>,
    /// Divider position in percent, kept usable on small logical canvases.
    pub columns: u8,
    pub rows: u8,
}

impl Definition {
    pub fn new(template: Template) -> Self {
        let panels = if template.count() == 3 {
            vec![Content::List, Content::Artwork, Content::ShortInformation]
        } else {
            vec![Content::List, Content::Artwork]
        };
        Self {
            name: String::new(),
            template,
            panels,
            columns: 50,
            rows: 50,
        }
    }
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() || self.name.chars().count() > 255 {
            return Err("A custom view needs a name of 1 to 255 characters".into());
        }
        if self.panels.len() != self.template.count()
            || self
                .panels
                .iter()
                .filter(|content| **content == Content::List)
                .count()
                != 1
        {
            return Err(
                "A custom view must contain exactly one browser list and the template's panels"
                    .into(),
            );
        }
        if !(20..=80).contains(&self.columns) || !(20..=80).contains(&self.rows) {
            return Err("Custom view dividers must be between 20 and 80 percent".into());
        }
        Ok(())
    }
    pub fn choose(&mut self, panel: usize, content: Content) {
        let Some(previous) = self.panels.get(panel).copied() else {
            return;
        };
        if content == Content::List {
            if let Some(old) = self.panels.iter().position(|value| *value == Content::List) {
                self.panels[old] = previous;
            }
        } else if previous == Content::List {
            // Move the single list into the next panel, never remove it.
            let next = (panel + 1) % self.panels.len();
            self.panels[next] = Content::List;
        }
        self.panels[panel] = content;
    }
    pub fn has(&self, content: Content) -> bool {
        self.panels.contains(&content)
    }
    pub fn rectangles(&self, width: f32, height: f32, gap: f32) -> Vec<Rect> {
        let left = ((width - gap) * f32::from(self.columns) / 100.0).floor();
        let right = (width - gap - left).max(1.0);
        let top = ((height - gap) * f32::from(self.rows) / 100.0).floor();
        let bottom = (height - gap - top).max(1.0);
        match self.template {
            Template::Columns => vec![
                Rect::new(0.0, 0.0, left, height),
                Rect::new(left + gap, 0.0, right, height),
            ],
            Template::SplitRight => vec![
                Rect::new(0.0, 0.0, left, height),
                Rect::new(left + gap, 0.0, right, top),
                Rect::new(left + gap, top + gap, right, bottom),
            ],
            Template::SplitLeft => vec![
                Rect::new(0.0, 0.0, left, top),
                Rect::new(0.0, top + gap, left, bottom),
                Rect::new(left + gap, 0.0, right, height),
            ],
            Template::Stacked => vec![
                Rect::new(0.0, 0.0, width, top),
                Rect::new(0.0, top + gap, width, bottom),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Rect {
    fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width: width.max(1.0),
            height: height.max(1.0),
        }
    }
}

pub fn key(id: &str) -> String {
    format!("custom:{id}")
}
pub fn from_key<'a>(views: &'a BTreeMap<String, Definition>, key: &str) -> Option<&'a Definition> {
    views.get(key.strip_prefix("custom:")?)
}
pub fn validate(views: &BTreeMap<String, Definition>) -> std::result::Result<(), String> {
    for (id, view) in views {
        if id
            .strip_prefix('v')
            .and_then(|number| number.parse::<u64>().ok())
            .is_none()
        {
            return Err("Invalid custom view identity".into());
        }
        view.validate()?;
    }
    Ok(())
}
pub fn next_id(views: &BTreeMap<String, Definition>) -> Option<String> {
    views
        .keys()
        .filter_map(|id| id.strip_prefix('v')?.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .map(|id| format!("v{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moving_panel_contents_always_keeps_one_list() {
        for template in Template::ALL {
            let mut view = Definition::new(template);
            view.name = "My view".into();
            for panel in 0..template.count() {
                for content in Content::ALL {
                    view.choose(panel, content);
                    view.validate().unwrap();
                }
            }
        }
    }
    #[test]
    fn proportions_fill_the_same_safe_canvas_at_every_limit_and_orientation() {
        for (width, height) in [(316.0, 180.0), (216.0, 268.0), (1152.0, 648.0)] {
            for template in Template::ALL {
                for columns in [20, 50, 80] {
                    for rows in [20, 50, 80] {
                        let mut view = Definition::new(template);
                        view.columns = columns;
                        view.rows = rows;
                        let rectangles = view.rectangles(width, height, 4.0);
                        assert_eq!(rectangles.len(), template.count());
                        for rect in &rectangles {
                            assert!(rect.width > 0.0 && rect.height > 0.0);
                            assert!(rect.x + rect.width <= width && rect.y + rect.height <= height);
                        }
                        for (at, a) in rectangles.iter().enumerate() {
                            for b in rectangles.iter().skip(at + 1) {
                                assert!(
                                    a.x + a.width <= b.x
                                        || b.x + b.width <= a.x
                                        || a.y + a.height <= b.y
                                        || b.y + b.height <= a.y
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn saved_definitions_have_stable_identity_and_reject_invalid_shape() {
        let mut view = Definition::new(Template::Columns);
        view.name = "Games".into();
        let mut views = BTreeMap::from([("v4".into(), view.clone())]);
        assert_eq!(next_id(&views).as_deref(), Some("v5"));
        assert_eq!(from_key(&views, "custom:v4"), Some(&view));
        assert!(from_key(&views, "details").is_none());
        views.get_mut("v4").unwrap().panels = vec![Content::Artwork; 2];
        assert!(validate(&views).is_err());
    }
}
