//! Neo Geo ROM sets, listed the way MiSTer's own menu lists them.
//!
//! The Neo Geo core takes three kinds of game: a `.neo` file, a zipped ROM
//! set (`mslug.zip`) and an unzipped one (`mslug/`, a folder of raw ROM
//! components). Main tells the sets apart from ordinary archives and
//! folders with `romsets.xml`, the catalogue shipped beside the core: an
//! entry there turns the ZIP or folder of that name into one game, named by
//! the catalogue, and the complete path is handed to the Neo Geo loader.
//! Nothing inside the set is opened until launch, when Main validates and
//! loads the components itself.
//!
//! Everything here is read from Main's `neogeo_scan_xml` and
//! `neogeo_get_altname` rather than invented: the tag and attribute names
//! compare without regard to case, a `name` holds comma-separated aliases,
//! `hide` takes an entry out of the list, the first entry in document order
//! answers for a name, and a folder carrying its own `romset.xml` is a game
//! before the catalogue is consulted at all.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::config::{LaunchRule, SystemConfig};
use crate::error::{DegaussError, Result};

/// The shipped catalogue is well under a hundred kilobytes; a file past this
/// is not one.
const MAX_CATALOGUE_BYTES: u64 = 8 * 1024 * 1024;

/// True for a system whose games are Neo Geo ROM sets: the Neo Geo core
/// with `.neo` among its extensions, which is what the shipped Neo Geo and
/// Neo Geo MVS rows are. Neo Geo CD runs the same core on disc images and
/// is not one, and the answer comes from the system's own definition, so a
/// table edited before this existed keeps working without a new field.
pub fn is_romset_system(config: &SystemConfig) -> bool {
    let core = Path::new(&config.rbf)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    crate::core_variants::same_core_identity(core, "NeoGeo")
        && config
            .extensions
            .iter()
            .any(|extension| extension.eq_ignore_ascii_case("neo"))
}

/// One `<romset>` of the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RomsetEntry {
    /// The on-disk names this entry answers for, in the order written.
    names: Vec<String>,
    /// The title Main shows. Absent when the entry carries none.
    altname: Option<String>,
    hidden: bool,
}

/// What a catalogue makes of a name found on the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recognition {
    /// One game, shown under this title.
    Game(String),
    /// A set the catalogue says not to list.
    Hidden,
    /// Not a set: an ordinary folder or archive.
    Unrecognised,
}

/// A parsed `romsets.xml`.
#[derive(Debug, Default)]
pub struct Catalogue {
    entries: Vec<RomsetEntry>,
    /// Lowercased alias to the entry and the position of the alias within
    /// it. Filled in document order and never overwritten, so the first
    /// entry naming an alias is the one that answers for it, as in Main.
    by_alias: HashMap<String, (usize, usize)>,
}

impl Catalogue {
    /// Parse a catalogue. Malformed XML is an error: the difference between
    /// "there are no sets here" and "the catalogue is broken" is exactly
    /// what a card owner needs to be told.
    pub fn read(path: &Path) -> Result<Catalogue> {
        let mut catalogue = Catalogue::default();
        read_romset_nodes(path, "romsets.xml", |node| {
            // Without a name there is nothing on the card an entry could
            // answer for, and Main never matches one either.
            let Some(name) = node.name else {
                return;
            };
            catalogue.push(RomsetEntry {
                names: name.split(',').map(str::to_string).collect(),
                altname: node.altname.filter(|title| !title.is_empty()),
                hidden: node.hidden,
            });
        })?;
        Ok(catalogue)
    }

    fn push(&mut self, entry: RomsetEntry) {
        let index = self.entries.len();
        for (position, alias) in entry.names.iter().enumerate() {
            if alias.is_empty() {
                continue;
            }
            self.by_alias
                .entry(alias.to_ascii_lowercase())
                .or_insert((index, position));
        }
        self.entries.push(entry);
    }

    /// What Main would make of a ZIP or folder of this name.
    ///
    /// The title is the entry's `altname`; a later alias of a multi-name
    /// entry is shown as "Title (alias)", exactly as Main prints it, so the
    /// variants of one game stay distinguishable. An entry with no title
    /// keeps the name on the card rather than inventing one.
    pub fn recognise(&self, set_name: &str) -> Recognition {
        let Some(&(index, position)) = self.by_alias.get(&set_name.to_ascii_lowercase()) else {
            return Recognition::Unrecognised;
        };
        let entry = &self.entries[index];
        if entry.hidden {
            return Recognition::Hidden;
        }
        Recognition::Game(match (&entry.altname, position) {
            (None, _) => set_name.to_string(),
            (Some(title), 0) => title.clone(),
            (Some(title), _) => format!("{title} ({set_name})"),
        })
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The attributes of one `<romset>` this reads. Anything else the node
/// carries (chip flags, offsets, the loader's file list) is the loader's.
struct RomsetNode {
    name: Option<String>,
    altname: Option<String>,
    hidden: bool,
}

/// Walk every `<romset>` of a file, handing each to `node` in document
/// order. Malformed XML is an error, an unclosed element included: a file
/// cut short must not pass for a shorter catalogue.
fn read_romset_nodes(
    path: &Path,
    what: &'static str,
    mut node: impl FnMut(RomsetNode),
) -> Result<()> {
    let bad = |detail: String| DegaussError::malformed(what, path, detail);
    let bytes = read_bounded(path, what)?;
    let mut reader = Reader::from_reader(bytes.as_slice());
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer) {
            Err(error) => {
                return Err(bad(format!(
                    "at position {}: {error}",
                    reader.buffer_position()
                )));
            }
            Ok(Event::Eof) => break,
            Ok(Event::End(_)) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| bad("unexpected closing element".to_string()))?;
            }
            Ok(Event::Start(event)) => {
                depth += 1;
                if let Some(found) = romset_node(&event).map_err(&bad)? {
                    node(found);
                }
            }
            Ok(Event::Empty(event)) => {
                if let Some(found) = romset_node(&event).map_err(&bad)? {
                    node(found);
                }
            }
            Ok(_) => {}
        }
        buffer.clear();
    }
    if depth != 0 {
        return Err(bad("unclosed XML element".to_string()));
    }
    Ok(())
}

/// The `<romset>` a start or empty element is, if it is one.
fn romset_node(
    event: &quick_xml::events::BytesStart<'_>,
) -> std::result::Result<Option<RomsetNode>, String> {
    if !event.name().as_ref().eq_ignore_ascii_case("romset") {
        return Ok(None);
    }
    let mut found = RomsetNode {
        name: None,
        altname: None,
        hidden: false,
    };
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        let key = attribute.key.as_ref();
        if key.eq_ignore_ascii_case("name") {
            found.name = Some(attribute_value(&attribute)?);
        } else if key.eq_ignore_ascii_case("altname") {
            found.altname = Some(attribute_value(&attribute)?);
        } else if key.eq_ignore_ascii_case("hide") {
            // Main only asks whether the attribute is there.
            found.hidden = true;
        }
    }
    Ok(Some(found))
}

fn attribute_value(
    attribute: &quick_xml::events::attributes::Attribute<'_>,
) -> std::result::Result<String, String> {
    attribute
        .normalized_value(XmlVersion::Implicit1_0)
        .map(|value| value.into_owned())
        .map_err(|error| error.to_string())
}

fn read_bounded(path: &Path, what: &'static str) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).map_err(|error| DegaussError::io(what, path, error))?;
    let mut bytes = Vec::new();
    file.take(MAX_CATALOGUE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| DegaussError::io(what, path, error))?;
    if bytes.len() as u64 > MAX_CATALOGUE_BYTES {
        return Err(DegaussError::unsupported(
            what,
            format!(
                "{} is larger than {MAX_CATALOGUE_BYTES} bytes",
                path.display()
            ),
        ));
    }
    Ok(bytes)
}

/// The title a folder's own `romset.xml` gives it, if the folder has one.
///
/// Main reads this before it looks at the catalogue: a folder carrying the
/// file is one game whatever the catalogue says, even a hidden entry. The
/// title is the `altname` of the last `<romset>` in the file, or its `name`
/// when there is no `altname`, which is what Main's reader is left holding.
/// [`None`] when there is no file or it names nothing; an unreadable or
/// malformed file is an error for the caller to report.
pub fn romset_title(dir: &Path) -> Result<Option<String>> {
    let path = dir.join("romset.xml");
    if !path.is_file() {
        return Ok(None);
    }
    let mut title = String::new();
    read_romset_nodes(&path, "romset.xml", |node| {
        if let Some(value) = node.altname.or(node.name) {
            title = value;
        }
    })?;
    Ok((!title.is_empty()).then_some(title))
}

/// The catalogues one system's folders answer to, read once each.
///
/// Main reads the catalogue of the folder being scanned when it has one,
/// and the system folder's otherwise. The root catalogues are read when
/// the library is opened; a sub-folder's own is read the first time that
/// folder is listed and kept, so listing costs one lookup per entry rather
/// than a parse. Every file that could not be read is remembered, once, so
/// the audit can name it.
pub struct Catalogues {
    roots: Vec<(PathBuf, Arc<Catalogue>)>,
    /// A sub-folder's own catalogue, or [`None`] once it is known to have
    /// none and answers to its root's.
    local: RefCell<BTreeMap<PathBuf, Option<Arc<Catalogue>>>>,
    problems: RefCell<Vec<(PathBuf, String)>>,
}

impl Catalogues {
    /// Read the catalogue at the top of each declared folder. A folder
    /// without one is normal: a library of `.neo` files needs none.
    pub fn open<'a>(roots: impl IntoIterator<Item = &'a Path>) -> Catalogues {
        let catalogues = Catalogues {
            roots: Vec::new(),
            local: RefCell::new(BTreeMap::new()),
            problems: RefCell::new(Vec::new()),
        };
        let roots = roots
            .into_iter()
            .map(|root| {
                let catalogue = catalogues
                    .read(&root.join("romsets.xml"))
                    .unwrap_or_default();
                (root.to_path_buf(), catalogue)
            })
            .collect();
        Catalogues {
            roots,
            ..catalogues
        }
    }

    /// The catalogue at `path`, or [`None`] when there is no file there.
    /// A file that cannot be read is reported and answers as empty, so
    /// every `.neo` and `.mgl` beside it is still listed.
    fn read(&self, path: &Path) -> Option<Arc<Catalogue>> {
        if !path.is_file() {
            return None;
        }
        Some(Arc::new(match Catalogue::read(path) {
            Ok(catalogue) => catalogue,
            Err(error) => {
                self.record_problem(path, error.to_string());
                Catalogue::default()
            }
        }))
    }

    /// The catalogue that answers for the entries of `dir`, which sits in
    /// the declared folder `root`.
    pub fn for_dir(&self, dir: &Path, root: Option<usize>) -> Arc<Catalogue> {
        if let Some((_, catalogue)) = self.roots.iter().find(|(path, _)| path == dir) {
            return catalogue.clone();
        }
        let known = self.local.borrow().get(dir).cloned();
        let own = match known {
            Some(known) => known,
            None => {
                let read = self.read(&dir.join("romsets.xml"));
                self.local
                    .borrow_mut()
                    .insert(dir.to_path_buf(), read.clone());
                read
            }
        };
        own.or_else(|| root.map(|index| self.roots[index].1.clone()))
            .unwrap_or_default()
    }

    /// What one entry of a listed folder is: a game, a set kept out of the
    /// list, or nothing the catalogue knows about.
    ///
    /// A folder carrying its own `romset.xml` is a game before the
    /// catalogue is asked, as in Main; a broken one is reported and the
    /// folder is then judged by the catalogue like any other. A ZIP is
    /// looked up by its name without the extension, and is never opened.
    pub fn classify(
        &self,
        catalogue: &Catalogue,
        path: &Path,
        name: &str,
        is_dir: bool,
    ) -> Recognition {
        if is_dir {
            match romset_title(path) {
                Ok(Some(title)) => return Recognition::Game(title),
                Ok(None) => {}
                Err(error) => self.record_problem(&path.join("romset.xml"), error.to_string()),
            }
            return catalogue.recognise(name);
        }
        catalogue.recognise(&name[..name.len() - ".zip".len()])
    }

    /// Every catalogue file that could not be read, with the reason.
    pub fn problems(&self) -> Vec<(PathBuf, String)> {
        self.problems.borrow().clone()
    }

    fn record_problem(&self, path: &Path, reason: String) {
        let mut problems = self.problems.borrow_mut();
        if problems.iter().any(|(known, _)| known == path) {
            return;
        }
        crate::note(&format!("neogeo       {}: {reason}", path.display()));
        problems.push((path.to_path_buf(), reason));
    }
}

/// The rule that starts a ROM set, where the system's own rules cover
/// only extensions.
///
/// A set has no extension of its own (a folder) or one no rule names (a
/// ZIP), and Main loads both through the same file slot as a `.neo`: an
/// MGL `<file>` for the Neo Geo core goes to the ROM-set loader whatever
/// it points at. So a set borrows the `.neo` rule. [`None`] for every
/// other system, and whenever a rule already covers the file.
pub fn romset_rule<'a>(system: &'a SystemConfig, game: &Path) -> Option<&'a LaunchRule> {
    if !is_romset_system(system) || system.rule_for(game).is_some() {
        return None;
    }
    system.launch.iter().find(|rule| {
        rule.extensions
            .iter()
            .any(|extension| extension.eq_ignore_ascii_case("neo"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "degauss-neogeo-{tag}-{}-{:p}",
            std::process::id(),
            &tag
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn catalogue(xml: &str) -> Catalogue {
        let dir = temp("catalogue");
        let path = dir.join("romsets.xml");
        std::fs::write(&path, xml).unwrap();
        let read = Catalogue::read(&path);
        std::fs::remove_dir_all(&dir).ok();
        read.expect("a readable catalogue")
    }

    fn shipped(id: &str) -> SystemConfig {
        let table = crate::systems::parse_table(
            include_str!("../assets/systems.toml"),
            Path::new("systems.toml"),
        )
        .unwrap();
        let def = table.into_iter().find(|system| system.id == id).unwrap();
        crate::systems::FoundSystem {
            def,
            paths: vec![PathBuf::from("/games")],
            logo_dir: None,
            menu_folder: None,
        }
        .to_config()
    }

    #[test]
    fn the_catalogue_is_read_the_way_main_reads_it() {
        // The shipped file opens with a comment, mixes empty and nested
        // elements, escapes ampersands and carries attributes the loader
        // uses and this never will. Tag and attribute names are compared
        // without regard to case, as Main compares them.
        let catalogue = catalogue(
            r#"<!-- placed in the NeoGeo dir -->
            <romsets>
              <ROMSET NAME="aof" pcm="1" ALTNAME="Art of Fighting" altnamej="Ryuuko no Ken">
                <file name="044-p1.p1" type="P" index="4"/>
              </ROMSET>
              <romset name="quizdai2" altname="Quiz Meitantei Neo &amp; Geo"/>
              <romset name="burningfh,brningfh" altname="Burning Fight (US)"/>
              <romset name="secret" altname="Secret" hide="1"/>
              <romset name="noname"/>
              <romset name="blank" altname=""/>
              <romset altname="Nameless"/>
            </romsets>"#,
        );
        assert_eq!(catalogue.len(), 6, "an entry without a name is nothing");
        assert_eq!(
            catalogue.recognise("AOF"),
            Recognition::Game("Art of Fighting".into())
        );
        assert_eq!(
            catalogue.recognise("quizdai2"),
            Recognition::Game("Quiz Meitantei Neo & Geo".into())
        );
        assert_eq!(catalogue.recognise("secret"), Recognition::Hidden);
        assert_eq!(catalogue.recognise("kof98"), Recognition::Unrecognised);
        // No title means the name on the card, not Main's literal "No
        // name": a game called "No name" cannot be told from the next one.
        assert_eq!(
            catalogue.recognise("noname"),
            Recognition::Game("noname".into())
        );
        assert_eq!(
            catalogue.recognise("blank"),
            Recognition::Game("blank".into())
        );
    }

    #[test]
    fn a_later_alias_is_titled_altname_then_set_name_as_main_does() {
        // The shipped catalogue lists the MAME name and the Darksoft name
        // of one set together. Main names the first plainly and the others
        // "Title (name)", which is how two folders holding the same game
        // stay distinguishable in one list.
        let catalogue = catalogue(
            r#"<romsets><romset name="burningfh,brningfh,BRNFH" altname="Burning Fight (US)"/></romsets>"#,
        );
        assert_eq!(
            catalogue.recognise("burningfh"),
            Recognition::Game("Burning Fight (US)".into())
        );
        assert_eq!(
            catalogue.recognise("BrningFH"),
            Recognition::Game("Burning Fight (US) (BrningFH)".into())
        );
        assert_eq!(
            catalogue.recognise("brnfh"),
            Recognition::Game("Burning Fight (US) (brnfh)".into())
        );
    }

    #[test]
    fn the_first_entry_in_document_order_wins_a_duplicate_alias() {
        // Main walks the entries top to bottom and stops at the first
        // match, so a name claimed twice is answered the same way every
        // time rather than by whichever entry a map happened to keep.
        let catalogue = catalogue(
            r#"<romsets>
              <romset name="twice" altname="First"/>
              <romset name="other,twice" altname="Second" hide="1"/>
            </romsets>"#,
        );
        assert_eq!(
            catalogue.recognise("twice"),
            Recognition::Game("First".into())
        );
        assert_eq!(catalogue.recognise("other"), Recognition::Hidden);
    }

    #[test]
    fn a_broken_catalogue_is_an_error_not_an_empty_list() {
        let dir = temp("broken");
        let path = dir.join("romsets.xml");
        std::fs::write(&path, "<romsets><romset name=\"aof\" altname=\"x\">").unwrap();
        let error = Catalogue::read(&path).expect_err("must fail");
        assert!(error.to_string().contains("romsets.xml"), "got: {error}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folders_own_romset_xml_names_it_and_the_last_entry_wins() {
        let dir = temp("romset");
        std::fs::write(
            dir.join("romset.xml"),
            r#"<romset name="custom" altname="Custom Build"/>"#,
        )
        .unwrap();
        assert_eq!(romset_title(&dir).unwrap().as_deref(), Some("Custom Build"));
        // Main copies the name and then the altname into one buffer for
        // every node it meets, so the last node is what it is left with,
        // and a node without an altname leaves its name there.
        std::fs::write(
            dir.join("romset.xml"),
            r#"<romsets><romset name="first" altname="First"/><romset name="second"/></romsets>"#,
        )
        .unwrap();
        assert_eq!(romset_title(&dir).unwrap().as_deref(), Some("second"));
        std::fs::write(dir.join("romset.xml"), "<romsets></romsets>").unwrap();
        assert_eq!(romset_title(&dir).unwrap(), None);
        std::fs::remove_file(dir.join("romset.xml")).unwrap();
        assert_eq!(romset_title(&dir).unwrap(), None);
        std::fs::write(dir.join("romset.xml"), "<romset name=\"x\"").unwrap();
        assert!(romset_title(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folders_own_romset_xml_outranks_a_hidden_catalogue_entry() {
        // Main returns the folder's own title before it checks the
        // catalogue, hidden flag included: the file is the owner saying
        // "this is a game", and it is not overruled by a shared list.
        let dir = temp("precedence");
        std::fs::write(
            dir.join("romsets.xml"),
            r#"<romsets><romset name="own" altname="Hidden" hide="1"/></romsets>"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("own")).unwrap();
        std::fs::write(
            dir.join("own/romset.xml"),
            r#"<romset name="own" altname="Own Title"/>"#,
        )
        .unwrap();
        let catalogues = Catalogues::open([dir.as_path()]);
        let catalogue = catalogues.for_dir(&dir, Some(0));
        assert_eq!(
            catalogues.classify(&catalogue, &dir.join("own"), "own", true),
            Recognition::Game("Own Title".into())
        );
        assert_eq!(
            catalogues.classify(&catalogue, &dir.join("own.zip"), "own.zip", false),
            Recognition::Hidden,
            "a ZIP has no romset.xml of its own to outrank the catalogue"
        );
        assert!(catalogues.problems().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_sub_folders_own_catalogue_replaces_the_roots_and_is_read_once() {
        let dir = temp("sub");
        std::fs::write(
            dir.join("romsets.xml"),
            r#"<romsets><romset name="root" altname="Root Game"/></romsets>"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("gog")).unwrap();
        std::fs::create_dir_all(dir.join("plain")).unwrap();
        std::fs::write(
            dir.join("gog/romsets.xml"),
            r#"<romsets><romset name="gog" altname="GOG Game"/></romsets>"#,
        )
        .unwrap();
        let catalogues = Catalogues::open([dir.as_path()]);
        let gog = catalogues.for_dir(&dir.join("gog"), Some(0));
        assert_eq!(gog.recognise("gog"), Recognition::Game("GOG Game".into()));
        assert_eq!(
            gog.recognise("root"),
            Recognition::Unrecognised,
            "a folder's own catalogue replaces the root's rather than extending it"
        );
        // A folder without one answers to the root's catalogue.
        let plain = catalogues.for_dir(&dir.join("plain"), Some(0));
        assert_eq!(
            plain.recognise("root"),
            Recognition::Game("Root Game".into())
        );
        // Once read, the file is not consulted again for this library:
        // replacing it changes nothing until the library is reopened.
        std::fs::write(
            dir.join("gog/romsets.xml"),
            r#"<romsets><romset name="gog" altname="Changed"/></romsets>"#,
        )
        .unwrap();
        assert!(Arc::ptr_eq(
            &gog,
            &catalogues.for_dir(&dir.join("gog"), Some(0))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_catalogue_is_reported_once_and_answers_as_empty() {
        let dir = temp("reported");
        std::fs::write(dir.join("romsets.xml"), "<romsets><romset").unwrap();
        let catalogues = Catalogues::open([dir.as_path()]);
        let catalogue = catalogues.for_dir(&dir, Some(0));
        assert_eq!(catalogue.recognise("anything"), Recognition::Unrecognised);
        let problems = catalogues.problems();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].0, dir.join("romsets.xml"));
        assert!(
            problems[0].1.contains("romsets.xml"),
            "got: {}",
            problems[0].1
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_the_neo_geo_cartridge_systems_are_rom_set_systems() {
        assert!(is_romset_system(&shipped("NeoGeo")));
        assert!(is_romset_system(&shipped("NeoGeoMVS")));
        // Same core, disc images: the catalogue means nothing to it.
        assert!(!is_romset_system(&shipped("NeoGeoCD")));
        assert!(!is_romset_system(&shipped("NES")));
        // A user's table may carry a dated core reference.
        let mut dated = shipped("NeoGeo");
        dated.rbf = "_Console/NeoGeo_20240101".into();
        assert!(is_romset_system(&dated));
    }

    #[test]
    fn a_set_borrows_the_neo_rule_and_nothing_else_does() {
        let neogeo = shipped("NeoGeo");
        let rule =
            romset_rule(&neogeo, Path::new("/games/NEOGEO/mslug.zip")).expect("the neo rule");
        assert_eq!((rule.kind.as_str(), rule.index, rule.delay), ("f", 1, 1));
        assert!(romset_rule(&neogeo, Path::new("/games/NEOGEO/mslug")).is_some());
        // A file a rule already covers keeps that rule.
        assert!(romset_rule(&neogeo, Path::new("/games/NEOGEO/mslug.neo")).is_none());
        // Another system's ZIP is still refused rather than guessed at.
        assert!(romset_rule(&shipped("NES"), Path::new("/games/NES/game.zip")).is_none());
        assert!(romset_rule(&shipped("NeoGeoCD"), Path::new("/games/NEOGEO/mslug.zip")).is_none());
    }
}
