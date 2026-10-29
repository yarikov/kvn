use anyhow::{Context, Result};
use clap::{ArgGroup, Subcommand};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use super::confirm;
use crate::config::profile::{Config, GeoRegion};
use crate::config::recovery::RecoveryKind;
use crate::config::routing::{GLOBAL_MODE_ID, RoutingConfig};

const NEEDS_FLAGS: &str = "`kvn config reset` without flags asks what to reset and needs a terminal; pass --profiles and/or --routing <regions>, with --yes to skip the confirmation";

#[derive(Debug, Subcommand)]
pub(super) enum ConfigCommand {
    /// Archive profiles.json and/or routing.json and create them as a fresh install does.
    #[command(group(
        ArgGroup::new("files")
            .multiple(true)
            .args(["profiles", "routing"])
    ))]
    Reset {
        /// Reset profiles.json: every profile, subscription and setting.
        #[arg(long)]
        profiles: bool,

        /// Reset routing.json to Global and the modes of these countries (comma-separated or repeated).
        #[arg(long, value_name = "REGION", value_delimiter = ',')]
        routing: Vec<GeoRegion>,

        /// Skip the confirmation (the old files are preserved, not deleted).
        #[arg(long, requires = "files")]
        yes: bool,
    },
    /// Validate saved files and install them as profiles.json and/or routing.json.
    #[command(group(
        ArgGroup::new("files")
            .required(true)
            .multiple(true)
            .args(["profiles", "routing"])
    ))]
    Recover {
        /// Configuration or conflict file to install as profiles.json.
        #[arg(long, value_name = "FILE")]
        profiles: Option<PathBuf>,

        /// Routing file to install as routing.json.
        #[arg(long, value_name = "FILE")]
        routing: Option<PathBuf>,
    },
}

pub(super) fn run(command: &ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Reset {
            profiles,
            routing,
            yes,
        } => run_reset(*profiles, routing, *yes),
        ConfigCommand::Recover { profiles, routing } => run_recover(Change {
            profiles: profiles.clone().map(ProfilesSource::File),
            routing: routing.clone().map(RoutingSource::File),
        }),
    }
}

struct ConfigPaths {
    profiles: PathBuf,
    routing: PathBuf,
}

impl ConfigPaths {
    fn current() -> Result<Self> {
        Ok(Self {
            profiles: crate::paths::profiles_path().context("Failed to determine profiles path")?,
            routing: crate::paths::routing_path()
                .context("Failed to determine routing config path")?,
        })
    }

    fn recovery_dir(&self) -> PathBuf {
        self.profiles.with_file_name("recovery")
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ProfilesSource {
    FirstRun,
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
enum RoutingSource {
    Regions(Vec<GeoRegion>),
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
struct Change {
    profiles: Option<ProfilesSource>,
    routing: Option<RoutingSource>,
}

impl Change {
    fn reset(profiles: bool, regions: &[GeoRegion]) -> Option<Self> {
        let change = Self {
            profiles: profiles.then_some(ProfilesSource::FirstRun),
            routing: (!regions.is_empty()).then(|| RoutingSource::Regions(distinct(regions))),
        };
        (change.profiles.is_some() || change.routing.is_some()).then_some(change)
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
    profiles: Option<Config>,
    routing: Option<RoutingConfig>,
    mode: ModeOutcome,
    dropped_service_routes: bool,
}

impl Prepared {
    fn adjusts_profiles(&self) -> bool {
        self.change.profiles.is_none()
            && (matches!(self.mode, ModeOutcome::Switched(_)) || self.dropped_service_routes)
    }

    fn profiles_to_write(&self) -> Option<&Config> {
        self.profiles
            .as_ref()
            .filter(|_| self.change.profiles.is_some() || self.adjusts_profiles())
    }
}

struct Applied {
    profiles_archive: Option<PathBuf>,
    routing_archive: Option<PathBuf>,
}

fn require_daemon_stopped() -> Result<()> {
    anyhow::ensure!(
        !crate::ipc::is_daemon_running(),
        "stop the kvn daemon before changing profiles.json or routing.json: systemctl --user stop kvn-tui.service"
    );
    Ok(())
}

fn run_reset(profiles: bool, regions: &[GeoRegion], yes: bool) -> Result<()> {
    require_daemon_stopped()?;
    let paths = ConfigPaths::current()?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let change = match Change::reset(profiles, regions) {
        Some(change) => change,
        None => {
            anyhow::ensure!(stdin.is_terminal(), NEEDS_FLAGS);
            let current = regions_with_modes(&paths.routing);
            let invalid_profiles = current_profiles(&paths.profiles)
                .err()
                .map(|error| format!("{:#}", error.root_cause()));
            match ask_reset(
                &mut stdin.lock(),
                &mut stdout.lock(),
                &current,
                invalid_profiles.as_deref(),
            )? {
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

fn ask_reset<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    current_regions: &[GeoRegion],
    invalid_profiles: Option<&str>,
) -> Result<Option<Change>> {
    if let Some(problem) = invalid_profiles {
        writeln!(output, "profiles.json is invalid: {problem}")?;
    }
    let profiles = confirm(
        input,
        output,
        "Reset profiles.json (profiles, subscriptions and settings)?",
    )?;
    if invalid_profiles.is_some() && !profiles {
        writeln!(
            output,
            "routing.json can be reset only together with the invalid profiles.json"
        )?;
        return Ok(None);
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
    Ok(Change::reset(profiles, &regions))
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
        Some(ProfilesSource::FirstRun) => Some(Config::for_first_run()),
        Some(ProfilesSource::File(file)) => Some(read_profiles_file(file, &paths.profiles)?),
        None => current_profiles(&paths.profiles)?,
    };
    let mut prepared = Prepared {
        change,
        profiles,
        routing,
        mode: ModeOutcome::Unchecked,
        dropped_service_routes: false,
    };
    let Some(config) = prepared.profiles.as_mut() else {
        return Ok(prepared);
    };
    if matches!(prepared.change.routing, Some(RoutingSource::Regions(_))) {
        prepared.dropped_service_routes = config
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
            &config.settings.geo_routing,
            false,
        )
        .ok(),
    };
    if let Some(routing) = routing {
        prepared.mode = settle_mode(config, &routing);
    }
    Ok(prepared)
}

fn current_profiles(path: &Path) -> Result<Option<Config>> {
    if !path.exists() {
        return Ok(None);
    }
    crate::config::load_config_for_recovery(path)
        .map(Some)
        .context("profiles.json is invalid, so the routing mode and the old Steam and Telegram overrides it keeps cannot be updated; fix profiles.json or replace it too with --profiles")
}

fn settle_mode(config: &mut Config, routing: &RoutingConfig) -> ModeOutcome {
    let mode = &mut config.settings.geo_routing.mode;
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

fn read_profiles_file(file: &Path, target: &Path) -> Result<Config> {
    ensure_not_target(file, target, "profiles.json")?;
    crate::config::load_config_for_recovery(file)
        .with_context(|| format!("failed to load recovery file {}", file.display()))
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

fn apply(paths: &ConfigPaths, prepared: &Prepared) -> Result<Applied> {
    let profiles = prepared.profiles_to_write();
    let applied = Applied {
        profiles_archive: match profiles {
            Some(_) => archive_current(&paths.profiles, RecoveryKind::Archive)?,
            None => None,
        },
        routing_archive: match &prepared.routing {
            Some(_) => archive_current(&paths.routing, RecoveryKind::RoutingArchive)?,
            None => None,
        },
    };
    if let Some(routing) = &prepared.routing {
        crate::config::routing::replace_routing_at(&paths.routing, routing)
            .context("failed to write routing.json")?;
    }
    if let Some(config) = profiles {
        crate::config::save_config_at(&paths.profiles, config)
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
        prepared.change.routing.as_ref().map(|_| "routing.json"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn archived_files(prepared: &Prepared) -> Vec<&'static str> {
    [
        prepared.profiles_to_write().map(|_| "profiles.json"),
        prepared.routing.as_ref().map(|_| "routing.json"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn reset_question(prepared: &Prepared) -> String {
    format!("Reset {}?", reset_files(prepared).join(" and "))
}

fn reset_warning(prepared: &Prepared, recovery_dir: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    if prepared.change.profiles.is_some() {
        lines.push(
            "profiles.json: every profile, subscription and setting is replaced with the defaults of a fresh install.".to_string(),
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

fn report_lines(prepared: &Prepared, applied: &Applied, paths: &ConfigPaths) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(source) = &prepared.change.profiles {
        let written = match source {
            ProfilesSource::FirstRun => format!(
                "Created default profiles.json at {}",
                paths.profiles.display()
            ),
            ProfilesSource::File(file) => {
                format!("Recovered profiles.json from {}", file.display())
            }
        };
        lines.push(written + &archived_suffix(&applied.profiles_archive));
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
    if prepared.adjusts_profiles() {
        lines.push(format!(
            "Updated {}{}",
            paths.profiles.display(),
            archived_suffix(&applied.profiles_archive)
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
            profiles: dir.path().join("profiles.json"),
            routing: dir.path().join("routing.json"),
        }
    }

    fn write_profiles(paths: &ConfigPaths, mode: &str, service_routes: bool) {
        let mut config = Config::default();
        config.settings.geo_routing.mode = mode.to_string();
        if service_routes {
            config.settings.geo_routing.legacy_service_routes =
                Some(serde_json::json!({ "steam": "direct" }));
        }
        crate::config::save_config_at(&paths.profiles, &config).unwrap();
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

    fn saved_profiles(paths: &ConfigPaths) -> Config {
        crate::config::load_config_for_recovery(&paths.profiles).unwrap()
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

    fn reset_routing(regions: &[GeoRegion]) -> Change {
        Change::reset(false, regions).unwrap()
    }

    #[test]
    fn parses_reset_targets() {
        assert!(matches!(
            parse(&["reset", "--routing", "ru,cn", "--routing", "ir", "--yes"]).unwrap(),
            ConfigCommand::Reset { profiles: false, routing, yes: true }
                if routing == [GeoRegion::Ru, GeoRegion::Cn, GeoRegion::Ir]
        ));
        assert!(matches!(
            parse(&["reset", "--profiles"]).unwrap(),
            ConfigCommand::Reset { profiles: true, routing, yes: false } if routing.is_empty()
        ));
        assert!(matches!(
            parse(&["reset"]).unwrap(),
            ConfigCommand::Reset { profiles: false, routing, yes: false } if routing.is_empty()
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
            parse(&["recover", "--profiles", "p.json", "--routing", "r.json"]).unwrap(),
            ConfigCommand::Recover { profiles: Some(profiles), routing: Some(routing) }
                if profiles == Path::new("p.json") && routing == Path::new("r.json")
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
        let change = ask_reset(&mut answers.as_bytes(), &mut output, current, None).unwrap();
        (change, String::from_utf8(output).unwrap())
    }

    #[test]
    fn questions_choose_files_and_countries() {
        assert_eq!(ask("n\nn\n", &[]).0, None);
        assert_eq!(ask("y\nn\n", &[]).0, Change::reset(true, &[]));
        let (change, output) = ask("n\ny\nxx\nru,cn,ru\n", &[GeoRegion::Ru]);
        assert_eq!(change, Some(reset_routing(&[GeoRegion::Ru, GeoRegion::Cn])));
        assert!(output.contains("[current: ru]"));
        assert!(output.contains("Unknown country `xx`"));
    }

    #[test]
    fn an_empty_country_answer_cancels_the_whole_reset() {
        assert_eq!(ask("y\ny\n\n", &[]).0, None);
        assert_eq!(ask("y\ny\n", &[]).0, None);
    }

    #[test]
    fn an_invalid_profiles_json_is_named_before_the_questions() {
        let mut output = Vec::new();
        let change = ask_reset(&mut "n\n".as_bytes(), &mut output, &[], Some("EOF")).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert_eq!(change, None);
        assert!(output.starts_with("profiles.json is invalid: EOF\n"));
        assert!(output.contains("only together with the invalid profiles.json"));
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
        write_profiles(&paths, "bypass_cn", true);
        let previous = write_routing(&paths, &[GeoRegion::Ru, GeoRegion::Cn]);

        let prepared = prepare(&paths, reset_routing(&[GeoRegion::Ru])).unwrap();
        assert_eq!(prepared.mode, ModeOutcome::Switched("bypass_cn".into()));
        assert!(prepared.dropped_service_routes);
        let applied = apply(&paths, &prepared).unwrap();

        assert_eq!(
            saved_routing(&paths),
            RoutingConfig::with_region_modes(&[GeoRegion::Ru])
        );
        let profiles = saved_profiles(&paths);
        assert_eq!(profiles.settings.geo_routing.mode, GLOBAL_MODE_ID);
        assert_eq!(profiles.settings.geo_routing.legacy_service_routes, None);
        let archive = applied.routing_archive.unwrap();
        assert_eq!(
            crate::config::routing::load_routing_for_recovery(&archive).unwrap(),
            previous
        );
        let archived_profiles = applied.profiles_archive.unwrap();
        assert_eq!(
            crate::config::load_config_for_recovery(&archived_profiles)
                .unwrap()
                .settings
                .geo_routing
                .mode,
            "bypass_cn"
        );
    }

    #[test]
    fn routing_reset_keeps_a_mode_that_still_exists_and_leaves_profiles_alone() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_profiles(&paths, "bypass_ru", false);
        let before = fs::read(&paths.profiles).unwrap();
        write_routing(&paths, &[GeoRegion::Ru]);

        let prepared = prepare(&paths, reset_routing(&[GeoRegion::Ru])).unwrap();
        let applied = apply(&paths, &prepared).unwrap();

        assert_eq!(prepared.mode, ModeOutcome::Kept("Bypass RU".into()));
        assert_eq!(applied.profiles_archive, None);
        assert_eq!(fs::read(&paths.profiles).unwrap(), before);
    }

    #[test]
    fn an_invalid_routing_json_is_reset_like_a_valid_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_profiles(&paths, "global", false);
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
    fn a_routing_change_is_refused_while_profiles_json_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        fs::write(&paths.profiles, "{").unwrap();
        write_routing(&paths, &[GeoRegion::Ru]);
        let routing_before = fs::read(&paths.routing).unwrap();

        let error = prepare(&paths, reset_routing(&[GeoRegion::Ru]))
            .err()
            .unwrap();

        assert!(format!("{error:#}").contains("profiles.json is invalid"));
        assert_eq!(fs::read(&paths.routing).unwrap(), routing_before);
        assert_eq!(fs::read_to_string(&paths.profiles).unwrap(), "{");
    }

    #[test]
    fn profiles_reset_archives_the_old_file() {
        let _lock = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        fs::write(&paths.profiles, "{").unwrap();

        let prepared = prepare(&paths, Change::reset(true, &[]).unwrap()).unwrap();
        let applied = apply(&paths, &prepared).unwrap();

        assert_eq!(saved_profiles(&paths).profiles.len(), 0);
        assert_eq!(
            fs::read_to_string(applied.profiles_archive.unwrap()).unwrap(),
            "{"
        );
        assert!(!paths.routing.exists());
    }

    #[test]
    fn recovering_routing_json_checks_the_file_and_archives_the_current_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_profiles(&paths, "work", false);
        let current = write_routing(&paths, &[GeoRegion::Ru]);
        let saved = dir.path().join("saved.json");
        fs::write(&saved, r#"{ "schema_version": 1, "modes": [] }"#).unwrap();
        let recover = |file: &Path| Change {
            profiles: None,
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
    fn recovering_profiles_json_switches_a_mode_routing_json_lacks() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_routing(&paths, &[GeoRegion::Ru]);
        let saved = dir.path().join("saved.json");
        let mut config = Config::default();
        config.settings.geo_routing.mode = "only_cn".into();
        crate::config::save_config_at(&saved, &config).unwrap();

        let change = Change {
            profiles: Some(ProfilesSource::File(saved)),
            routing: None,
        };
        let prepared = prepare(&paths, change).unwrap();
        apply(&paths, &prepared).unwrap();

        assert_eq!(prepared.mode, ModeOutcome::Switched("only_cn".into()));
        assert_eq!(
            saved_profiles(&paths).settings.geo_routing.mode,
            GLOBAL_MODE_ID
        );
    }

    #[test]
    fn the_warning_names_what_each_reset_removes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_profiles(&paths, "bypass_cn", true);
        let prepared = prepare(&paths, reset_routing(&[GeoRegion::Ru])).unwrap();

        let warning = reset_warning(&prepared, Path::new("/recovery")).join("\n");

        assert!(warning.contains("it keeps Global and the modes of RU."));
        assert!(warning.contains("`bypass_cn` will no longer exist"));
        assert!(warning.contains("Steam and Telegram overrides"));
        assert!(warning.contains("in /recovery"));
        assert!(warning.contains(
            "`kvn config recover --profiles <file>` or `kvn config recover --routing <file>`"
        ));
        assert!(!warning.contains("profiles.json:"));
        assert_eq!(reset_question(&prepared), "Reset routing.json?");
    }

    #[test]
    fn the_report_names_written_and_archived_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(&dir);
        write_profiles(&paths, "bypass_ru", false);
        write_routing(&paths, &[GeoRegion::Ru]);
        let prepared = prepare(&paths, reset_routing(&[GeoRegion::Ru, GeoRegion::Cn])).unwrap();
        let applied = apply(&paths, &prepared).unwrap();

        let report = report_lines(&prepared, &applied, &paths).join("\n");

        assert!(report.contains("Created routing.json with Global and the modes of RU, CN"));
        assert!(report.contains("previous file archived at"));
        assert!(report.contains("Routing mode Bypass RU kept"));
    }
}
