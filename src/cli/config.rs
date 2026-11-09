use anyhow::{Context, Result};
use clap::{ArgGroup, Subcommand};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use super::confirm;
use crate::config::profile::{
    Config, ConfigFile, GeoRegion, ProfilesDocument, Settings, SettingsDocument,
};
use crate::config::recovery::RecoveryKind;
use crate::config::routing::{GLOBAL_MODE_ID, RoutingConfig};

const NEEDS_FLAGS: &str = "`kvn config reset` without flags asks what to reset and needs a terminal; pass --profiles, --settings, --hwid and/or --routing <regions>, with --yes to skip the confirmation";

#[derive(Debug, Subcommand)]
pub(super) enum ConfigCommand {
    /// Archive profiles.json, settings.json and/or routing.json and create them as a fresh install does.
    #[command(group(
        ArgGroup::new("files")
            .multiple(true)
            .args(["profiles", "settings", "hwid", "routing"])
    ))]
    Reset {
        /// Reset profiles.json: every profile and subscription.
        #[arg(long)]
        profiles: bool,

        /// Reset settings.json: theme, DNS, kill switch, routing mode, update schedules and the HWID.
        #[arg(long)]
        settings: bool,

        /// Replace only the HWID in settings.json with a new one; subscriptions that send it see a new device.
        #[arg(long)]
        hwid: bool,

        /// Reset routing.json to Global and the modes of these countries (comma-separated or repeated).
        #[arg(long, value_name = "REGION", value_delimiter = ',')]
        routing: Vec<GeoRegion>,

        /// Skip the confirmation (the old files are preserved, not deleted).
        #[arg(long, requires = "files")]
        yes: bool,
    },
    /// Validate saved files and install them as profiles.json, settings.json and/or routing.json.
    #[command(group(
        ArgGroup::new("files")
            .required(true)
            .multiple(true)
            .args(["profiles", "settings", "routing"])
    ))]
    Recover {
        /// Profiles or conflict file to install as profiles.json; a profiles.json from kvn 0.36 or older gives its profiles and subscriptions.
        #[arg(long, value_name = "FILE")]
        profiles: Option<PathBuf>,

        /// Settings or conflict file to install as settings.json; a profiles.json from kvn 0.36 or older gives its settings.
        #[arg(long, value_name = "FILE")]
        settings: Option<PathBuf>,

        /// Routing file to install as routing.json.
        #[arg(long, value_name = "FILE")]
        routing: Option<PathBuf>,
    },
}

pub(super) fn run(command: &ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Reset {
            profiles,
            settings,
            hwid,
            routing,
            yes,
        } => run_reset(
            Requested {
                profiles: *profiles,
                settings: *settings,
                hwid: *hwid,
            },
            routing,
            *yes,
        ),
        ConfigCommand::Recover {
            profiles,
            settings,
            routing,
        } => run_recover(Change {
            profiles: profiles.clone().map(Source::File),
            settings: settings.clone().map(Source::File),
            new_hwid: false,
            routing: routing.clone().map(RoutingSource::File),
        }),
    }
}

struct ConfigPaths {
    files: crate::config::ConfigPaths,
    routing: PathBuf,
}

impl ConfigPaths {
    fn current() -> Result<Self> {
        Ok(Self {
            files: crate::config::ConfigPaths::current()?,
            routing: crate::paths::routing_path()
                .context("Failed to determine routing config path")?,
        })
    }

    fn recovery_dir(&self) -> PathBuf {
        self.files.profiles.with_file_name("recovery")
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Source {
    FirstRun,
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
enum RoutingSource {
    Regions(Vec<GeoRegion>),
    File(PathBuf),
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Requested {
    profiles: bool,
    settings: bool,
    hwid: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Change {
    profiles: Option<Source>,
    settings: Option<Source>,
    new_hwid: bool,
    routing: Option<RoutingSource>,
}

impl Change {
    fn reset(requested: Requested, regions: &[GeoRegion]) -> Option<Self> {
        let change = Self {
            profiles: requested.profiles.then_some(Source::FirstRun),
            settings: requested.settings.then_some(Source::FirstRun),
            new_hwid: requested.hwid && !requested.settings,
            routing: (!regions.is_empty()).then(|| RoutingSource::Regions(distinct(regions))),
        };
        (change.profiles.is_some()
            || change.settings.is_some()
            || change.new_hwid
            || change.routing.is_some())
        .then_some(change)
    }
}

#[derive(Debug, PartialEq)]
enum ModeOutcome {
    Unchecked,
    Kept(String),
    Switched(String),
}

struct Prepared {
    change: Change,
    profiles: Option<ProfilesDocument>,
    settings: Option<SettingsDocument>,
    routing: Option<RoutingConfig>,
    mode: ModeOutcome,
    dropped_service_routes: bool,
}

impl Prepared {
    fn adjusts_settings(&self) -> bool {
        self.change.settings.is_none()
            && !self.change.new_hwid
            && (matches!(self.mode, ModeOutcome::Switched(_)) || self.dropped_service_routes)
    }

    fn settings_to_write(&self) -> Option<&SettingsDocument> {
        self.settings.as_ref().filter(|_| {
            self.change.settings.is_some() || self.change.new_hwid || self.adjusts_settings()
        })
    }
}

struct Applied {
    profiles_archive: Option<PathBuf>,
    settings_archive: Option<PathBuf>,
    routing_archive: Option<PathBuf>,
}

fn require_daemon_stopped() -> Result<()> {
    anyhow::ensure!(
        !crate::ipc::is_daemon_running(),
        "stop the kvn daemon before changing profiles.json, settings.json or routing.json: systemctl --user stop kvn-tui.service"
    );
    Ok(())
}

fn split_old_profiles(paths: &ConfigPaths, new_hwid: bool) -> bool {
    match crate::config::split_combined_at(&paths.files, new_hwid) {
        Ok(true) => {
            println!(
                "Moved settings from {} to {}{}",
                paths.files.profiles.display(),
                paths.files.settings.display(),
                if new_hwid { " with a new HWID" } else { "" }
            );
            true
        }
        Ok(false) => false,
        Err(error) => {
            eprintln!(
                "{} is invalid, so the settings it holds cannot be used: {error:#}",
                paths.files.profiles.display()
            );
            false
        }
    }
}

fn run_reset(requested: Requested, regions: &[GeoRegion], yes: bool) -> Result<()> {
    require_daemon_stopped()?;
    let paths = ConfigPaths::current()?;
    let hwid_replaced = split_old_profiles(&paths, requested.hwid && !requested.settings);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let asked = requested == Requested::default() && regions.is_empty();
    let requested = Requested {
        hwid: requested.hwid && !hwid_replaced,
        ..requested
    };
    let change = match Change::reset(requested, regions) {
        Some(change) => change,
        None if !asked => return Ok(()),
        None => {
            anyhow::ensure!(stdin.is_terminal(), NEEDS_FLAGS);
            let current = regions_with_modes(&paths.routing);
            let invalid = InvalidFiles::of(&paths);
            match ask_reset(&mut stdin.lock(), &mut stdout.lock(), &current, &invalid)? {
                Some(change) => change,
                None => {
                    println!("Config reset cancelled");
                    return Ok(());
                }
            }
        }
    };
    let prepared = prepare(&paths, change)?;
    println!("Warning:");
    for line in reset_warning(&prepared, &paths.recovery_dir()) {
        println!("  - {line}");
    }
    println!();
    if !yes
        && !confirm(
            &mut stdin.lock(),
            &mut stdout.lock(),
            &reset_question(&prepared),
        )?
    {
        println!("Config reset cancelled");
        return Ok(());
    }
    require_daemon_stopped()?;
    print_report(&prepared, &apply(&paths, &prepared)?, &paths);
    Ok(())
}

fn run_recover(change: Change) -> Result<()> {
    require_daemon_stopped()?;
    let paths = ConfigPaths::current()?;
    split_old_profiles(&paths, false);
    let prepared = prepare(&paths, change)?;
    print_report(&prepared, &apply(&paths, &prepared)?, &paths);
    Ok(())
}

fn distinct(regions: &[GeoRegion]) -> Vec<GeoRegion> {
    let mut unique = Vec::new();
    for region in regions {
        if !unique.contains(region) {
            unique.push(*region);
        }
    }
    unique
}

fn region_codes() -> String {
    clap::ValueEnum::value_variants()
        .iter()
        .map(GeoRegion::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_regions(answer: &str) -> Result<Vec<GeoRegion>, String> {
    let mut regions = Vec::new();
    for code in answer
        .split(',')
        .map(str::trim)
        .filter(|code| !code.is_empty())
    {
        let region = <GeoRegion as clap::ValueEnum>::from_str(code, true)
            .map_err(|_| format!("Unknown country `{code}`; choose from {}", region_codes()))?;
        regions.push(region);
    }
    Ok(regions)
}

fn regions_with_modes(routing_path: &Path) -> Vec<GeoRegion> {
    let Ok(routing) = crate::config::routing::load_routing_for_recovery(routing_path) else {
        return Vec::new();
    };
    GeoRegion::ALL
        .into_iter()
        .filter(|region| *region != GeoRegion::Global)
        .filter(|region| {
            routing
                .modes
                .iter()
                .any(|mode| GeoRegion::of_default_mode(&mode.id) == Some(*region))
        })
        .collect()
}

#[derive(Debug, Default)]
struct InvalidFiles {
    profiles: Option<String>,
    settings: Option<String>,
}

impl InvalidFiles {
    fn of(paths: &ConfigPaths) -> Self {
        let load = crate::config::load_config_read_only_at(&paths.files);
        let problem = |error: anyhow::Error| format!("{:#}", error.root_cause());
        Self {
            profiles: load.profiles_error.map(problem),
            settings: load.settings_error.map(problem),
        }
    }
}

fn ask_reset<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    current_regions: &[GeoRegion],
    invalid: &InvalidFiles,
) -> Result<Option<Change>> {
    for (name, problem) in [
        ("profiles.json", &invalid.profiles),
        ("settings.json", &invalid.settings),
    ] {
        if let Some(problem) = problem {
            writeln!(output, "{name} is invalid: {problem}")?;
        }
    }
    let profiles = confirm(
        input,
        output,
        "Reset profiles.json (profiles and subscriptions)?",
    )?;
    let settings = confirm(
        input,
        output,
        "Reset settings.json (theme, DNS, kill switch, routing mode and update schedules)?",
    )?;
    let requested = Requested {
        profiles,
        settings,
        hwid: false,
    };
    if invalid.settings.is_some() && !settings {
        writeln!(
            output,
            "routing.json can be reset only together with the invalid settings.json"
        )?;
        return Ok(Change::reset(requested, &[]));
    }
    let regions = if confirm(
        input,
        output,
        "Reset routing.json (routing modes, rules and rule-sets)?",
    )? {
        match ask_regions(input, output, current_regions)? {
            Some(regions) => regions,
            None => return Ok(None),
        }
    } else {
        Vec::new()
    };
    Ok(Change::reset(requested, &regions))
}

fn ask_regions<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    current_regions: &[GeoRegion],
) -> Result<Option<Vec<GeoRegion>>> {
    let current = if current_regions.is_empty() {
        String::new()
    } else {
        let codes: Vec<&str> = current_regions.iter().map(GeoRegion::as_str).collect();
        format!(" [current: {}]", codes.join(", "))
    };
    loop {
        write!(
            output,
            "Countries to keep routing modes for ({}; comma-separated; an empty answer cancels){current}: ",
            region_codes()
        )?;
        output.flush()?;
        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            return Ok(None);
        }
        match parse_regions(&answer) {
            Ok(regions) if regions.is_empty() => return Ok(None),
            Ok(regions) => return Ok(Some(regions)),
            Err(message) => writeln!(output, "{message}")?,
        }
    }
}

fn prepare(paths: &ConfigPaths, change: Change) -> Result<Prepared> {
    let routing = match &change.routing {
        Some(RoutingSource::Regions(regions)) => Some(RoutingConfig::with_region_modes(regions)),
        Some(RoutingSource::File(file)) => Some(read_routing_file(file, &paths.routing)?),
        None => None,
    };
    let profiles = match &change.profiles {
        Some(Source::FirstRun) => Some(ProfilesDocument::default()),
        Some(Source::File(file)) => Some(read_profiles_file(file, &paths.files.profiles)?),
        None => None,
    };
    let settings = match &change.settings {
        Some(Source::FirstRun) => Some(Config::for_first_run().settings_document()),
        Some(Source::File(file)) => Some(read_settings_file(file, &paths.files.settings)?),
        None if change.new_hwid => {
            Some(crate::config::current_settings_with_new_hwid(&paths.files)?)
        }
        None if change.routing.is_some() => Some(current_settings(&paths.files)?),
        None => None,
    };
    let mut prepared = Prepared {
        change,
        profiles,
        settings,
        routing,
        mode: ModeOutcome::Unchecked,
        dropped_service_routes: false,
    };
    let Some(document) = prepared.settings.as_mut() else {
        return Ok(prepared);
    };
    if matches!(prepared.change.routing, Some(RoutingSource::Regions(_))) {
        prepared.dropped_service_routes = document
            .settings
            .geo_routing
            .legacy_service_routes
            .take()
            .is_some();
    }
    let routing = match &prepared.routing {
        Some(routing) => Some(routing.clone()),
        None => crate::config::routing::load_routing_at(
            &paths.routing,
            &document.settings.geo_routing,
            false,
        )
        .ok(),
    };
    if let Some(routing) = routing {
        prepared.mode = settle_mode(&mut document.settings, &routing);
    }
    Ok(prepared)
}

fn current_settings(paths: &crate::config::ConfigPaths) -> Result<SettingsDocument> {
    let load = crate::config::load_config_read_only_at(paths);
    match load.settings_error {
        Some(error) => Err(error).context("settings.json is invalid, so the routing mode and the old Steam and Telegram overrides it keeps cannot be updated; fix settings.json or replace it too with --settings"),
        None => Ok(load.config.settings_document()),
    }
}

fn settle_mode(settings: &mut Settings, routing: &RoutingConfig) -> ModeOutcome {
    let mode = &mut settings.geo_routing.mode;
    if routing.declares(mode) {
        return ModeOutcome::Kept(routing.mode_name(mode));
    }
    ModeOutcome::Switched(std::mem::replace(mode, GLOBAL_MODE_ID.to_string()))
}

fn ensure_not_target(file: &Path, target: &Path, name: &str) -> Result<()> {
    anyhow::ensure!(
        file.is_file(),
        "recovery file {} does not exist or is not a regular file",
        file.display()
    );
    if target.exists() {
        anyhow::ensure!(
            std::fs::canonicalize(file)? != std::fs::canonicalize(target)?,
            "recovery file is already {name}"
        );
    }
    Ok(())
}

fn read_profiles_file(file: &Path, target: &Path) -> Result<ProfilesDocument> {
    ensure_not_target(file, target, "profiles.json")?;
    let document = crate::config::load_profiles_for_recovery(file)
        .with_context(|| format!("failed to load recovery file {}", file.display()))?;
    Ok(ProfilesDocument {
        json_schema: None,
        ..document
    })
}

fn read_settings_file(file: &Path, target: &Path) -> Result<SettingsDocument> {
    ensure_not_target(file, target, "settings.json")?;
    let document = crate::config::load_settings_for_recovery(file)
        .with_context(|| format!("failed to load recovery file {}", file.display()))?;
    Ok(SettingsDocument {
        json_schema: None,
        ..document
    })
}

fn read_routing_file(file: &Path, target: &Path) -> Result<RoutingConfig> {
    ensure_not_target(file, target, "routing.json")?;
    let routing = crate::config::routing::load_routing_for_recovery(file)
        .with_context(|| format!("failed to load recovery file {}", file.display()))?;
    Ok(RoutingConfig {
        json_schema: None,
        ..routing
    })
}

fn archive_current(path: &Path, kind: RecoveryKind) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let name = path.display();
    let contents =
        std::fs::read(path).with_context(|| format!("failed to read {name} before archiving"))?;
    let archive = crate::config::recovery::preserve_at(path, kind, &contents)
        .with_context(|| format!("failed to archive {name}"))?;
    Ok(Some(archive))
}

fn archive_file(paths: &ConfigPaths, file: ConfigFile, replaced: bool) -> Result<Option<PathBuf>> {
    if !replaced {
        return Ok(None);
    }
    archive_current(paths.files.of(file), RecoveryKind::archive(file))
}

fn apply(paths: &ConfigPaths, prepared: &Prepared) -> Result<Applied> {
    let settings = prepared.settings_to_write();
    let applied = Applied {
        profiles_archive: archive_file(paths, ConfigFile::Profiles, prepared.profiles.is_some())?,
        settings_archive: archive_file(paths, ConfigFile::Settings, settings.is_some())?,
        routing_archive: match &prepared.routing {
            Some(_) => archive_current(&paths.routing, RecoveryKind::RoutingArchive)?,
            None => None,
        },
    };
    if let Some(routing) = &prepared.routing {
        crate::config::routing::replace_routing_at(&paths.routing, routing)
            .context("failed to write routing.json")?;
    }
    if let Some(settings) = settings {
        let config = Config::from_documents(ProfilesDocument::default(), settings.clone());
        crate::config::save_file_at(ConfigFile::Settings, &paths.files.settings, &config)
            .context("failed to write settings.json")?;
    }
    if let Some(profiles) = &prepared.profiles {
        let config = Config::from_documents(profiles.clone(), SettingsDocument::default());
        crate::config::save_file_at(ConfigFile::Profiles, &paths.files.profiles, &config)
            .context("failed to write profiles.json")?;
    }
    Ok(applied)
}

fn kept_modes(regions: &[GeoRegion]) -> String {
    let countries: Vec<&str> = regions
        .iter()
        .filter(|region| **region != GeoRegion::Global)
        .map(GeoRegion::code_upper)
        .collect();
    if countries.is_empty() {
        "only Global".to_string()
    } else {
        format!("Global and the modes of {}", countries.join(", "))
    }
}

fn reset_files(prepared: &Prepared) -> Vec<&'static str> {
    [
        prepared.change.profiles.as_ref().map(|_| "profiles.json"),
        prepared.change.settings.as_ref().map(|_| "settings.json"),
        prepared.change.new_hwid.then_some("the HWID"),
        prepared.change.routing.as_ref().map(|_| "routing.json"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn archived_files(prepared: &Prepared) -> Vec<&'static str> {
    [
        prepared.profiles.as_ref().map(|_| "profiles.json"),
        prepared.settings_to_write().map(|_| "settings.json"),
        prepared.routing.as_ref().map(|_| "routing.json"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn joined(files: &[&str]) -> String {
    match files {
        [] => String::new(),
        [only] => (*only).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn reset_question(prepared: &Prepared) -> String {
    format!("Reset {}?", joined(&reset_files(prepared)))
}

fn reset_warning(prepared: &Prepared, recovery_dir: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    if prepared.change.profiles.is_some() {
        lines.push(
            "profiles.json: every profile and subscription is removed, as on a fresh install."
                .to_string(),
        );
    }
    if prepared.change.settings.is_some() {
        lines.push(
            "settings.json: every setting is replaced with the defaults of a fresh install, and subscriptions see a new HWID.".to_string(),
        );
    }
    if prepared.change.new_hwid {
        lines.push(
            "settings.json: a new HWID replaces the current one, and subscriptions that send it see a new device.".to_string(),
        );
    }
    if let Some(RoutingSource::Regions(regions)) = &prepared.change.routing {
        lines.push(format!(
            "routing.json: custom modes, rules and rule-sets and the modes of other countries are removed; it keeps {}.",
            kept_modes(regions)
        ));
    }
    if let ModeOutcome::Switched(mode) = &prepared.mode {
        lines.push(format!(
            "The selected routing mode `{mode}` will no longer exist; kvn switches to Global."
        ));
    }
    if prepared.dropped_service_routes {
        lines.push(
            "The Steam and Telegram overrides from kvn 0.35 that were not moved to routing.json yet are dropped.".to_string(),
        );
    }
    let restore: Vec<String> = archived_files(prepared)
        .into_iter()
        .map(|file| {
            format!(
                "`kvn config recover --{} <file>`",
                file.trim_end_matches(".json")
            )
        })
        .collect();
    lines.push(format!(
        "The current files are archived in {} (the three newest of each kind are kept); restore one with {}.",
        recovery_dir.display(),
        restore.join(" or ")
    ));
    lines
}

fn source_line(source: &Source, name: &str, path: &Path) -> String {
    match source {
        Source::FirstRun => format!("Created default {name} at {}", path.display()),
        Source::File(file) => format!("Recovered {name} from {}", file.display()),
    }
}

fn report_lines(prepared: &Prepared, applied: &Applied, paths: &ConfigPaths) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(source) = &prepared.change.profiles {
        lines.push(
            source_line(source, "profiles.json", &paths.files.profiles)
                + &archived_suffix(&applied.profiles_archive),
        );
    }
    if let Some(source) = &prepared.change.settings {
        lines.push(
            source_line(source, "settings.json", &paths.files.settings)
                + &archived_suffix(&applied.settings_archive),
        );
    }
    if prepared.change.new_hwid {
        lines.push(
            format!("Created a new HWID in {}", paths.files.settings.display())
                + &archived_suffix(&applied.settings_archive),
        );
    }
    if let Some(source) = &prepared.change.routing {
        let written = match source {
            RoutingSource::Regions(regions) => format!(
                "Created routing.json with {} at {}",
                kept_modes(regions),
                paths.routing.display()
            ),
            RoutingSource::File(file) => format!("Recovered routing.json from {}", file.display()),
        };
        lines.push(written + &archived_suffix(&applied.routing_archive));
    }
    match &prepared.mode {
        ModeOutcome::Switched(mode) => lines.push(format!(
            "Routing mode `{mode}` no longer exists; switched to Global"
        )),
        ModeOutcome::Kept(name) if prepared.change.routing.is_some() => {
            lines.push(format!("Routing mode {name} kept"))
        }
        _ => {}
    }
    if prepared.dropped_service_routes {
        lines.push("Dropped the Steam and Telegram overrides from kvn 0.35".to_string());
    }
    if prepared.adjusts_settings() {
        lines.push(format!(
            "Updated {}{}",
            paths.files.settings.display(),
            archived_suffix(&applied.settings_archive)
        ));
    }
    lines
}

fn archived_suffix(archived: &Option<PathBuf>) -> String {
    archived
        .as_ref()
        .map(|archive| format!("; previous file archived at {}", archive.display()))
        .unwrap_or_default()
}

fn print_report(prepared: &Prepared, applied: &Applied, paths: &ConfigPaths) {
    for line in report_lines(prepared, applied, paths) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;
    use std::fs;

    fn parse(args: &[&str]) -> Result<ConfigCommand, clap::Error> {
        let args = ["kvn", "config"].iter().chain(args);
        match Cli::try_parse_from(args)?.command {
            Some(Command::Config { command }) => Ok(command),
            _ => unreachable!("config arguments parse to a config command"),
        }
    }

    fn paths(dir: &tempfile::TempDir) -> ConfigPaths {
        ConfigPaths {
            files: crate::config::ConfigPaths::in_dir(dir.path()),
            routing: dir.path().join("routing.json"),
        }
    }

    fn settings_with(mode: &str, service_routes: bool) -> Config {
        let mut config = crate::test_helpers::saved_config();
        config.settings.geo_routing.mode = mode.to_string();
        if service_routes {
            config.settings.geo_routing.legacy_service_routes =
                Some(serde_json::json!({ "steam": "direct" }));
        }
        config
    }

    fn write_settings(paths: &ConfigPaths, mode: &str, service_routes: bool) {
        crate::config::save_file_at(
            ConfigFile::Settings,
            &paths.files.settings,
            &settings_with(mode, service_routes),
        )
        .unwrap();
    }

    fn write_routing(paths: &ConfigPaths, regions: &[GeoRegion]) -> RoutingConfig {
        let mut routing = RoutingConfig::with_region_modes(regions);
        let mut custom = routing.mode("global").unwrap().clone();
        custom.id = "work".into();
        custom.name = "Work".into();
        routing.modes.push(custom);
        crate::config::routing::replace_routing_at(&paths.routing, &routing).unwrap();
        routing
    }

    fn saved_settings(paths: &ConfigPaths) -> Settings {
        crate::config::load_settings_for_recovery(&paths.files.settings)
            .unwrap()
            .settings
    }

    fn saved_routing(paths: &ConfigPaths) -> RoutingConfig {
        crate::config::routing::load_routing_for_recovery(&paths.routing).unwrap()
    }

    fn recovery_files(paths: &ConfigPaths, prefix: &str) -> Vec<PathBuf> {
        fs::read_dir(paths.recovery_dir())
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| {
                        path.file_name()
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .starts_with(prefix)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn req(profiles: bool, settings: bool) -> Requested {
        Requested {
            profiles,
            settings,
            hwid: false,
        }
    }

    fn reset_routing(regions: &[GeoRegion]) -> Change {
        Change::reset(req(false, false), regions).unwrap()
    }

    #[test]
    fn parses_reset_targets() {
        assert!(matches!(
            parse(&["reset", "--routing", "ru,cn", "--routing", "ir", "--yes"]).unwrap(),
            ConfigCommand::Reset { profiles: false, settings: false, hwid: false, routing, yes: true }
                if routing == [GeoRegion::Ru, GeoRegion::Cn, GeoRegion::Ir]
        ));
        assert!(matches!(
            parse(&["reset", "--profiles"]).unwrap(),
            ConfigCommand::Reset { profiles: true, settings: false, hwid: false, routing, yes: false } if routing.is_empty()
        ));
        assert!(matches!(
            parse(&["reset", "--settings", "--yes"]).unwrap(),
            ConfigCommand::Reset { profiles: false, settings: true, hwid: false, routing, yes: true } if routing.is_empty()
        ));
        assert!(matches!(
            parse(&["reset"]).unwrap(),
            ConfigCommand::Reset { profiles: false, settings: false, hwid: false, routing, yes: false } if routing.is_empty()
        ));
    }

    #[test]
    fn rejects_unknown_countries_and_a_bare_yes() {
        for args in [
            &["reset", "--routing", "ru,xx"][..],
            &["reset", "--yes"],
            &["recover", "saved.json"],
            &["recover"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn parses_recover_targets() {
        assert!(matches!(
            parse(&["recover", "--profiles", "p.json", "--settings", "s.json", "--routing", "r.json"]).unwrap(),
            ConfigCommand::Recover { profiles: Some(profiles), settings: Some(settings), routing: Some(routing) }
                if profiles == Path::new("p.json") && settings == Path::new("s.json") && routing == Path::new("r.json")
        ));
    }

    #[test]
    fn country_answers_ignore_case_and_spaces() {
        assert_eq!(
            parse_regions(" RU, cn,, ir \n"),
            Ok(vec![GeoRegion::Ru, GeoRegion::Cn, GeoRegion::Ir])
        );
        assert_eq!(parse_regions("\n"), Ok(Vec::new()));
        assert!(parse_regions("ru,xx").unwrap_err().contains("`xx`"));
    }

    fn ask(answers: &str, current: &[GeoRegion]) -> (Option<Change>, String) {
        let mut output = Vec::new();
        let change = ask_reset(
            &mut answers.as_bytes(),
            &mut output,
            current,
            &InvalidFiles::default(),
        )
        .unwrap();
        (change, String::from_utf8(output).unwrap())
    }

    #[test]
    fn questions_choose_files_and_countries() {
        assert_eq!(ask("n\nn\nn\n", &[]).0, None);
        assert_eq!(
            ask("y\nn\nn\n", &[]).0,
            Change::reset(req(true, false), &[])
        );
        assert_eq!(
            ask("n\ny\nn\n", &[]).0,
            Change::reset(req(false, true), &[])
        );
        let (change, output) = ask("n\nn\ny\nxx\nru,cn,ru\n", &[GeoRegion::Ru]);
        assert_eq!(change, Some(reset_routing(&[GeoRegion::Ru, GeoRegion::Cn])));
        assert!(output.contains("[current: ru]"));
        assert!(output.contains("Unknown country `xx`"));
    }

    #[test]
    fn an_empty_country_answer_cancels_the_whole_reset() {
        assert_eq!(ask("y\ny\ny\n\n", &[]).0, None);
        assert_eq!(ask("y\ny\ny\n", &[]).0, None);
    }

    #[test]
    fn an_invalid_settings_json_is_named_before_the_questions() {
        let mut output = Vec::new();
        let invalid = InvalidFiles {
            profiles: None,
            settings: Some("EOF".into()),
        };
        let change = ask_reset(&mut "y\nn\n".as_bytes(), &mut output, &[], &invalid).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert_eq!(change, Change::reset(req(true, false), &[]));
        assert!(output.starts_with("settings.json is invalid: EOF\n"));
        assert!(output.contains("only together with the invalid settings.json"));
        assert!(!output.contains("Reset routing.json"));
    }

    #[test]
    fn current_countries_are_those_with_modes_in_routing_json() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        assert!(regions_with_modes(&paths.routing).is_empty());
        write_routing(&paths, &[GeoRegion::Cn, GeoRegion::Ru]);
        assert_eq!(
            regions_with_modes(&paths.routing),
            [GeoRegion::Ru, GeoRegion::Cn]
        );
        fs::write(&paths.routing, "{").unwrap();
        assert!(regions_with_modes(&paths.routing).is_empty());
    }

    #[test]
    fn routing_reset_rebuilds_the_file_and_switches_a_lost_mode_to_global() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_settings(&paths, "bypass_cn", true);
        let previous = write_routing(&paths, &[GeoRegion::Ru, GeoRegion::Cn]);

        let prepared = prepare(&paths, reset_routing(&[GeoRegion::Ru])).unwrap();
        assert_eq!(prepared.mode, ModeOutcome::Switched("bypass_cn".into()));
        assert!(prepared.dropped_service_routes);
        let applied = apply(&paths, &prepared).unwrap();

        assert_eq!(
            saved_routing(&paths),
            RoutingConfig::with_region_modes(&[GeoRegion::Ru])
        );
        let settings = saved_settings(&paths);
        assert_eq!(settings.geo_routing.mode, GLOBAL_MODE_ID);
        assert_eq!(settings.geo_routing.legacy_service_routes, None);
        assert!(!paths.files.profiles.exists());
        let archive = applied.routing_archive.unwrap();
        assert_eq!(
            crate::config::routing::load_routing_for_recovery(&archive).unwrap(),
            previous
        );
        let archived_settings = applied.settings_archive.unwrap();
        assert_eq!(
            crate::config::load_settings_for_recovery(&archived_settings)
                .unwrap()
                .settings
                .geo_routing
                .mode,
            "bypass_cn"
        );
    }

    #[test]
    fn routing_reset_keeps_a_mode_that_still_exists_and_leaves_settings_alone() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_settings(&paths, "bypass_ru", false);
        let before = fs::read(&paths.files.settings).unwrap();
        write_routing(&paths, &[GeoRegion::Ru]);

        let prepared = prepare(&paths, reset_routing(&[GeoRegion::Ru])).unwrap();
        let applied = apply(&paths, &prepared).unwrap();

        assert_eq!(prepared.mode, ModeOutcome::Kept("Bypass RU".into()));
        assert_eq!(applied.settings_archive, None);
        assert_eq!(fs::read(&paths.files.settings).unwrap(), before);
    }

    #[test]
    fn an_invalid_routing_json_is_reset_like_a_valid_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_settings(&paths, "global", false);
        fs::write(&paths.routing, "{ broken").unwrap();

        let prepared = prepare(&paths, reset_routing(&[GeoRegion::Global])).unwrap();
        let applied = apply(&paths, &prepared).unwrap();

        assert_eq!(
            saved_routing(&paths),
            RoutingConfig::with_region_modes(&[GeoRegion::Global])
        );
        assert_eq!(
            fs::read_to_string(applied.routing_archive.unwrap()).unwrap(),
            "{ broken"
        );
    }

    #[test]
    fn a_routing_change_is_refused_while_settings_json_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        fs::write(&paths.files.settings, "{").unwrap();
        write_routing(&paths, &[GeoRegion::Ru]);
        let routing_before = fs::read(&paths.routing).unwrap();

        let error = prepare(&paths, reset_routing(&[GeoRegion::Ru]))
            .err()
            .unwrap();

        assert!(format!("{error:#}").contains("settings.json is invalid"));
        assert_eq!(fs::read(&paths.routing).unwrap(), routing_before);
        assert_eq!(fs::read_to_string(&paths.files.settings).unwrap(), "{");
    }

    #[test]
    fn a_routing_change_is_refused_while_an_invalid_old_profiles_json_holds_the_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        fs::write(&paths.files.profiles, "{").unwrap();
        write_routing(&paths, &[GeoRegion::Ru]);

        let error = prepare(&paths, reset_routing(&[GeoRegion::Ru]))
            .err()
            .unwrap();

        assert!(format!("{error:#}").contains("settings are kept in"));
    }

    #[test]
    fn profiles_reset_archives_the_old_file_and_keeps_settings() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        fs::write(&paths.files.profiles, "{").unwrap();
        write_settings(&paths, "global", false);
        let settings = fs::read(&paths.files.settings).unwrap();

        let prepared = prepare(&paths, Change::reset(req(true, false), &[]).unwrap()).unwrap();
        let applied = apply(&paths, &prepared).unwrap();

        assert!(
            crate::config::load_profiles_for_recovery(&paths.files.profiles)
                .unwrap()
                .profiles
                .is_empty()
        );
        assert_eq!(
            fs::read_to_string(applied.profiles_archive.unwrap()).unwrap(),
            "{"
        );
        assert_eq!(fs::read(&paths.files.settings).unwrap(), settings);
        assert!(!paths.routing.exists());
    }

    #[test]
    fn settings_reset_writes_first_run_settings_and_archives_the_old_file() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_settings(&paths, "work", false);
        let old = fs::read_to_string(&paths.files.settings).unwrap();

        let prepared = prepare(&paths, Change::reset(req(false, true), &[]).unwrap()).unwrap();
        let applied = apply(&paths, &prepared).unwrap();

        assert_eq!(
            saved_settings(&paths).geo_routing.auto_update,
            crate::config::profile::GeoAutoUpdate::Every7d
        );
        assert_eq!(
            fs::read_to_string(applied.settings_archive.unwrap()).unwrap(),
            old
        );
        assert_eq!(recovery_files(&paths, "settings.json.invalid-").len(), 1);
        assert!(!paths.files.profiles.exists());
    }

    #[test]
    fn hwid_reset_replaces_only_the_hwid() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_settings(&paths, "work", false);
        write_routing(&paths, &[GeoRegion::Ru]);
        let old = fs::read_to_string(&paths.files.settings).unwrap();
        let change = Change::reset(
            Requested {
                hwid: true,
                ..Requested::default()
            },
            &[],
        )
        .unwrap();

        let prepared = prepare(&paths, change).unwrap();
        let applied = apply(&paths, &prepared).unwrap();

        let settings = saved_settings(&paths);
        assert_ne!(settings.hwid, crate::test_helpers::TEST_HWID);
        assert!(settings.hwid.starts_with("lnx-"));
        assert_eq!(settings.geo_routing.mode, "work");
        assert_eq!(
            fs::read_to_string(applied.settings_archive.unwrap()).unwrap(),
            old
        );
        assert_eq!(reset_question(&prepared), "Reset the HWID?");
    }

    #[test]
    fn recovering_routing_json_checks_the_file_and_archives_the_current_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_settings(&paths, "work", false);
        let current = write_routing(&paths, &[GeoRegion::Ru]);
        let saved = dir.path().join("saved.json");
        fs::write(&saved, r#"{ "schema_version": 1, "modes": [] }"#).unwrap();
        let recover = |file: &Path| Change {
            profiles: None,
            settings: None,
            new_hwid: false,
            routing: Some(RoutingSource::File(file.to_path_buf())),
        };

        assert!(prepare(&paths, recover(&saved)).is_err());
        assert!(prepare(&paths, recover(&paths.routing)).is_err());
        assert_eq!(saved_routing(&paths), current);

        let replacement = RoutingConfig::with_region_modes(&[GeoRegion::Cn]);
        let editor_copy = RoutingConfig {
            json_schema: Some("file:///run/user/1000/kvn/routing.schema.json".into()),
            ..replacement.clone()
        };
        crate::config::routing::replace_routing_at(&saved, &editor_copy).unwrap();
        let prepared = prepare(&paths, recover(&saved)).unwrap();
        let applied = apply(&paths, &prepared).unwrap();

        assert_eq!(saved_routing(&paths), replacement);
        assert_eq!(prepared.mode, ModeOutcome::Switched("work".into()));
        assert!(!prepared.dropped_service_routes);
        assert_eq!(recovery_files(&paths, "routing.json.invalid-").len(), 1);
        assert!(applied.routing_archive.is_some());
    }

    #[test]
    fn recovering_settings_json_switches_a_mode_routing_json_lacks() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_routing(&paths, &[GeoRegion::Ru]);
        let saved = dir.path().join("saved.json");
        crate::config::save_file_at(
            ConfigFile::Settings,
            &saved,
            &settings_with("only_cn", false),
        )
        .unwrap();

        let change = Change {
            profiles: None,
            settings: Some(Source::File(saved)),
            new_hwid: false,
            routing: None,
        };
        let prepared = prepare(&paths, change).unwrap();
        apply(&paths, &prepared).unwrap();

        assert_eq!(prepared.mode, ModeOutcome::Switched("only_cn".into()));
        assert_eq!(saved_settings(&paths).geo_routing.mode, GLOBAL_MODE_ID);
    }

    #[test]
    fn an_old_profiles_json_recovers_into_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        let saved = dir.path().join("profiles.json.before-migration");
        let mut config = settings_with("global", false);
        config.settings.theme = "nord".into();
        config
            .profiles
            .push(crate::config::profile::Profile::new_vless(
                "Old".into(),
                "a".into(),
                1,
                crate::test_helpers::TEST_UUID.into(),
            ));
        let mut combined = serde_json::to_value(&config).unwrap();
        combined["schema_version"] = 8.into();
        fs::write(&saved, combined.to_string()).unwrap();

        let change = Change {
            profiles: Some(Source::File(saved.clone())),
            settings: Some(Source::File(saved)),
            new_hwid: false,
            routing: None,
        };
        let prepared = prepare(&paths, change).unwrap();
        apply(&paths, &prepared).unwrap();

        let profiles = crate::config::load_profiles_for_recovery(&paths.files.profiles).unwrap();
        assert_eq!(profiles.profiles[0].name, "Old");
        assert_eq!(saved_settings(&paths).theme, "nord");
    }

    #[test]
    fn the_warning_names_what_each_reset_removes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_settings(&paths, "bypass_cn", true);
        let prepared = prepare(&paths, reset_routing(&[GeoRegion::Ru])).unwrap();

        let warning = reset_warning(&prepared, Path::new("/recovery")).join("\n");

        assert!(warning.contains("it keeps Global and the modes of RU."));
        assert!(warning.contains("`bypass_cn` will no longer exist"));
        assert!(warning.contains("Steam and Telegram overrides"));
        assert!(warning.contains("in /recovery"));
        assert!(warning.contains(
            "`kvn config recover --settings <file>` or `kvn config recover --routing <file>`"
        ));
        assert!(!warning.contains("settings.json:"));
        assert_eq!(reset_question(&prepared), "Reset routing.json?");
        let all = prepare(
            &paths,
            Change::reset(req(true, true), &[GeoRegion::Ru]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            reset_question(&all),
            "Reset profiles.json, settings.json and routing.json?"
        );
    }

    #[test]
    fn the_report_names_written_and_archived_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_settings(&paths, "bypass_ru", false);
        write_routing(&paths, &[GeoRegion::Ru]);
        let prepared = prepare(&paths, reset_routing(&[GeoRegion::Ru, GeoRegion::Cn])).unwrap();
        let applied = apply(&paths, &prepared).unwrap();

        let report = report_lines(&prepared, &applied, &paths).join("\n");

        assert!(report.contains("Created routing.json with Global and the modes of RU, CN"));
        assert!(report.contains("previous file archived at"));
        assert!(report.contains("Routing mode Bypass RU kept"));
    }
}
