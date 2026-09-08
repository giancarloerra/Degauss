//! Degauss: a fast game browser for MiSTer FPGA.
//!
//! Draws straight to the Linux framebuffer with a software renderer, reads
//! what is on the card, and launches games the way MiSTer itself does. It
//! runs no service and duplicates nothing the main menu already provides.
//! Its optional, user-started scraper can add artwork and metadata to the
//! existing EmulationStation gamelists on the card.

slint::include_modules!();

mod app;
mod artwork_pack;
mod browse;
mod cache;
mod category_images;
mod config;
mod core_choices;
mod core_variants;
mod covers;
mod error;
mod favorites;
mod font;
mod gamelist;
mod index_job;
mod input;
mod launch;
mod list_state;
mod metrics;
mod options;
mod provider_job;
mod render;
mod scraper;
mod settings;
mod source_cache;
mod state;
mod status;
mod surface;
mod systems;
mod theme;
mod theme_editor;
mod zip;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use slint::platform::software_renderer::RepaintBufferType;
use slint::{ComponentHandle, PhysicalSize};

#[cfg(target_os = "linux")]
use crate::app::Outcome;
use crate::app::{App, Layout, Loaded, Screen};
use crate::config::Config;
use crate::error::{DegaussError, Result};
use crate::metrics::StartupTimings;
use crate::render::{PresentMode, Presenter};
use crate::settings::Settings;
use crate::surface::{MemorySurface, PixelFormat, Surface};
use crate::systems::FoundSystem;

/// Long enough to be stable, short enough to finish on a slow machine.
const DEFAULT_SELFTEST_FRAMES: u32 = 600;

const USAGE: &str = "\
degauss - a fast game browser for MiSTer FPGA

  --config <path>     configuration file (default: degauss.toml beside the binary)
  --systems <path>    systems table (default: systems.toml beside the binary)
  --system <id>       open this system directly, e.g. C64
  --list-systems      print the systems found on this machine and exit
  --report            read one system, print what was found, and exit
  --audit             read every system and print a table of games and
                      artwork, so nothing has to be checked by hand
  --check-install     look over the installation itself and report what is
                      present, missing or half-migrated, then exit
  --dry-run-launch    print the MGL that would launch the first game, and exit
  --import-favorites <file>
                      write a favourite for each line of a list, in the
                      form MiSTer's own favourites script writes them.
                      Each line is folder, path, title, separated by tabs;
                      a title is only for AmigaVision, which has no file
  --render <file.bmp> draw one frame to an image instead of the screen
  --bench <frames>    scroll for N frames into memory and report the cost
  --selftest          compare both drawing paths against the real framebuffer
  --frames <n>        frames per run for --bench and --selftest
  --layout <name>     details (default), tiled, list, carousel, multi-list
                      or gallery
  --screen <name>     browse (default), menu, options, advanced, theme-editor,
                      help, about, splash, find, context, category-image,
                      screensaver, scraper, scraper-keyboard,
                      scraper-progress, game-data-source,
                      artwork-pack-location, artwork-pack-directory or
                      source-progress, for --render
  --select <n>        which entry to highlight in --render
  --find <text>       a search already typed, with --screen find
  --geometry <WxH>    geometry for --render and --bench
  --format <fmt>      rgb565 or xrgb8888, for --render and --bench
  --device <path>     framebuffer device (default /dev/fb0)
  --present <mode>    direct or staged (default depends on framebuffer mapping)
  --help              this text

With no flags it takes over the framebuffer and browses.

  up / down     move
  left / right  scroll speed, 0.5x to 12x
  enter         open a system, or launch a game
  escape        back
  tab           this folder: random, favourites, letter, search, view
  space         menu: options, help, about, exit
";

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn default_present_mode(mapping_source: &str) -> PresentMode {
    if mapping_source == "/dev/mem fallback" {
        PresentMode::Staged
    } else {
        PresentMode::Direct
    }
}

fn main() -> ExitCode {
    // The release build aborts on panic, which skips destructors, so the
    // console is restored here as well as in the guards. Leaving a terminal
    // unusable would be worse than failing.
    let previous = std::panic::take_hook();
    crate::input::install_signal_handlers();
    std::panic::set_hook(Box::new(move |info| {
        crate::input::restore_terminal();
        crate::input::restore_console();
        previous(info);
    }));

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            crate::input::restore_terminal();
            crate::input::restore_console();
            eprintln!("degauss: {e}");
            let mut source = std::error::Error::source(&e);
            while let Some(inner) = source {
                eprintln!("  caused by: {inner}");
                source = inner.source();
            }
            ExitCode::FAILURE
        }
    }
}

struct Args {
    config: Option<PathBuf>,
    systems: Option<PathBuf>,
    system: Option<String>,
    list_systems: bool,
    report: bool,
    audit: bool,
    check_install: bool,
    dry_run_launch: bool,
    render: Option<PathBuf>,
    bench: Option<u32>,
    selftest: Option<u32>,
    geometry: (u32, u32),
    format: PixelFormat,
    device: PathBuf,
    /// Only set by `--present`. Absent means the saved setting stands.
    present: Option<PresentMode>,
    /// A list of favourites to write, one per line.
    import_favorites: Option<PathBuf>,
    /// Only set by `--layout`. Absent means the user's saved view stands.
    layout: Option<Layout>,
    screen: Screen,
    select: usize,
    /// A search to have already typed, for `--render`.
    find: Option<String>,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            config: None,
            systems: None,
            system: None,
            list_systems: false,
            report: false,
            audit: false,
            check_install: false,
            dry_run_launch: false,
            render: None,
            bench: None,
            selftest: None,
            geometry: (640, 240),
            format: PixelFormat::Rgb565,
            device: PathBuf::from("/dev/fb0"),
            present: None,
            import_favorites: None,
            layout: None,
            screen: Screen::Browse,
            select: 0,
            find: None,
        }
    }
}

fn parse_args() -> std::result::Result<Option<Args>, String> {
    parse_from(std::env::args().skip(1))
}

/// The argument parser proper, over any sequence of words, so the rules it
/// encodes can be tested without a process to hand them to.
fn parse_from<I: Iterator<Item = String>>(argv: I) -> std::result::Result<Option<Args>, String> {
    let mut args = Args::default();
    let mut argv = argv;
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(None),
            "--report" => args.report = true,
            "--audit" => args.audit = true,
            "--check-install" => args.check_install = true,
            "--list-systems" => args.list_systems = true,
            "--dry-run-launch" => args.dry_run_launch = true,
            "--selftest" => args.selftest = Some(DEFAULT_SELFTEST_FRAMES),
            "--config" => args.config = Some(PathBuf::from(next(&mut argv, "--config")?)),
            "--systems" => args.systems = Some(PathBuf::from(next(&mut argv, "--systems")?)),
            "--system" => args.system = Some(next(&mut argv, "--system")?),
            "--import-favorites" => {
                args.import_favorites = Some(PathBuf::from(next(&mut argv, "--import-favorites")?))
            }
            "--render" => args.render = Some(PathBuf::from(next(&mut argv, "--render")?)),
            "--device" => args.device = PathBuf::from(next(&mut argv, "--device")?),
            "--bench" => {
                let value = next(&mut argv, "--bench")?;
                args.bench = Some(
                    value
                        .parse()
                        .map_err(|_| format!("bad frame count {value:?}"))?,
                );
            }
            "--frames" => {
                let value = next(&mut argv, "--frames")?;
                let frames = value
                    .parse()
                    .map_err(|_| format!("bad frame count {value:?}"))?;
                if args.selftest.is_some() {
                    args.selftest = Some(frames);
                } else if args.bench.is_some() {
                    args.bench = Some(frames);
                } else {
                    return Err("--frames needs --bench or --selftest before it".to_string());
                }
            }
            "--find" => {
                args.find = Some(next(&mut argv, "--find")?);
            }
            "--select" => {
                let value = next(&mut argv, "--select")?;
                args.select = value.parse().map_err(|_| format!("bad index {value:?}"))?;
            }
            "--screen" => {
                args.screen = match next(&mut argv, "--screen")?.as_str() {
                    "browse" => Screen::Browse,
                    "menu" => Screen::Menu,
                    "options" => Screen::Options,
                    "advanced" => Screen::Advanced,
                    "theme-editor" => Screen::ThemeEditor,
                    "help" => Screen::Help,
                    "about" => Screen::About,
                    "find" => Screen::Find,
                    "context" => Screen::Context,
                    "category-image" => Screen::CategoryImage,
                    "splash" => Screen::Splash,
                    "screensaver" => Screen::Screensaver,
                    "scraper" => Screen::Scraper,
                    "scraper-keyboard" => Screen::ScraperKeyboard,
                    "scraper-progress" => Screen::ScraperProgress,
                    "game-data-source" => Screen::GameDataSource,
                    "artwork-pack-location" => Screen::ArtworkPackLocation,
                    "artwork-pack-directory" => Screen::ArtworkPackDirectory,
                    "source-progress" => Screen::SourceProgress,
                    other => return Err(format!("unknown screen {other:?}")),
                }
            }
            "--layout" => {
                let value = next(&mut argv, "--layout")?;
                args.layout =
                    Some(Layout::parse(&value).ok_or_else(|| format!("unknown layout {value:?}"))?);
            }
            "--present" => {
                args.present = match next(&mut argv, "--present")?.as_str() {
                    "direct" => Some(PresentMode::Direct),
                    "staged" => Some(PresentMode::Staged),
                    other => return Err(format!("unknown drawing path {other:?}")),
                }
            }
            "--geometry" => {
                let value = next(&mut argv, "--geometry")?;
                let (w, h) = value
                    .split_once('x')
                    .ok_or_else(|| format!("--geometry wants WxH, got {value:?}"))?;
                args.geometry = (
                    w.parse().map_err(|_| format!("bad width in {value:?}"))?,
                    h.parse().map_err(|_| format!("bad height in {value:?}"))?,
                );
            }
            "--format" => {
                args.format = match next(&mut argv, "--format")?.as_str() {
                    "rgb565" => PixelFormat::Rgb565,
                    "xrgb8888" => PixelFormat::Xrgb8888,
                    other => return Err(format!("unknown format {other:?}")),
                }
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(Some(args))
}

fn next(
    argv: &mut impl Iterator<Item = String>,
    flag: &str,
) -> std::result::Result<String, String> {
    argv.next().ok_or_else(|| format!("{flag} needs a value"))
}

/// Configuration sits beside the binary, so the whole program is one folder
/// that can be copied to a card and deleted again.
fn beside_binary(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Which machine this binary was built for, for the first line of the log.
fn build_description() -> &'static str {
    if cfg!(target_arch = "arm") {
        "MiSTer ARM"
    } else {
        "development host"
    }
}

fn load_everything(args: &Args) -> Result<Loaded> {
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| beside_binary("degauss.toml"));
    let config = Config::load(&config_path)?;

    let settings_path = config_path
        .parent()
        .map(|dir| dir.join("settings.toml"))
        .unwrap_or_else(|| PathBuf::from("settings.toml"));
    let settings = Settings::load(&settings_path)?;

    let systems_path = args
        .systems
        .clone()
        .unwrap_or_else(|| beside_binary("systems.toml"));
    let table = systems::prepare_table(
        systems::load_table(&systems_path)?,
        Path::new(&config.menu_root),
    )?;

    let roots: Vec<PathBuf> = config.game_roots.iter().map(PathBuf::from).collect();
    // Logos live beside the configuration, named after the system.
    let logo_dir = config_path.parent().map(|dir| dir.join("logos"));
    // Themes too, so wherever the configuration goes, its themes follow.
    let themes_dir = config_path
        .parent()
        .map(|dir| dir.join("themes"))
        .unwrap_or_else(|| PathBuf::from("themes"));
    let themes = theme::load_available(&themes_dir);
    // Which group each system belongs to comes from where its core
    // actually is on this card, not from what the table guessed.
    let cores = systems::CoreIndex::read(Path::new(&config.menu_root));
    let systems = systems::discover_checked(&table, &roots, logo_dir.as_deref(), &cores)?;
    // The names the stock menu shows for cores, arcade boards and
    // shortcuts, when the card carries the file that defines them.
    let names = browse::DisplayNames::read(&Path::new(&config.menu_root).join("names.txt"));

    Ok(Loaded {
        config,
        settings,
        settings_path,
        systems,
        names,
        table,
        logo_dir,
        themes_dir,
        themes,
    })
}

/// Look over the installation itself: what is present, what is missing,
/// what a half-finished upgrade left behind. Reads everything, loads
/// nothing into a UI, refuses nothing: the whole point is to keep working
/// on the installation that does not come up.
fn select_system(systems: &[FoundSystem], name: &str) -> Result<usize> {
    if let Some(index) = systems.iter().position(|system| {
        system.def.id.eq_ignore_ascii_case(name) || system.name().eq_ignore_ascii_case(name)
    }) {
        return Ok(index);
    }
    let aliases: Vec<usize> = systems
        .iter()
        .enumerate()
        .filter_map(|(index, system)| {
            system
                .def
                .folders
                .iter()
                .any(|folder| {
                    Path::new(folder)
                        .file_name()
                        .is_some_and(|part| part.eq_ignore_ascii_case(name))
                })
                .then_some(index)
        })
        .collect();
    match aliases.as_slice() {
        [index] => Ok(*index),
        [] => Err(DegaussError::unsupported(
            "system",
            format!("{name:?} is not on this machine"),
        )),
        _ => Err(DegaussError::unsupported(
            "system",
            format!(
                "{name:?} is ambiguous; use {}",
                aliases
                    .iter()
                    .map(|&i| systems[i].def.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// Diagnostic launches share ownership and current virtual-target checks with
/// the interactive path so their reported core choice is the one users select.
fn diagnostic_launch_plan(
    loaded: &Loaded,
    system: &FoundSystem,
    path: &Path,
    mgl: &Path,
) -> Result<launch::LaunchPlan> {
    if zip::split_member_path(path).is_some() {
        zip::validate_member_for_launch(path)?;
    }
    let reference = if systems::is_favorites(system.category()) {
        favorites::reference_of_with_systems(path, &loaded.systems)
    } else {
        None
    };
    if let Some(reference) = &reference {
        if zip::split_member_path(&reference.owner_target).is_some() {
            zip::validate_member_for_launch(&reference.owner_target)?;
        }
    }
    let owner = if systems::is_favorites(system.category()) {
        reference
            .and_then(|reference| app::owner_of_favorite(&loaded.systems, &reference))
            .and_then(|id| {
                loaded
                    .systems
                    .iter()
                    .find(|candidate| candidate.def.id == id)
            })
            .unwrap_or(system)
    } else {
        system
    };
    launch::plan_with_choice(
        &owner.to_config(),
        path,
        mgl,
        Path::new(&loaded.config.menu_root),
        loaded.settings.core_preference.unwrap_or_default()
            == settings::CorePreference::RetroAchievementsFirst,
        loaded
            .settings
            .core_choices
            .get(&owner.def.id)
            .map(String::as_str),
    )
}

fn check_install(config_path: &Path) -> Result<()> {
    let dir = config_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut problems: Vec<String> = Vec::new();

    println!("build        {}", build_description());
    println!("folder       {}", dir.display());

    let binary = dir.join("degauss");
    match std::fs::metadata(&binary) {
        Ok(meta) => {
            #[cfg(unix)]
            let runnable = {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let runnable = true;
            if runnable {
                println!("binary       present, {} bytes", meta.len());
            } else {
                println!("binary       PRESENT BUT NOT EXECUTABLE");
                problems.push("the binary is not executable: chmod +x it".into());
            }
        }
        Err(_) => {
            println!("binary       MISSING");
            problems.push("no binary in the folder: the install did not finish".into());
        }
    }

    match std::fs::read_to_string(config_path) {
        Ok(text) => match Config::parse(&text, config_path) {
            Ok(_) => println!("degauss.toml present, parses"),
            Err(e) => {
                println!("degauss.toml PRESENT BUT BROKEN");
                problems.push(format!("degauss.toml does not parse: {e}"));
            }
        },
        Err(_) => {
            println!("degauss.toml MISSING");
            problems.push("degauss.toml is missing: Degauss will not start without it".into());
        }
    }

    let systems_path = dir.join("systems.toml");
    match std::fs::read_to_string(&systems_path) {
        Ok(text) => match systems::parse_table(&text, &systems_path) {
            Ok(table) => println!("systems.toml present, parses, {} systems", table.len()),
            Err(e) => {
                println!("systems.toml PRESENT BUT BROKEN");
                problems.push(format!("systems.toml does not parse: {e}"));
            }
        },
        Err(_) => {
            println!("systems.toml MISSING");
            problems.push("systems.toml is missing: Degauss will not start without it".into());
        }
    }

    let settings_path = dir.join("settings.toml");
    let loaded_settings = match Settings::load(&settings_path) {
        Ok(settings) => {
            if settings_path.exists() {
                println!("settings     present, parses");
            } else {
                println!("settings     none yet (fresh defaults)");
            }
            Some(settings)
        }
        Err(e) => {
            println!("settings     PRESENT BUT BROKEN");
            problems.push(format!("settings.toml does not parse: {e}"));
            None
        }
    };

    if let Some(settings) = loaded_settings.as_ref() {
        if settings.artwork_pack_roots.is_empty() {
            println!("artwork pack no systems selected");
        } else {
            for (group, root) in &settings.artwork_pack_roots {
                let provider = artwork_pack::Provider::load(group, Path::new(root), None);
                println!(
                    "artwork pack {group}: {} at {root}",
                    provider.health.label()
                );
                if !provider.health.usable() {
                    problems.push(format!("Artwork Pack {group}: {}", provider.status_line()));
                }
            }
        }
    }

    // State cannot fail by design: a broken file reads as a fresh one.
    let state_path = dir.join("state.toml");
    println!(
        "state        {}",
        if state_path.exists() {
            "present"
        } else {
            "none yet"
        }
    );

    let cache_dir = cache::dir_for(&settings_path);
    match cache::load_index(&cache_dir) {
        Some(index) => println!(
            "cache        index present, {} systems written down",
            index.systems.len()
        ),
        None => println!("cache        none yet (built on the first run, or after Rebuild)"),
    }

    let count_files = |sub: &str| -> Option<usize> {
        std::fs::read_dir(dir.join(sub))
            .ok()
            .map(|entries| entries.filter_map(|e| e.ok()).count())
    };
    match count_files("logos") {
        Some(n) => println!("logos        {n} files"),
        None => println!("logos        no folder (names are drawn instead)"),
    }
    let themes = theme::load_available(&dir.join("themes"));
    println!("themes       {} loaded", themes.themes.len());
    for problem in &themes.problems {
        println!("             {problem}");
        problems.push(format!("a theme did not load: {problem}"));
    }

    let mut missing_licences: Vec<&str> = Vec::new();
    for lic in [
        "LICENSE",
        "DejaVuSans-LICENSE.txt",
        "Px437-LICENSE.txt",
        "RobotoCondensed-LICENSE.txt",
        "Tamzen-LICENSE.txt",
    ] {
        if !dir.join(lic).exists() {
            missing_licences.push(lic);
        }
    }
    if missing_licences.is_empty() {
        println!("licences     all 5 present");
    } else {
        println!("licences     missing: {}", missing_licences.join(" "));
    }

    // The folder releases before this one installed to. Present means the
    // migration in degauss.sh has not finished, and the files in it are
    // the user's answers about what went wrong.
    let old = dir
        .parent()
        .and_then(Path::parent)
        .map(|scripts| scripts.join(".degauss"));
    match old {
        Some(old) if old.is_dir() => {
            let names: Vec<String> = std::fs::read_dir(&old)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            println!(
                "old folder   {} STILL PRESENT: {}",
                old.display(),
                names.join(" ")
            );
            problems.push(
                "the old folder has not migrated: start Degauss once from the Scripts menu and read its console line"
                    .into(),
            );
        }
        Some(old) => println!(
            "old folder   {} gone (migrated or never there)",
            old.display()
        ),
        None => println!("old folder   not applicable here"),
    }

    if problems.is_empty() {
        println!(
            "
verdict      ok"
        );
        Ok(())
    } else {
        println!(
            "
verdict      {} problem(s):",
            problems.len()
        );
        for problem in &problems {
            println!("  - {problem}");
        }
        Err(DegaussError::unsupported(
            "check-install",
            format!("{} problem(s) found", problems.len()),
        ))
    }
}

fn run() -> Result<()> {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return Ok(());
        }
        Err(message) => {
            eprintln!("degauss: {message}\n\n{USAGE}");
            return Err(DegaussError::unsupported("arguments", message));
        }
    };

    if args.check_install {
        // Deliberately before anything is loaded: this flag exists for
        // the installation that does NOT come up, so it must not depend
        // on a config that parses or a table that reads.
        let config_path = args
            .config
            .clone()
            .unwrap_or_else(|| beside_binary("degauss.toml"));
        return check_install(&config_path);
    }

    let started = Instant::now();
    let loaded = load_everything(&args)?;

    if loaded.systems.is_empty() {
        return Err(DegaussError::unsupported(
            "systems",
            format!(
                "no game folders found under {}",
                loaded.config.game_roots.join(", ")
            ),
        ));
    }

    if let Some(list) = args.import_favorites.clone() {
        return import_favorites(&loaded, &list);
    }

    if args.list_systems {
        println!("build        {}", build_description());
        println!("{} systems found", loaded.systems.len());
        for system in &loaded.systems {
            println!("  {:<28} {}", system.name(), system.path().display());
        }
        return Ok(());
    }

    // Which system was asked for, if any. `None` is not the same as the
    // first one: without the flag the browser opens where it always would.
    let chosen: Option<usize> = match &args.system {
        Some(id) => Some(select_system(&loaded.systems, id)?),
        None => None,
    };

    if args.audit {
        return audit_everything(&loaded);
    }

    if args.report || args.dry_run_launch {
        let system = &loaded.systems[chosen.unwrap_or(0)];
        println!("build        {}", build_description());
        println!("reading      {} ...", system.path().display());
        let effective = effective_library(&loaded, system)?;
        let audit = effective.audit(false);
        print_report(system, &effective, &audit);

        if args.dry_run_launch {
            let entry = audit.first_game.as_ref().ok_or_else(|| {
                DegaussError::unsupported("dry run", "this system has no games".to_string())
            })?;
            let mgl = Path::new("/tmp/degauss.mgl");
            let plan = match &entry.kind {
                browse::Kind::Play(browse::Launch::File(path)) => {
                    diagnostic_launch_plan(&loaded, system, path, mgl)?
                }
                browse::Kind::Play(browse::Launch::AmigaVision { install, title }) => {
                    launch::plan_amiga_vision(&system.to_config(), install, title, mgl)?
                }
                browse::Kind::Enter(_) => {
                    return Err(DegaussError::unsupported(
                        "dry run",
                        "the first entry is a folder".to_string(),
                    ))
                }
            };
            if let Some((boot, contents)) = &plan.boot_file {
                println!(
                    "\n--- would write {} ---\n{}",
                    boot.display(),
                    String::from_utf8_lossy(contents)
                );
            }
            println!("--- would write /tmp/degauss.mgl ---\n{}", plan.mgl);
            println!(
                "--- would write to {} ---\n{}",
                launch::CMD_FIFO,
                plan.command
            );
        }
        return Ok(());
    }

    if let Some(frames) = args.selftest {
        return selftest(loaded, args, chosen.unwrap_or(0), frames);
    }
    if let Some(path) = args.render.clone() {
        return render_once(loaded, args, chosen.unwrap_or(0), &path);
    }
    if let Some(frames) = args.bench {
        return bench(loaded, args, chosen.unwrap_or(0), frames);
    }

    run_on_framebuffer(loaded, args, chosen, started)
}

fn build_app(loaded: Loaded, width: u32, height: u32, repaint: RepaintBufferType) -> Result<App> {
    let window = render::install_platform(repaint)?;
    let ui = DegaussWindow::new()
        .map_err(|e| DegaussError::unsupported("building the interface", e.to_string()))?;
    window.set_size(PhysicalSize::new(width, height));
    ui.show()
        .map_err(|e| DegaussError::unsupported("showing the window", e.to_string()))?;

    Ok(App::new(
        loaded,
        window,
        ui,
        StartupTimings::default(),
        width,
        height,
    ))
}

/// Where a run says what it did.
///
/// Not the console. The console is behind the picture: anything printed
/// there is either never seen or a flash of text before the first frame,
/// which is not what a program that has taken over the television should be
/// doing. In /tmp because it is tmpfs, so this costs the card nothing and
/// clears itself on a power cycle.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const LOG_PATH: &str = "/tmp/degauss.log";

/// Begin a fresh log for this run.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn log_start() {
    let _ = std::fs::write(LOG_PATH, b"");
}

/// Add one line to it. Failing to log is never a reason to stop.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn note(line: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_PATH)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Beside the settings, which is beside the configuration.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
/// Write a favourite for every line of a list.
///
/// Reads what to favourite from a file rather than from a database,
/// because the database it came from is another program's and this one has
/// no business linking against SQLite to read it. What each line becomes
/// is decided here, by the same rules that launch the game: the table
/// knows which core a file belongs to and which slot it goes in, and a
/// favourite that guessed differently would start the wrong machine.
fn import_favorites(loaded: &Loaded, list: &Path) -> Result<()> {
    let text = std::fs::read_to_string(list)
        .map_err(|e| error::DegaussError::io("reading the list", list, e))?;
    let root = PathBuf::from(&loaded.config.menu_root).join(favorites::FAVORITES_DIR);
    let already = favorites::Favorites::read_with_systems(&root, &loaded.systems);

    let (mut written, mut skipped, mut missing, mut failed) = (0, 0, 0, 0);
    for line in text.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let (Some(folder), Some(path)) = (parts.next(), parts.next()) else {
            println!("skipped      malformed line: {line}");
            failed += 1;
            continue;
        };
        let title = parts.next().unwrap_or("").trim();
        // One plain name, never a path. Joining an absolute folder replaces
        // the root outright and `..` walks out of it, so a list file could
        // otherwise write anywhere this process can reach.
        let folder = folder.trim();
        let is_plain = std::path::Path::new(folder).components().count() == 1
            && !folder.contains('/')
            && !folder.contains('\\')
            && folder != "."
            && folder != "..";
        if !is_plain || !favorites::name_is_usable(folder) {
            println!("skipped      unusable folder name: {folder:?}");
            failed += 1;
            continue;
        }
        let path = PathBuf::from(path);
        let into = root.join(folder);

        if title.is_empty() {
            if zip::split_member_path(&path).is_some() {
                if let Err(error) = zip::validate_member_for_launch(&path) {
                    println!("failed       {}: {error}", path.display());
                    failed += 1;
                    continue;
                }
            } else if !path.exists() {
                println!("gone         {}", path.display());
                missing += 1;
                continue;
            }
        }
        let reference = if title.is_empty() {
            favorites::reference_of_with_systems(&path, &loaded.systems)
        } else {
            None
        };
        if let Some(reference) = &reference {
            if zip::split_member_path(&reference.owner_target).is_some() {
                if let Err(error) = zip::validate_member_for_launch(&reference.owner_target) {
                    println!("failed       {}: {error}", path.display());
                    failed += 1;
                    continue;
                }
            }
        }
        let owner_id = if title.is_empty() {
            reference
                .and_then(|reference| app::owner_of_favorite(&loaded.systems, &reference))
                .or_else(|| app::owner_of_path(&loaded.systems, &path))
        } else {
            app::owner_of_path(&loaded.systems, &favorites::amiga_key(&path, title))
        };
        let owner =
            owner_id.and_then(|id| loaded.systems.iter().find(|system| system.def.id == id));
        let Some(owner) = owner else {
            println!("skipped      no system owns {}", path.display());
            failed += 1;
            continue;
        };
        let config = owner.to_config();

        if title.is_empty() {
            if already.holds(&path) {
                skipped += 1;
                continue;
            }
            let outcome = match launch::favorite_mgl_with_choice(
                &config,
                &path,
                Path::new(&loaded.config.menu_root),
                loaded.settings.core_preference.unwrap_or_default()
                    == settings::CorePreference::RetroAchievementsFirst,
                loaded
                    .settings
                    .core_choices
                    .get(&owner.def.id)
                    .map(String::as_str),
            ) {
                Ok(Some(mgl)) => {
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                    favorites::add_game(&into, &stem, &mgl)
                }
                // A core file is linked to, not described.
                Ok(None) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    favorites::add_core(&into, &name, &path)
                }
                Err(e) => Err(e),
            };
            match outcome {
                Ok(_) => written += 1,
                Err(e) => {
                    println!("failed       {}: {e}", path.display());
                    failed += 1;
                }
            }
            continue;
        }

        // A title rather than a file: AmigaVision, whose games live inside
        // one image. `path` is the install rather than a game.
        if already.holds(&favorites::amiga_key(&path, title)) {
            skipped += 1;
            continue;
        }
        let safe: String = title
            .chars()
            .map(|c| {
                if favorites::BAD_CHARS.contains(&c) {
                    '-'
                } else {
                    c
                }
            })
            .collect();
        match launch::favorite_mgl_amiga(&config, &path, title)
            .and_then(|mgl| favorites::add_game(&into, &safe, &mgl))
        {
            Ok(_) => written += 1,
            Err(e) => {
                println!("failed       {title}: {e}");
                failed += 1;
            }
        }
    }

    println!(
        "favourites   {written} written, {skipped} already there, {missing} gone, {failed} failed"
    );
    Ok(())
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn state_path_for(settings: &Path) -> PathBuf {
    settings
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("state.toml")
}

fn render_once(loaded: Loaded, args: Args, chosen: usize, path: &Path) -> Result<()> {
    let (width, height) = args.geometry;
    // MemorySurface retains the previous frame, exactly like the framebuffer.
    // ReusedBuffer lets the second pass update chrome without discarding the
    // complete first pass from the image that is written below.
    let mut app = build_app(loaded, width, height, RepaintBufferType::ReusedBuffer)?;
    app.set_layout(args.layout.unwrap_or(Layout::Details));
    if args.system.is_some() {
        app.open_system_by_index(chosen);
    }
    // Browsing first, so the cursor moves in the folder rather than in
    // whatever list the screen it starts on happens to use; then the
    // choice; then the screen that was asked for, because what a menu
    // offers depends on what the cursor is on.
    app.set_screen(Screen::Browse);
    app.select(args.select);
    // A search is its own door into the same grid, so it goes last: asking
    // for the find screen afterwards would open it as a letter jump and
    // throw the query away.
    match args.find.as_deref() {
        Some(text) => app.search_for(text),
        None => {
            app.set_screen(args.screen);
            // The choice above moved the cursor in the folder, which is
            // what a context menu needs. The screens carrying lists of
            // their own take the choice themselves, or --select could
            // never reach past their first visible rows.
            if matches!(
                args.screen,
                Screen::Context
                    | Screen::Options
                    | Screen::Advanced
                    | Screen::ThemeEditor
                    | Screen::Help
                    | Screen::CategoryImage
                    | Screen::Scraper
                    | Screen::ScraperKeyboard
                    | Screen::ScraperProgress
                    | Screen::GameDataSource
                    | Screen::ArtworkPackLocation
                    | Screen::ArtworkPackDirectory
                    | Screen::SourceProgress
            ) {
                app.select(args.select);
            }
        }
    }

    let mut surface = MemorySurface::new(width, height, args.format);
    let mut presenter = Presenter::new(
        surface.geometry(),
        args.present.unwrap_or(PresentMode::Direct),
    );
    app.render_once(&mut surface, &mut presenter)?;
    surface.write_bmp(path)?;
    println!("wrote {} ({width}x{height})", path.display());
    Ok(())
}

fn bench(loaded: Loaded, args: Args, chosen: usize, frames: u32) -> Result<()> {
    let (width, height) = args.geometry;
    let mut app = build_app(loaded, width, height, RepaintBufferType::ReusedBuffer)?;
    app.set_layout(args.layout.unwrap_or(Layout::Details));
    app.open_system_by_index(chosen);

    for mode in [PresentMode::Direct, PresentMode::Staged] {
        let mut surface = MemorySurface::new(width, height, args.format);
        let mut presenter = Presenter::new(surface.geometry(), mode);
        let report = app.bench(&mut surface, &mut presenter, frames)?;
        report.print(&format!(
            "{} / {} {}x{} {:?}",
            args.layout.unwrap_or(Layout::Details).label(),
            mode.label(),
            width,
            height,
            args.format
        ));
    }
    Ok(())
}

/// Average milliseconds per frame for one phase of the work.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn per_frame_ms(total: std::time::Duration, frames: u32) -> f32 {
    if frames == 0 {
        return 0.0;
    }
    total.as_micros() as f32 / 1000.0 / frames as f32
}

#[cfg(not(target_os = "linux"))]
fn selftest(_loaded: Loaded, _args: Args, _chosen: usize, _frames: u32) -> Result<()> {
    Err(DegaussError::unsupported(
        "selftest",
        "needs a real framebuffer; run it on the device".to_string(),
    ))
}

/// Run the same scroll twice, once per drawing path, against the real
/// framebuffer, and write the numbers to a log.
///
/// This cannot be done on a memory surface: the two paths differ only in
/// where the renderer writes, and the question is whether the framebuffer's
/// write-combined mapping makes blending expensive. Ordinary RAM behaves the
/// same either way.
#[cfg(target_os = "linux")]
fn selftest(loaded: Loaded, args: Args, chosen: usize, frames: u32) -> Result<()> {
    use std::fmt::Write as _;

    let mut framebuffer = surface::Framebuffer::open(&args.device)?;
    let geometry = framebuffer.geometry();
    let vsync = framebuffer.wait_for_vsync()?;

    let mut app = build_app(
        loaded,
        geometry.width,
        geometry.height,
        RepaintBufferType::ReusedBuffer,
    )?;
    app.set_layout(args.layout.unwrap_or(Layout::Details));
    app.open_system_by_index(chosen);

    let mut log = String::new();
    let _ = writeln!(log, "mapping      {}", framebuffer.mapping_source());
    let _ = writeln!(
        log,
        "framebuffer  {} {}x{} {:?} stride {}",
        args.device.display(),
        geometry.width,
        geometry.height,
        geometry.format,
        geometry.line_length
    );
    let _ = writeln!(
        log,
        "vsync        {}",
        if vsync { "answered" } else { "not answered" }
    );
    let _ = writeln!(log, "build        {}", build_description());
    let _ = writeln!(
        log,
        "layout       {}  frames per run {}",
        args.layout.unwrap_or(Layout::Details).label(),
        frames
    );

    let mut totals = Vec::new();
    for mode in [PresentMode::Staged, PresentMode::Direct] {
        let mut presenter = Presenter::new(geometry, mode);
        let report = app.bench(&mut framebuffer, &mut presenter, frames)?;
        let draw = per_frame_ms(report.render_total, report.frames_drawn);
        let copy = per_frame_ms(report.blit_total, report.frames_drawn);
        let _ = writeln!(
            log,
            "\n{} path\n  frames {} in {:.2}s\n  frame avg {:.2} ms  p95 {:.2} ms  max {:.2} ms\n  per frame: art+rows {:.2} ms, draw {:.2} ms, copy {:.2} ms",
            mode.label(),
            report.frames_drawn,
            report.wall.as_secs_f32(),
            report.summary.avg_us as f32 / 1000.0,
            report.summary.p95_us as f32 / 1000.0,
            report.summary.max_us as f32 / 1000.0,
            per_frame_ms(report.build_total, report.frames_drawn),
            draw,
            copy,
        );
        totals.push((mode.label(), draw + copy));
    }

    if let [(a_name, a), (b_name, b)] = totals.as_slice() {
        let (faster, fast, slow) = if a <= b {
            (a_name, a, b)
        } else {
            (b_name, b, a)
        };
        let _ = writeln!(
            log,
            "\nverdict      {faster} is faster: {fast:.2} ms vs {slow:.2} ms per frame"
        );
        if (slow - fast).abs() < 0.2 {
            let _ = writeln!(
                log,
                "             under 0.2 ms apart, which is not worth choosing between"
            );
        }
    }

    let path = Path::new("/tmp/degauss-selftest.log");
    std::fs::write(path, log.as_bytes())
        .map_err(|e| DegaussError::io("writing selftest log", path, e))?;
    print!("{log}");
    println!("\nlog written to {}", path.display());
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run_on_framebuffer(
    _loaded: Loaded,
    _args: Args,
    _chosen: Option<usize>,
    _started: Instant,
) -> Result<()> {
    Err(DegaussError::unsupported(
        "framebuffer",
        "this build has no framebuffer; use --render to draw to an image".to_string(),
    ))
}

#[cfg(target_os = "linux")]
fn run_on_framebuffer(
    loaded: Loaded,
    args: Args,
    chosen: Option<usize>,
    started: Instant,
) -> Result<()> {
    let system_count = loaded.systems.len();
    log_start();
    let state_path = state_path_for(&loaded.settings_path);
    let resuming = state::is_resuming();
    let mut framebuffer = surface::Framebuffer::open(&args.device)?;
    let geometry = framebuffer.geometry();
    let vsync = framebuffer.wait_for_vsync()?;

    note(&format!("degauss      {}", build_description()));
    note(&format!("mapping      {}", framebuffer.mapping_source()));
    note(&format!(
        "framebuffer  {} {}x{} {:?} stride {}",
        args.device.display(),
        geometry.width,
        geometry.height,
        geometry.format,
        geometry.line_length
    ));
    note(&format!(
        "vsync        {}",
        if vsync { "answered" } else { "not answered" }
    ));
    note(&format!("systems      {system_count} found"));

    // MiSTer exposes one buffer, so the previous frame is still in it and
    // only what changed needs repainting.
    let mut app = build_app(
        loaded,
        geometry.width,
        geometry.height,
        RepaintBufferType::ReusedBuffer,
    )?;
    // Only when asked: without the flag the view saved in settings.toml,
    // which App::new already chose, is the one the user wants.
    if let Some(layout) = args.layout {
        app.set_layout(layout);
    }

    // Coming back from a game, go straight to where the user was: the
    // wordmark and the top of the tree are both wrong answers after they
    // walked four folders into a collection to pick that game.
    // Where the user was is worth going back to after a game and worth
    // nothing after a power cycle. The marker in /tmp is the difference,
    // and /tmp does not survive a restart.
    if resuming {
        state::clear_resuming();
        app.skip_splash();
    }
    match state::take_position(resuming, &state_path) {
        Some(saved) => {
            let at = Instant::now();
            app.restore_position(&saved);
            note(&format!(
                "resumed      {} in {} ms",
                saved.system,
                at.elapsed().as_millis()
            ));
        }
        None if !resuming => note("position     cold start, beginning at the top"),
        None => {}
    }

    // Asked for explicitly, so it wins over both the top of the tree and
    // whatever position was restored.
    if let Some(index) = chosen {
        app.open_system_by_index(index);
        app.skip_splash();
        note("system       opened from the command line");
    }

    let mut input = input::InputReader::open()?;
    // Worth printing: when a controller does nothing, the first question is
    // always whether Degauss can see any input device at all.
    note(&format!("input        {} devices", input.devices().len()));
    for device in input.devices() {
        note(&format!(
            "             {} {}{}",
            device.path.display(),
            device.name,
            if device.is_mister_virtual {
                "   <- the gamepad arrives here"
            } else {
                ""
            }
        ));
    }
    if !input.has_mister_virtual() {
        note("input        MiSTer's virtual device is absent, so a gamepad will not");
        note("             reach Degauss. The keyboard still works.");
    }

    // Stop the virtual terminal drawing its cursor over the picture. Not
    // fatal if it fails, but worth saying out loud.
    let mut console = match input::ConsoleGuard::acquire() {
        Ok(guard) => Some(guard),
        Err(message) => {
            note(&format!("console      {message}"));
            None
        }
    };
    let mut terminal = input::TerminalGuard::acquire()?;

    // Physical framebuffer memory favours staged rendering. Explicit choices
    // still win, and this runtime default never changes the saved settings.
    let default = default_present_mode(framebuffer.mapping_source());
    let mode = app.initialize_presentation(default, args.present);
    let mut presenter = Presenter::new(geometry, mode);
    let outcome = app.run(&mut framebuffer, &mut input, &mut presenter);

    terminal.restore();
    if let Some(console) = console.as_mut() {
        console.restore();
    }
    let outcome = outcome?;

    let summary = app.frame_summary();
    note(&format!(
        "ran for      {:.1} s",
        started.elapsed().as_secs_f32()
    ));
    note(&format!(
        "frame time   avg {:.2} ms   p95 {:.2} ms   max {:.2} ms   {:.0} fps",
        summary.avg_us as f32 / 1000.0,
        summary.p95_us as f32 / 1000.0,
        summary.max_us as f32 / 1000.0,
        summary.fps()
    ));
    let art = app.art_stats();
    let covers = app.covers();
    let thumbnails = app.gallery_covers();
    note(&format!(
        "art          {} loads, {} skipped by scrolling, worst {} us, {} large and {} thumbnails held",
        art.loads,
        art.deferred,
        art.worst_load_us,
        covers.len(),
        thumbnails.len()
    ));
    if let Some(text) = app::report_art_failures(covers, thumbnails) {
        note(&text);
    }

    match outcome {
        Outcome::Quit => note("ended        user quit"),
        Outcome::Launch { plan, name } => {
            // Written before the core is asked for: once the command goes
            // into the FIFO, MiSTer replaces this process and there is no
            // later moment to save anything in.
            if let Err(e) = app.position().save(&state_path) {
                note(&format!("state        not saved: {e}"));
            }
            state::mark_resuming();
            note(&format!("ended        launching {name}"));
            launch::execute(&plan, Path::new(launch::CMD_FIFO))?;
        }
    }
    Ok(())
}

struct EffectiveLibrary {
    library: browse::Library,
    provider: Option<artwork_pack::Provider>,
    fingerprints: cache::ContentFingerprints,
    notes: Vec<String>,
}

impl EffectiveLibrary {
    fn audit(&self, show_empty: bool) -> browse::Audit {
        match self.provider.as_ref() {
            Some(provider) => self.library.audit_projected(show_empty, &mut |rows| {
                let _ = provider.apply_with_fingerprints(rows, &self.fingerprints);
            }),
            None => self.library.audit(show_empty),
        }
    }
}

fn effective_library(loaded: &Loaded, system: &FoundSystem) -> Result<EffectiveLibrary> {
    let id = &system.def.id;
    let Some(root) = artwork_pack::selected_root(&loaded.settings.artwork_pack_roots, id) else {
        return Ok(EffectiveLibrary {
            library: browse::Library::open_with_names(&system.to_config(), loaded.names.clone())?,
            provider: None,
            fingerprints: cache::ContentFingerprints::new(),
            notes: Vec::new(),
        });
    };

    let (language, notes) = match scraper::ScraperSettings::load(
        &scraper::ScraperSettings::path_beside(&loaded.settings_path),
    ) {
        Ok(settings) => (settings.language, Vec::new()),
        Err(error) => (
            None,
            vec![format!(
                "ScreenScraper settings could not supply the preferred synopsis language: {error}"
            )],
        ),
    };
    let provider = artwork_pack::Provider::load(id, root, language.as_deref());
    let library = browse::Library::open_source_neutral(&system.to_config(), loaded.names.clone())?;
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let cached = cache::load_artwork_pack_data(&cache::dir_for(&loaded.settings_path), id);
    let cached_is_current = match cached.as_ref() {
        Some(data) => provider
            .cached_fingerprints_are_current(
                &data.cache,
                &data.fingerprints,
                data.fingerprints_complete,
                &cancelled,
            )?
            .unwrap_or(false),
        None => false,
    };
    let fingerprints = if cached_is_current {
        cached.map(|data| data.fingerprints).unwrap_or_default()
    } else if provider.health.usable() {
        let source_cache = match cached {
            Some(data) => data.cache,
            None => cache::build_system_checked(&library)?,
        };
        provider
            .fingerprints_for_cache(&source_cache, &cancelled, &mut |_, _| {})?
            .unwrap_or_default()
    } else {
        cache::ContentFingerprints::new()
    };
    Ok(EffectiveLibrary {
        library,
        provider: Some(provider),
        fingerprints,
        notes,
    })
}

fn print_report(system: &FoundSystem, effective: &EffectiveLibrary, audit: &browse::Audit) {
    let library = &effective.library;
    println!("system       {} ({})", system.name(), system.category());
    match effective.provider.as_ref() {
        Some(provider) => {
            println!("source       Artwork Pack ({})", provider.health.label());
            println!("pack root    {}", provider.docs_root.display());
            for diagnostic in &provider.diagnostics {
                println!("pack note    {diagnostic}");
            }
        }
        None => println!("source       Gamelist"),
    }
    for note in &effective.notes {
        println!("source note  {note}");
    }
    for (folder, has_gamelist) in library.gamelists() {
        println!(
            "folder       {} [{}]",
            folder.display(),
            if has_gamelist && effective.provider.is_some() {
                "gamelist.xml ignored"
            } else if has_gamelist {
                "gamelist.xml"
            } else {
                "no gamelist"
            }
        );
    }
    println!(
        "metadata     gamelist {} ms, artwork index {} ms over {} files",
        library.cost.gamelist_ms, library.cost.art_ms, library.cost.art_files
    );
    println!(
        "contents     {} games, {} folders, {} places read, deepest {}",
        audit.games, audit.folders, audit.places_read, audit.deepest
    );
    println!(
        "artwork      {} of {} games have a picture ({}%)",
        audit.with_art,
        audit.games,
        percent(audit.with_art, audit.games)
    );
    if audit.stopped_at_limit {
        println!("             INCOMPLETE: the walk hit its limit, totals are a floor");
    }
    for (path, reason) in audit.unreadable.iter().take(5) {
        println!("unreadable   {}: {reason}", path.display());
    }
    if let Some(first) = &audit.first_game {
        println!("first game   {}", first.name);
        if let (Some(provider), browse::Kind::Play(launch)) =
            (effective.provider.as_ref(), &first.kind)
        {
            match provider
                .presentation_for_launch_with_fingerprints(launch, &effective.fingerprints)
            {
                Ok(Some(presentation)) => {
                    if let Some(diagnostic) = presentation.diagnostic {
                        println!(
                            "pack match   {} / {} by {}{}",
                            diagnostic.pack_folder,
                            diagnostic.key,
                            diagnostic.method.label(),
                            diagnostic
                                .synopsis_language
                                .map(|language| format!(", synopsis {language}"))
                                .unwrap_or_default()
                        );
                    }
                }
                Ok(None) => println!("pack match   none"),
                Err(error) => println!("pack match   FAILED: {error}"),
            }
        }
    }
}

fn percent(part: usize, whole: usize) -> usize {
    part.checked_mul(100)
        .and_then(|scaled| scaled.checked_div(whole))
        .unwrap_or(0)
}

/// Every system on the card, in one table.
///
/// The question this exists to answer is which systems are wrong, without
/// opening each one and looking. A system with games and no artwork, or
/// artwork on the card and a gamelist that binds none of it, shows up as a
/// number out of line with the rest.
fn audit_everything(loaded: &Loaded) -> Result<()> {
    println!("build        {}", build_description());
    println!();
    println!(
        "{:<26} {:<9} {:>7} {:>8} {:>6}  folders",
        "system", "group", "games", "art", "art%"
    );

    let mut total_games = 0usize;
    let mut total_art = 0usize;
    let mut problems: Vec<String> = Vec::new();

    for system in &loaded.systems {
        let effective = match effective_library(loaded, system) {
            Ok(effective) => effective,
            Err(e) => {
                println!("{:<26} FAILED: {e}", system.name());
                problems.push(format!("{}: {e}", system.name()));
                continue;
            }
        };
        let audit = effective.audit(false);
        total_games += audit.games;
        total_art += audit.with_art;

        println!(
            "{:<26} {:<9} {:>7} {:>8} {:>5}%  {}",
            truncate(system.name(), 26),
            truncate(system.category(), 9),
            audit.games,
            audit.with_art,
            percent(audit.with_art, audit.games),
            audit.folders
        );

        let has_gamelist = effective
            .library
            .gamelists()
            .iter()
            .any(|(_, present)| *present);
        if let Some(provider) = effective.provider.as_ref() {
            if provider.health != artwork_pack::ProviderHealth::Ready {
                problems.push(format!(
                    "{}: Artwork Pack {}",
                    system.name(),
                    provider.status_line()
                ));
            }
        }
        if audit.games == 0 {
            problems.push(format!("{}: no games found", system.name()));
        } else if effective
            .provider
            .as_ref()
            .is_some_and(|provider| provider.health.usable())
            && audit.with_art == 0
        {
            problems.push(format!(
                "{}: Artwork Pack resolved no pictures for {} games",
                system.name(),
                audit.games
            ));
        } else if effective.provider.is_none() && has_gamelist && audit.with_art == 0 {
            problems.push(format!(
                "{}: has a gamelist but not one picture bound",
                system.name()
            ));
        }
        if audit.stopped_at_limit {
            problems.push(format!(
                "{}: walk hit its limit, totals are a floor",
                system.name()
            ));
        }
        for (path, reason) in audit.unreadable.iter().take(3) {
            problems.push(format!(
                "{}: {} unreadable: {reason}",
                system.name(),
                path.display()
            ));
        }
    }

    println!();
    println!(
        "total        {} systems, {} games, {} with artwork ({}%)",
        loaded.systems.len(),
        total_games,
        total_art,
        percent(total_art, total_games)
    );
    if problems.is_empty() {
        println!("problems     none");
    } else {
        println!("problems     {}", problems.len());
        for problem in &problems {
            println!("             {problem}");
        }
    }
    Ok(())
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        text.chars().take(width - 1).chain("~".chars()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Args {
        parse_from(words.iter().map(|w| w.to_string()))
            .expect("parses")
            .expect("not --help")
    }

    fn diagnostic_test_systems(root: &Path) -> Vec<FoundSystem> {
        let definitions = systems::parse_table(
            r#"
[[systems]]
name = "Nintendo Entertainment System"
id = "NES"
folders = ["Shared", "Nintendo"]
rbf = "_Console/NES"
extensions = ["nes", "mgl"]
[[systems.launch]]
extensions = ["nes"]
type = "f"
index = 1
delay = 1
[[systems]]
name = "Famicom Disk System"
id = "FDS"
folders = ["Shared", "NES"]
rbf = "_Console/NES"
setname = "FDS"
extensions = ["fds", "mgl"]
[[systems.launch]]
extensions = ["fds"]
type = "f"
index = 1
delay = 2
[[systems]]
name = "Favorites"
id = "Favorites"
folders = ["/media/fat/_@Favorites", "_@Favorites"]
rbf = ""
extensions = ["mgl", "rbf"]
category = "Favorites"
"#,
            Path::new("diagnostic fixture"),
        )
        .unwrap();
        definitions
            .into_iter()
            .map(|def| {
                let folder = if def.id == "Favorites" {
                    "_@Favorites"
                } else {
                    "games/NES"
                };
                FoundSystem {
                    def,
                    paths: vec![root.join(folder)],
                    logo_dir: None,
                    menu_folder: None,
                }
            })
            .collect()
    }

    #[test]
    fn physical_mapping_defaults_to_staged_and_native_keeps_direct() {
        assert_eq!(
            default_present_mode("/dev/mem fallback"),
            PresentMode::Staged
        );
        assert_eq!(default_present_mode("fbdev"), PresentMode::Direct);
    }

    #[test]
    fn diagnostic_ids_and_names_take_precedence_over_folder_aliases() {
        let systems = diagnostic_test_systems(Path::new("/fixture"));
        assert_eq!(select_system(&systems, "nes").unwrap(), 0);
        assert_eq!(select_system(&systems, "FAMICOM DISK SYSTEM").unwrap(), 1);
        assert_eq!(select_system(&systems, "fds").unwrap(), 1);
        assert_eq!(select_system(&systems, "Nintendo").unwrap(), 0);
        // Both configured spellings belong to one system, so this is unique.
        assert_eq!(select_system(&systems, "_@favorites").unwrap(), 2);
    }

    #[test]
    fn diagnostic_ambiguous_alias_lists_owner_ids_and_unknown_alias_errors() {
        let systems = diagnostic_test_systems(Path::new("/fixture"));
        let error = select_system(&systems, "Shared").unwrap_err().to_string();
        assert!(error.contains("ambiguous"));
        assert!(error.contains("NES, FDS"));
        assert!(!error.contains("Favorites"));
        assert!(select_system(&systems, "uninstalled")
            .unwrap_err()
            .to_string()
            .contains("not on this machine"));
    }

    fn diagnostic_fixture(name: &str) -> (PathBuf, Loaded) {
        let root = std::env::temp_dir().join(format!("degauss-cli-{name}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        for folder in ["games/NES", "_@Favorites", "_Console", "_RA_Cores/Cores"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        std::fs::write(root.join("_Console/NES.rbf"), b"discovery fixture").unwrap();
        std::fs::write(root.join("_RA_Cores/Cores/NES.rbf"), b"discovery fixture").unwrap();
        std::fs::write(root.join("_RA_Cores/NES.mgl"), "<mistergamedescription><rbf>_RA_Cores/Cores/NES</rbf><setname same_dir=\"1\">RA_NES</setname></mistergamedescription>").unwrap();
        let mut config = Config::parse("[app]\n", Path::new("fixture")).unwrap();
        config.menu_root = root.to_string_lossy().into_owned();
        let loaded = Loaded {
            config,
            settings: Settings::default(),
            settings_path: root.join("settings.toml"),
            systems: diagnostic_test_systems(&root),
            names: browse::DisplayNames::default(),
            table: Vec::new(),
            logo_dir: None,
            themes_dir: root.join("themes"),
            themes: theme::ThemeSet::default(),
        };
        (root, loaded)
    }

    #[test]
    fn diagnostic_favorite_uses_the_owning_system_core_choice() {
        let (root, mut loaded) = diagnostic_fixture("favorite-owner");
        std::fs::write(root.join("games/NES/Game.fds"), b"path fixture").unwrap();
        let favorite = root.join("_@Favorites/Game.mgl");
        std::fs::write(&favorite, "<mistergamedescription><rbf>_Console/NES</rbf><setname>FDS</setname><file delay=\"2\" type=\"f\" index=\"1\" path=\"Game.fds\"/></mistergamedescription>").unwrap();
        loaded
            .settings
            .core_choices
            .insert("NES".into(), "standard".into());
        loaded
            .settings
            .core_choices
            .insert("FDS".into(), "ra".into());
        for category in ["Favorites", "favorites", "FAVORITES"] {
            loaded.systems[2].def.category = Some(category.into());
            let plan = diagnostic_launch_plan(
                &loaded,
                &loaded.systems[2],
                &favorite,
                &root.join("temp.mgl"),
            )
            .unwrap();
            assert!(plan.mgl.contains("<rbf>_RA_Cores/Cores/NES</rbf>"));
            assert!(plan
                .mgl
                .contains("<setname same_dir=\"1\">RA_NES</setname>"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn favorites_import_accepts_zip_members_and_uses_shared_system_choice() {
        let (root, mut loaded) = diagnostic_fixture("zip-import");
        let archive = root.join("games/NES/library.zip");
        std::fs::write(&archive, zip::tests_archive(&["Game.fds"], false)).unwrap();
        let member = archive.join("Game.fds");
        loaded
            .settings
            .core_choices
            .insert("NES".into(), "standard".into());
        loaded
            .settings
            .core_choices
            .insert("FDS".into(), "ra".into());
        let list = root.join("import.tsv");
        std::fs::write(&list, format!("Test\t{}\n", member.display())).unwrap();
        import_favorites(&loaded, &list).unwrap();
        let result = std::fs::read_to_string(root.join("_@Favorites/Test/Game.mgl")).unwrap();
        assert!(result.contains("<rbf>_RA_Cores/Cores/NES</rbf>"));
        assert!(result.contains("library.zip/Game.fds"));
        std::fs::write(&archive, b"corrupt archive").unwrap();
        assert!(diagnostic_launch_plan(
            &loaded,
            &loaded.systems[1],
            &member,
            &root.join("temp.mgl")
        )
        .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn screen_help_uses_one_separator_before_the_final_choice() {
        let section = USAGE
            .split("  --screen <name>")
            .nth(1)
            .and_then(|rest| rest.split("  --select <n>").next())
            .expect("screen help section exists");
        assert_eq!(
            section
                .split_whitespace()
                .filter(|word| *word == "or")
                .count(),
            1
        );
        assert!(
            section.contains("artwork-pack-directory or\n                      source-progress")
        );
    }

    #[test]
    fn without_the_flag_no_view_is_forced() {
        // The view is a setting the user changes in Options and expects to
        // still be there next time. If the parser defaulted to a real
        // layout, startup would overwrite that saved choice on every run
        // and the setting would look like it never saved.
        assert_eq!(parse(&[]).layout, None);
    }

    #[test]
    fn the_flag_forces_exactly_the_view_named() {
        assert_eq!(
            parse(&["--layout", "carousel"]).layout,
            Some(Layout::Carousel)
        );
        assert_eq!(parse(&["--layout", "list"]).layout, Some(Layout::List));
        assert_eq!(
            parse(&["--layout", "multi-list"]).layout,
            Some(Layout::MultiList)
        );
        assert_eq!(
            parse(&["--layout", "gallery"]).layout,
            Some(Layout::Gallery)
        );
    }

    #[test]
    fn the_retired_view_names_still_parse() {
        // Cards written by an older build name these; they must keep working.
        assert_eq!(
            parse(&["--layout", "preview"]).layout,
            Some(Layout::Details)
        );
        assert_eq!(parse(&["--layout", "covers"]).layout, Some(Layout::Tiled));
    }

    #[test]
    fn the_category_image_picker_can_be_rendered_for_visual_checks() {
        assert_eq!(
            parse(&["--screen", "category-image"]).screen,
            Screen::CategoryImage
        );
    }

    #[test]
    fn without_the_flag_no_drawing_path_is_forced() {
        // Advanced > Drawing path is saved when changed. If the parser
        // defaulted to a real mode, startup would override the saved one and
        // the setting would silently revert on every restart.
        assert_eq!(parse(&[]).present, None);
        assert_eq!(
            parse(&["--present", "staged"]).present,
            Some(PresentMode::Staged)
        );
    }

    #[test]
    fn no_system_asked_for_is_not_the_same_as_the_first_one() {
        // Absent means "open wherever browsing would normally start". A
        // default of zero here would silently open whichever system happens
        // to sort first, which is what --system C64 once did.
        assert_eq!(parse(&[]).system, None);
        assert_eq!(parse(&["--system", "C64"]).system, Some("C64".to_string()));
    }

    #[test]
    fn every_scraper_screen_can_be_rendered_for_visual_checks() {
        assert_eq!(parse(&["--screen", "scraper"]).screen, Screen::Scraper);
        assert_eq!(
            parse(&["--screen", "scraper-keyboard"]).screen,
            Screen::ScraperKeyboard
        );
        assert_eq!(
            parse(&["--screen", "scraper-progress"]).screen,
            Screen::ScraperProgress
        );
    }

    #[test]
    fn every_artwork_pack_screen_can_be_rendered_for_visual_checks() {
        assert_eq!(
            parse(&["--screen", "game-data-source"]).screen,
            Screen::GameDataSource
        );
        assert_eq!(
            parse(&["--screen", "artwork-pack-location"]).screen,
            Screen::ArtworkPackLocation
        );
        assert_eq!(
            parse(&["--screen", "artwork-pack-directory"]).screen,
            Screen::ArtworkPackDirectory
        );
        assert_eq!(
            parse(&["--screen", "source-progress"]).screen,
            Screen::SourceProgress
        );
    }

    fn visual_match(id: &str, name: &str, year: &str) -> scraper::Match {
        scraper::Match {
            id: id.to_string(),
            name: name.to_string(),
            names: vec![name.to_string()],
            metadata: scraper::Metadata {
                releasedate: Some(format!("{year}0101T000000")),
                ..Default::default()
            },
            media: None,
            rom_crc32: None,
            rom_md5: None,
            rom_sha1: None,
        }
    }

    fn visual_preview(seed: u8) -> covers::RgbImage {
        let width = 320u32;
        let height = 180u32;
        let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..height {
            for x in 0..width {
                rgb.extend_from_slice(&[
                    seed.wrapping_add((x * 155 / width) as u8),
                    35u8.wrapping_add((y * 190 / height) as u8),
                    225u8.wrapping_sub((x * 120 / width) as u8),
                ]);
            }
        }
        covers::RgbImage::new(width, height, rgb).expect("visual preview dimensions agree")
    }

    fn write_scraper_visual(
        app: &mut App,
        surface: &mut MemorySurface,
        presenter: &mut Presenter,
        folder: &Path,
        stem: &str,
    ) {
        app.render_once(surface, presenter)
            .expect("visual state renders");
        surface
            .write_bmp(&folder.join(format!(
                "{stem}-{}x{}.bmp",
                surface.geometry().width,
                surface.geometry().height
            )))
            .expect("visual capture writes");
    }

    #[test]
    #[ignore = "writes explicitly requested manual-match visual QA captures"]
    fn render_manual_scraper_match_acceptance_states() {
        let output = std::env::var_os("DEGAUSS_VISUAL_DIR")
            .map(PathBuf::from)
            .expect("DEGAUSS_VISUAL_DIR names the private capture folder");
        std::fs::create_dir_all(&output).expect("capture folder exists");
        let geometry =
            std::env::var("DEGAUSS_VISUAL_GEOMETRY").unwrap_or_else(|_| "352x240".to_string());
        let (width, height) = geometry
            .split_once('x')
            .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
            .expect("DEGAUSS_VISUAL_GEOMETRY is WIDTHxHEIGHT");

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let scratch = output.join(format!(".runtime-{}", std::process::id()));
        std::fs::create_dir(&scratch).expect("private visual runtime folder is new");
        std::fs::copy(root.join("degauss.toml"), scratch.join("degauss.toml"))
            .expect("visual fixture configuration is copied");
        let args = Args {
            config: Some(scratch.join("degauss.toml")),
            systems: Some(root.join("assets/systems.toml")),
            geometry: (width, height),
            ..Default::default()
        };
        let loaded = load_everything(&args).expect("visual fixture configuration loads");
        let mut app = build_app(loaded, width, height, RepaintBufferType::ReusedBuffer)
            .expect("visual fixture interface builds");
        let mut surface = MemorySurface::new(width, height, PixelFormat::Rgb565);
        let mut presenter = Presenter::new(surface.geometry(), PresentMode::Direct);
        let scope = "Adventure Island (USA, Europe) [Rev A]";
        let candidates = vec![
            visual_match("101", "Adventure Island", "1986"),
            visual_match("102", "Adventure Island Classic", "1992"),
            visual_match(
                "103",
                "Adventure Island - The Beginning of an Exceptionally Long Candidate Name",
                "2008",
            ),
            visual_match("104", "Hudson's Adventure Island II", "1991"),
            visual_match("105", "Super Adventure Island", "1992"),
            visual_match("106", "New Adventure Island", "1992"),
        ];

        app.set_scraper_match_visual_fixture(
            scope,
            candidates.clone(),
            0,
            "6 Matches",
            Some(visual_preview(24)),
            "",
        );
        write_scraper_visual(
            &mut app,
            &mut surface,
            &mut presenter,
            &output,
            "scraper-match-first",
        );

        app.set_scraper_match_visual_fixture(
            scope,
            candidates.clone(),
            3,
            "6 Matches",
            Some(visual_preview(96)),
            "",
        );
        write_scraper_visual(
            &mut app,
            &mut surface,
            &mut presenter,
            &output,
            "scraper-match-later",
        );

        app.set_scraper_match_visual_fixture(
            scope,
            candidates.clone(),
            2,
            "6 Matches",
            None,
            "Loading Image...",
        );
        write_scraper_visual(
            &mut app,
            &mut surface,
            &mut presenter,
            &output,
            "scraper-match-loading",
        );

        app.set_scraper_match_visual_fixture(
            scope,
            candidates.clone(),
            4,
            "6 Matches",
            None,
            "No Image",
        );
        write_scraper_visual(
            &mut app,
            &mut surface,
            &mut presenter,
            &output,
            "scraper-match-no-art",
        );

        app.set_scraper_match_visual_fixture(
            scope,
            Vec::new(),
            0,
            "No Matches for Adventure Isles",
            None,
            "No Matches",
        );
        write_scraper_visual(
            &mut app,
            &mut surface,
            &mut presenter,
            &output,
            "scraper-match-no-result",
        );

        app.set_scraper_match_visual_fixture(
            scope,
            candidates.clone(),
            1,
            "ScreenScraper timed out",
            None,
            "Preview Unavailable",
        );
        write_scraper_visual(
            &mut app,
            &mut surface,
            &mut presenter,
            &output,
            "scraper-match-failure",
        );

        app.set_scraper_match_visual_fixture(
            scope,
            candidates.clone(),
            1,
            "6 Matches",
            Some(visual_preview(56)),
            "",
        );
        app.handle(input::Action::Accept);
        write_scraper_visual(
            &mut app,
            &mut surface,
            &mut presenter,
            &output,
            "scraper-match-confirm",
        );

        app.set_scraper_match_visual_fixture(
            scope,
            candidates,
            1,
            "6 Matches",
            Some(visual_preview(56)),
            "",
        );
        app.handle(input::Action::Context);
        write_scraper_visual(
            &mut app,
            &mut surface,
            &mut presenter,
            &output,
            "scraper-match-edit-search",
        );
        drop(app);
        std::fs::remove_dir_all(scratch).expect("private visual runtime folder is removed");
    }
}
