use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
pub(crate) mod json_pointer;
pub(crate) mod merge;
pub mod profile;
pub(crate) mod recovery;
pub mod routing;
pub mod subscription;

use json_pointer::JsonIndex;
use profile::{
    CURRENT_SCHEMA_VERSION, Config, ConfigDiagnostic, ConfigFile, ProfilesDocument,
    SETTINGS_SCHEMA_VERSION, SettingsDocument,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub profiles: PathBuf,
    pub settings: PathBuf,
}

impl ConfigPaths {
    pub fn current() -> Result<Self> {
        Ok(Self {
            profiles: crate::paths::profiles_path().context("Failed to determine profiles path")?,
            settings: crate::paths::settings_path().context("Failed to determine settings path")?,
        })
    }

    #[cfg(test)]
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            profiles: dir.join("profiles.json"),
            settings: dir.join("settings.json"),
        }
    }

    pub fn of(&self, file: ConfigFile) -> &Path {
        match file {
            ConfigFile::Profiles => &self.profiles,
            ConfigFile::Settings => &self.settings,
        }
    }
}

pub fn config_file_path(file: ConfigFile) -> Result<PathBuf> {
    Ok(ConfigPaths::current()?.of(file).to_path_buf())
}

#[derive(Debug)]
pub struct ConfigLoad {
    pub config: Config,
    pub profiles_error: Option<anyhow::Error>,
    pub settings_error: Option<anyhow::Error>,
}

impl ConfigLoad {
    pub fn error(&self, file: ConfigFile) -> Option<&anyhow::Error> {
        match file {
            ConfigFile::Profiles => self.profiles_error.as_ref(),
            ConfigFile::Settings => self.settings_error.as_ref(),
        }
    }

    pub fn into_result(self) -> Result<Config> {
        match (self.profiles_error, self.settings_error) {
            (None, None) => Ok(self.config),
            (Some(error), None) | (None, Some(error)) => Err(error),
            (Some(profiles), Some(settings)) => Err(profiles.context(format!("{settings:#}"))),
        }
    }

    fn failed(error: anyhow::Error) -> Self {
        let settings_error = anyhow::anyhow!("{error:#}");
        Self {
            config: Config::default(),
            profiles_error: Some(error),
            settings_error: Some(settings_error),
        }
    }
}

const LAST_SCHEMA_WITHOUT_HWID: u32 = 4;
const COMBINED_KEYS: [&str; 3] = ["settings", "profiles", "subscriptions"];

#[derive(serde::Deserialize)]
struct VersionProbe {
    schema_version: Option<u32>,
}

fn schema_version_of(contents: &str, path: &Path) -> Result<u32> {
    let probe: VersionProbe =
        serde_json::from_str(contents).with_context(|| format!("Failed to parse {path:?}"))?;
    probe
        .schema_version
        .with_context(|| format!("{path:?} has no schema_version"))
}

fn ensure_supported(file: ConfigFile, version: u32, path: &Path) -> Result<()> {
    let supported = match file {
        ConfigFile::Profiles => CURRENT_SCHEMA_VERSION,
        ConfigFile::Settings => SETTINGS_SCHEMA_VERSION,
    };
    anyhow::ensure!(
        version <= supported,
        "config {path:?} uses schema_version {version}, newer than this build supports (max {supported}); upgrade kvn"
    );
    Ok(())
}

fn profiles_version(contents: &str, path: &Path) -> Result<u32> {
    let version = schema_version_of(contents, path)?;
    ensure_supported(ConfigFile::Profiles, version, path)?;
    Ok(version)
}

fn is_combined(version: u32) -> bool {
    version < CURRENT_SCHEMA_VERSION
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Failed to read {path:?}")),
    }
}

fn located(contents: &str, diagnostics: Vec<ConfigDiagnostic>) -> Result<()> {
    if diagnostics.is_empty() {
        return Ok(());
    }
    let index = JsonIndex::new(contents);
    let problems: Vec<String> = diagnostics
        .into_iter()
        .map(|diagnostic| match index.nearest_line(&diagnostic.pointer) {
            Some(line) => format!("line {line}: {}", diagnostic.message),
            None => diagnostic.message,
        })
        .collect();
    anyhow::bail!("{}", problems.join("; "))
}

fn parse_combined(contents: &str, version: u32, path: &Path) -> Result<Config> {
    let mut config: Config =
        serde_json::from_str(contents).with_context(|| format!("Failed to parse {path:?}"))?;
    config
        .migrate()
        .with_context(|| format!("Failed to migrate {path:?}"))?;
    if version <= LAST_SCHEMA_WITHOUT_HWID && config.settings.hwid.is_empty() {
        config.settings.hwid = subscription::generate_hwid();
    }
    config.normalize();
    Ok(config)
}

fn parse_profiles(contents: &str, version: u32, path: &Path) -> Result<ProfilesDocument> {
    if is_combined(version) {
        return Ok(parse_combined(contents, version, path)?.profiles_document());
    }
    serde_json::from_str(contents).with_context(|| format!("Failed to parse {path:?}"))
}

pub(crate) fn check_settings(contents: &str, path: &Path) -> Result<SettingsDocument> {
    let document: serde_json::Value =
        serde_json::from_str(contents).with_context(|| format!("Failed to parse {path:?}"))?;
    if let Some(version) = document
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    {
        ensure_supported(
            ConfigFile::Settings,
            u32::try_from(version).unwrap_or(u32::MAX),
            path,
        )?;
    }
    located(contents, profile::settings_schema_diagnostics(&document))
        .with_context(|| format!("Invalid {path:?}"))?;
    serde_json::from_value(document).with_context(|| format!("Failed to parse {path:?}"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Persistence {
    Write,
    ReadOnly,
}

struct LoadedFile {
    contents: Option<String>,
    error: Option<anyhow::Error>,
}

impl LoadedFile {
    fn read(path: &Path) -> Self {
        match read_optional(path) {
            Ok(contents) => Self {
                contents,
                error: None,
            },
            Err(error) => Self {
                contents: None,
                error: Some(error),
            },
        }
    }

    fn failed(error: anyhow::Error) -> Self {
        Self {
            contents: None,
            error: Some(error),
        }
    }

    fn is_missing(&self) -> bool {
        self.contents.is_none() && self.error.is_none()
    }

    fn parse<T>(&mut self, parse: impl FnOnce(&str) -> Result<T>) -> Option<T> {
        let contents = self.contents.as_deref()?;
        match parse(contents) {
            Ok(document) => Some(document),
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    fn check(&mut self, file: ConfigFile, path: &Path, config: &Config) {
        if self.error.is_some() {
            return;
        }
        let contents = self.contents.as_deref().unwrap_or_default();
        if let Err(problem) = located(contents, config.file_diagnostics(file)) {
            self.error = Some(problem.context(format!("Invalid {path:?}")));
        }
    }

    fn persist(&mut self, file: ConfigFile, path: &Path, loaded: &Config, config: &Config) {
        if self.error.is_some() {
            return;
        }
        let expected = self.contents.as_deref().map(str::as_bytes);
        if expected.is_some() && loaded.with_file_of(file, config) == *loaded {
            return;
        }
        if let Err(error) = save_file_at_revision(file, path, config, expected) {
            self.error = Some(error.context(format!("Failed to save {path:?}")));
        }
    }
}

pub fn load_config_files_at(paths: &ConfigPaths) -> ConfigLoad {
    load_files(paths, Persistence::Write)
}

pub fn load_config_read_only_at(paths: &ConfigPaths) -> ConfigLoad {
    load_files(paths, Persistence::ReadOnly)
}

pub fn load_config() -> ConfigLoad {
    match ConfigPaths::current() {
        Ok(paths) => load_config_files_at(&paths),
        Err(error) => ConfigLoad::failed(error),
    }
}

pub(crate) fn load_config_read_only() -> ConfigLoad {
    match ConfigPaths::current() {
        Ok(paths) => load_config_read_only_at(&paths),
        Err(error) => ConfigLoad::failed(error),
    }
}

fn load_files(paths: &ConfigPaths, persistence: Persistence) -> ConfigLoad {
    let mut profiles = LoadedFile::read(&paths.profiles);
    let version = profiles.parse(|contents| profiles_version(contents, &paths.profiles));
    match (version, profiles.contents.as_deref()) {
        (Some(version), Some(contents)) if is_combined(version) => {
            match load_combined(paths, contents, version, persistence) {
                Ok(config) => ConfigLoad {
                    config,
                    profiles_error: None,
                    settings_error: None,
                },
                Err(error) => ConfigLoad::failed(error),
            }
        }
        (version, _) => load_split(paths, profiles, version, persistence),
    }
}

fn load_combined(
    paths: &ConfigPaths,
    contents: &str,
    version: u32,
    persistence: Persistence,
) -> Result<Config> {
    let config = parse_combined(contents, version, &paths.profiles)?;
    config
        .validate()
        .with_context(|| format!("Migrated config {:?} is invalid", paths.profiles))?;
    if persistence == Persistence::Write {
        split_combined(paths, contents, &config)?;
    }
    Ok(config)
}

fn split_combined(paths: &ConfigPaths, contents: &str, config: &Config) -> Result<()> {
    recovery::preserve_at(
        &paths.profiles,
        recovery::RecoveryKind::Migration,
        contents.as_bytes(),
    )
    .with_context(|| {
        format!(
            "Failed to preserve {:?} before splitting it",
            paths.profiles
        )
    })?;
    replace_settings_preserving_other(paths, config)?;
    save_file_at_revision(
        ConfigFile::Profiles,
        &paths.profiles,
        config,
        Some(contents.as_bytes()),
    )
    .with_context(|| format!("Failed to persist split config {:?}", paths.profiles))?;
    tracing::info!("Moved settings from profiles.json to settings.json");
    Ok(())
}

fn replace_settings_preserving_other(paths: &ConfigPaths, config: &Config) -> Result<()> {
    let json = checked_file_json(ConfigFile::Settings, config)?;
    if let Some(existing) = read_optional(&paths.settings)?
        && existing != json
    {
        recovery::preserve_at(
            &paths.profiles,
            recovery::RecoveryKind::SettingsArchive,
            existing.as_bytes(),
        )
        .with_context(|| {
            format!(
                "Failed to preserve {:?} before replacing it",
                paths.settings
            )
        })?;
    }
    write_file(&paths.settings, json.as_bytes(), None)
        .with_context(|| format!("Failed to write {:?}", paths.settings))
}

fn load_split(
    paths: &ConfigPaths,
    mut profiles: LoadedFile,
    version: Option<u32>,
    persistence: Persistence,
) -> ConfigLoad {
    let mut settings = LoadedFile::read(&paths.settings);
    let first_run_settings = || Config::for_first_run().settings_document();
    if settings.is_missing() && profiles.error.is_some() {
        settings = LoadedFile::failed(anyhow::anyhow!(
            "settings are kept in {:?} until it loads",
            paths.profiles
        ));
    }
    let profiles_document = match version {
        Some(version) => profiles
            .parse(|contents| parse_profiles(contents, version, &paths.profiles))
            .unwrap_or_default(),
        None => ProfilesDocument::default(),
    };
    let settings_missing = settings.is_missing();
    let settings_document = if settings_missing {
        first_run_settings()
    } else {
        settings
            .parse(|contents| check_settings(contents, &paths.settings))
            .unwrap_or_default()
    };
    let loaded = Config::from_documents(profiles_document, settings_document);
    let mut config = loaded.clone();
    config.normalize();
    profiles.check(ConfigFile::Profiles, &paths.profiles, &config);
    settings.check(ConfigFile::Settings, &paths.settings, &config);
    if persistence == Persistence::Write {
        if profiles.contents.is_some() {
            profiles.persist(ConfigFile::Profiles, &paths.profiles, &loaded, &config);
        }
        settings.persist(ConfigFile::Settings, &paths.settings, &loaded, &config);
    }
    if settings_missing && settings.error.is_none() {
        tracing::info!("Created settings.json with a new installation HWID");
    }
    if profiles.error.is_some() {
        config = config.with_profiles(ProfilesDocument::default());
    }
    if settings.error.is_some() {
        config = config.with_settings(SettingsDocument::default());
    }
    ConfigLoad {
        config,
        profiles_error: profiles.error,
        settings_error: settings.error,
    }
}

pub(crate) fn split_combined_at(paths: &ConfigPaths, new_hwid: bool) -> Result<bool> {
    let Some(contents) = read_optional(&paths.profiles)? else {
        return Ok(false);
    };
    let version = profiles_version(&contents, &paths.profiles)?;
    if !is_combined(version) {
        return Ok(false);
    }
    let mut config = parse_combined(&contents, version, &paths.profiles)?;
    if new_hwid {
        config.settings.hwid = subscription::generate_hwid();
    }
    config
        .validate()
        .with_context(|| format!("Migrated config {:?} is invalid", paths.profiles))?;
    split_combined(paths, &contents, &config)?;
    Ok(true)
}

fn validated_profiles(document: ProfilesDocument, path: &Path) -> Result<ProfilesDocument> {
    Config::from_documents(document.clone(), SettingsDocument::default())
        .validate_file(ConfigFile::Profiles)
        .with_context(|| format!("Invalid {path:?}"))?;
    Ok(document)
}

fn validated_settings(document: SettingsDocument, path: &Path) -> Result<SettingsDocument> {
    Config::from_documents(ProfilesDocument::default(), document.clone())
        .validate_file(ConfigFile::Settings)
        .with_context(|| format!("Invalid {path:?}"))?;
    Ok(document)
}

pub(crate) fn load_profiles_for_recovery(path: &Path) -> Result<ProfilesDocument> {
    let contents = fs::read_to_string(path).with_context(|| format!("Failed to read {path:?}"))?;
    let version = profiles_version(&contents, path)?;
    validated_profiles(parse_profiles(&contents, version, path)?, path)
}

fn combined_version(contents: &str, path: &Path) -> Result<Option<u32>> {
    let document: serde_json::Value =
        serde_json::from_str(contents).with_context(|| format!("Failed to parse {path:?}"))?;
    let holds_profiles = COMBINED_KEYS
        .into_iter()
        .any(|key| document.get(key).is_some());
    if !holds_profiles {
        return Ok(None);
    }
    let version = profiles_version(contents, path)?;
    Ok(is_combined(version).then_some(version))
}

fn unchecked_settings(contents: &str, path: &Path) -> Result<SettingsDocument> {
    match combined_version(contents, path)? {
        Some(version) => Ok(parse_combined(contents, version, path)?.settings_document()),
        None => check_settings(contents, path),
    }
}

pub(crate) fn load_settings_for_recovery(path: &Path) -> Result<SettingsDocument> {
    let contents = fs::read_to_string(path).with_context(|| format!("Failed to read {path:?}"))?;
    validated_settings(unchecked_settings(&contents, path)?, path)
}

pub(crate) fn current_settings_with_new_hwid(paths: &ConfigPaths) -> Result<SettingsDocument> {
    let contents = read_optional(&paths.settings)?.with_context(|| {
        format!(
            "{:?} does not exist; kvn creates it with a new HWID on the next start",
            paths.settings
        )
    })?;
    let mut document = unchecked_settings(&contents, &paths.settings)?;
    document.settings.hwid = subscription::generate_hwid();
    validated_settings(document, &paths.settings)
}

pub(crate) fn read_profiles_document(contents: &[u8], path: &Path) -> Result<ProfilesDocument> {
    let contents = std::str::from_utf8(contents)
        .with_context(|| format!("Config {path:?} is not valid UTF-8"))?;
    let version = profiles_version(contents, path)?;
    parse_profiles(contents, version, path)
}

pub(crate) fn read_settings_document(contents: &[u8], path: &Path) -> Result<SettingsDocument> {
    let contents = std::str::from_utf8(contents)
        .with_context(|| format!("Config {path:?} is not valid UTF-8"))?;
    check_settings(contents, path)
}

pub(crate) fn file_json(
    file: ConfigFile,
    config: &Config,
    json_schema: Option<String>,
) -> Result<String> {
    let mut normalized = config.clone();
    normalized.normalize();
    normalized_file_json(file, &normalized, json_schema)
}

fn normalized_file_json(
    file: ConfigFile,
    normalized: &Config,
    json_schema: Option<String>,
) -> Result<String> {
    Ok(match file {
        ConfigFile::Profiles => serde_json::to_string_pretty(&ProfilesDocument {
            json_schema,
            ..normalized.profiles_document()
        })?,
        ConfigFile::Settings => serde_json::to_string_pretty(&SettingsDocument {
            json_schema,
            ..normalized.settings_document()
        })?,
    })
}

fn checked_file_json(file: ConfigFile, config: &Config) -> Result<String> {
    let mut normalized = config.clone();
    normalized.normalize();
    normalized
        .validate_file(file)
        .with_context(|| format!("Refusing to save invalid {}", file.name()))?;
    normalized_file_json(file, &normalized, None)
}

fn write_file(path: &Path, contents: &[u8], expected: Option<Option<&[u8]>>) -> Result<()> {
    let dir = path.parent().context("Invalid config path")?;
    fs::create_dir_all(dir)?;
    match expected {
        Some(expected) => crate::atomic_write::write_if_unchanged(path, contents, expected),
        None => crate::atomic_write::write(path, contents),
    }
}

pub fn save_file_at(file: ConfigFile, path: &Path, config: &Config) -> Result<()> {
    write_file(path, checked_file_json(file, config)?.as_bytes(), None)
}

pub(crate) fn save_file_at_revision(
    file: ConfigFile,
    path: &Path,
    config: &Config,
    expected: Option<&[u8]>,
) -> Result<()> {
    write_file(
        path,
        checked_file_json(file, config)?.as_bytes(),
        Some(expected),
    )
}

pub(crate) fn save_editor_snapshot_at(
    file: ConfigFile,
    path: &Path,
    config: &Config,
    json_schema: Option<String>,
) -> Result<()> {
    let json = file_json(file, config, json_schema)?;
    crate::atomic_write::write(path, json.as_bytes())
}

pub fn load_routing(
    geo_routing: &profile::GeoRouting,
    create_missing: bool,
) -> Result<routing::RoutingConfig> {
    let path = crate::paths::routing_path().context("Failed to determine routing config path")?;
    routing::load_routing_at(&path, geo_routing, create_missing)
}

pub fn move_service_routes(
    config: &mut Config,
    routing: &mut routing::RoutingConfig,
) -> Result<bool> {
    let settings_path = config_file_path(ConfigFile::Settings)?;
    let routing_path =
        crate::paths::routing_path().context("Failed to determine routing config path")?;
    move_service_routes_at(&settings_path, &routing_path, config, routing)
}

fn move_service_routes_at(
    settings_path: &Path,
    routing_path: &Path,
    config: &mut Config,
    routing: &mut routing::RoutingConfig,
) -> Result<bool> {
    let Some(legacy) = config.settings.geo_routing.legacy_service_routes.clone() else {
        return Ok(false);
    };
    *routing = routing::update_routing_at(routing_path, |routing| {
        routing.adopt_service_routes(&legacy)
    })?;
    config.settings.geo_routing.legacy_service_routes = None;
    save_file_at(ConfigFile::Settings, settings_path, config)?;
    Ok(true)
}

#[cfg(test)]
pub fn save_config(config: &Config) -> Result<()> {
    let paths = ConfigPaths::current()?;
    save_config_files_at(&paths, config)
}

#[cfg(test)]
pub fn save_config_files_at(paths: &ConfigPaths, config: &Config) -> Result<()> {
    save_file_at(ConfigFile::Profiles, &paths.profiles, config)?;
    save_file_at(ConfigFile::Settings, &paths.settings, config)
}

pub(crate) fn save_conflict_file(file: ConfigFile, config: &Config) -> Result<PathBuf> {
    let json = checked_file_json(file, config)?;
    recovery::preserve(recovery::RecoveryKind::conflict(file), json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::saved_config;
    use profile::Settings;
    use std::path::PathBuf;

    fn dir_paths(dir: &tempfile::TempDir) -> ConfigPaths {
        ConfigPaths::in_dir(dir.path())
    }

    fn load(paths: &ConfigPaths) -> Config {
        load_config_files_at(paths).into_result().unwrap()
    }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn recovery_files(paths: &ConfigPaths, prefix: &str) -> Vec<PathBuf> {
        fs::read_dir(paths.profiles.with_file_name("recovery"))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
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

    fn combined_v8(theme: &str) -> String {
        format!(
            r#"{{"schema_version":8,"profiles":[{{"name":"A","protocol":"vless","address":"a","port":1,"uuid":"{}"}}],"settings":{{"theme":"{theme}","hwid":"lnx-d415f1264a264924aa76e7f55380b0c7"}}}}"#,
            crate::test_helpers::TEST_UUID
        )
    }

    #[test]
    fn editor_snapshot_keeps_a_config_that_fails_validation() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::test_helpers::saved_config();
        config
            .subscriptions
            .push(crate::test_helpers::subscription_with_hwid(
                true,
                Some("abc123"),
            ));
        assert!(config.validate().is_err());

        save_editor_snapshot_at(
            ConfigFile::Profiles,
            &dir.path().join("snapshot.json"),
            &config,
            None,
        )
        .unwrap();
        assert!(
            save_file_at(
                ConfigFile::Profiles,
                &dir.path().join("profiles.json"),
                &config
            )
            .is_err()
        );
    }

    #[test]
    fn schema_reference_reaches_only_editor_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            json_schema: Some("file:///run/user/1000/kvn/profiles.schema.json".into()),
            ..crate::test_helpers::saved_config()
        };
        let profiles = dir.path().join("profiles.json");
        save_file_at(ConfigFile::Profiles, &profiles, &config).unwrap();
        assert!(!fs::read_to_string(&profiles).unwrap().contains("$schema"));
        let snapshot = dir.path().join("snapshot.json");
        save_editor_snapshot_at(
            ConfigFile::Settings,
            &snapshot,
            &crate::test_helpers::saved_config(),
            Some("file:///run/user/1000/kvn/settings.schema.json".into()),
        )
        .unwrap();
        assert!(fs::read_to_string(&snapshot).unwrap().starts_with(
            "{\n  \"$schema\": \"file:///run/user/1000/kvn/settings.schema.json\",\n  \"schema_version\": 1,"
        ));
    }

    #[test]
    fn config_dir_matches_paths_module() {
        let from_paths = crate::paths::config_dir();
        assert!(from_paths.is_some());
    }

    #[test]
    fn missing_files_load_first_run_preferences() {
        let dir = tempfile::tempdir().unwrap();
        for config in [
            load_config_files_at(&dir_paths(&dir))
                .into_result()
                .unwrap(),
            load_config_read_only_at(&ConfigPaths::in_dir(Path::new("/nonexistent/path")))
                .into_result()
                .unwrap(),
        ] {
            assert!(config.settings.hwid.starts_with("lnx-"));
            assert!(config.profiles.is_empty());
            assert_eq!(config.settings.tun_interface, "kvn0");
            assert_eq!(
                config.settings.geo_routing.auto_update,
                profile::GeoAutoUpdate::Every7d
            );
        }
    }

    #[test]
    fn each_file_keeps_its_own_part_of_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        let mut config = crate::test_helpers::saved_config();
        config.settings.hwid = "lnx-d415f1264a264924aa76e7f55380b0c7".into();
        config.settings.geo_routing.set_mode("only_ru");
        let profile = profile::Profile::new_vless(
            "Test".to_string(),
            "1.2.3.4".to_string(),
            443,
            crate::test_helpers::TEST_UUID.to_string(),
        );
        config.settings.last_connected_profile = Some(profile.id);
        config.profiles.push(profile);

        save_config_files_at(&paths, &config).unwrap();

        assert_eq!(load(&paths), config);
        let profiles = read_json(&paths.profiles);
        assert_eq!(profiles["schema_version"], CURRENT_SCHEMA_VERSION);
        assert!(profiles.get("settings").is_none());
        assert_eq!(
            profiles["last_connected_profile"],
            config.profiles[0].id.to_string()
        );
        let settings = read_json(&paths.settings);
        assert_eq!(settings["schema_version"], SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings["geo_routing"]["mode"], "only_ru");
        assert!(settings.get("last_connected_profile").is_none());
    }

    #[test]
    fn load_trims_and_persists_tun_interface() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        save_config_files_at(&paths, &crate::test_helpers::saved_config()).unwrap();
        let profiles = fs::read(&paths.profiles).unwrap();
        let mut settings = read_json(&paths.settings);
        settings["tun_interface"] = " \tkvn0\n".into();
        fs::write(&paths.settings, settings.to_string()).unwrap();

        assert_eq!(load(&paths).settings.tun_interface, "kvn0");
        assert_eq!(read_json(&paths.settings)["tun_interface"], "kvn0");
        assert_eq!(fs::read(&paths.profiles).unwrap(), profiles);
    }

    #[test]
    fn save_trims_tun_interface_and_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::test_helpers::saved_config();
        config.settings.tun_interface = " kvn-work ".into();
        let nested = dir.path().join("a/b/settings.json");

        save_file_at(ConfigFile::Settings, &nested, &config).unwrap();

        assert_eq!(read_json(&nested)["tun_interface"], "kvn-work");
    }

    #[test]
    fn a_broken_profiles_json_blocks_settings_only_while_they_may_live_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        fs::write(&paths.profiles, "not json at all").unwrap();

        let load = load_config_files_at(&paths);
        assert!(load.profiles_error.is_some());
        assert!(load.settings_error.is_some());
        assert!(!paths.settings.exists());

        let mut stored = crate::test_helpers::saved_config();
        stored.settings.theme = "nord".into();
        save_file_at(ConfigFile::Settings, &paths.settings, &stored).unwrap();
        let load = load_config_files_at(&paths);
        assert!(load.profiles_error.is_some());
        assert!(load.settings_error.is_none());
        assert_eq!(load.config.settings.theme, "nord");
        assert_eq!(
            fs::read_to_string(&paths.profiles).unwrap(),
            "not json at all"
        );
    }

    #[test]
    fn an_invalid_settings_json_is_reported_with_its_line_and_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        let mut stored = crate::test_helpers::saved_config();
        stored.profiles.push(profile::Profile::new_vless(
            "Kept".into(),
            "a".into(),
            1,
            crate::test_helpers::TEST_UUID.into(),
        ));
        save_file_at(ConfigFile::Profiles, &paths.profiles, &stored).unwrap();
        let invalid = "{\n  \"schema_version\": 1,\n  \"theme\": \"missing\"\n}";
        fs::write(&paths.settings, invalid).unwrap();

        let load = load_config_files_at(&paths);

        let error = format!("{:#}", load.settings_error.unwrap());
        assert!(error.contains("line 3: /theme: must be one of"), "{error}");
        assert!(load.profiles_error.is_none());
        assert_eq!(load.config.profiles[0].name, "Kept");
        assert_eq!(load.config.settings, Settings::default());
        assert_eq!(fs::read_to_string(&paths.settings).unwrap(), invalid);
    }

    #[test]
    fn a_settings_json_without_hwid_is_reported_instead_of_getting_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        save_file_at(ConfigFile::Profiles, &paths.profiles, &Config::default()).unwrap();
        fs::write(
            &paths.settings,
            r#"{ "schema_version": 1, "theme": "nord" }"#,
        )
        .unwrap();

        let load = load_config_files_at(&paths);

        let error = format!("{:#}", load.settings_error.unwrap());
        assert!(error.contains("kvn config reset --hwid"), "{error}");
        assert!(load.config.settings.hwid.is_empty());
        assert_eq!(
            fs::read_to_string(&paths.settings).unwrap(),
            r#"{ "schema_version": 1, "theme": "nord" }"#
        );
    }

    #[test]
    fn a_profiles_json_without_schema_version_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        fs::write(&paths.profiles, r#"{ "profiles": [] }"#).unwrap();
        save_file_at(ConfigFile::Settings, &paths.settings, &saved_config()).unwrap();
        let settings = fs::read(&paths.settings).unwrap();

        let load = load_config_files_at(&paths);

        let error = format!("{:#}", load.profiles_error.unwrap());
        assert!(error.contains("has no schema_version"), "{error}");
        assert!(load.settings_error.is_none());
        assert_eq!(fs::read(&paths.settings).unwrap(), settings);
    }

    #[test]
    fn settings_next_to_a_broken_profiles_json_get_every_check() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        fs::write(&paths.profiles, "{").unwrap();
        fs::write(
            &paths.settings,
            format!(
                r#"{{ "schema_version": 1, "hwid": "{}", "tun_interface": "eth0" }}"#,
                crate::test_helpers::TEST_HWID
            ),
        )
        .unwrap();

        let load = load_config_files_at(&paths);

        let error = format!("{:#}", load.settings_error.unwrap());
        assert!(error.contains("tun_interface"), "{error}");
    }

    #[test]
    fn an_old_profiles_json_without_hwid_splits_only_with_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        let original = r#"{"schema_version":8,"profiles":[],"settings":{"theme":"nord"}}"#;
        fs::write(&paths.profiles, original).unwrap();

        assert!(load_config_files_at(&paths).settings_error.is_some());
        assert_eq!(fs::read_to_string(&paths.profiles).unwrap(), original);

        assert!(split_combined_at(&paths, true).unwrap());
        let settings = read_json(&paths.settings);
        assert_eq!(settings["theme"], "nord");
        assert!(settings["hwid"].as_str().unwrap().starts_with("lnx-"));
    }

    #[test]
    fn recovering_settings_refuses_a_current_profiles_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        save_file_at(ConfigFile::Profiles, &path, &saved_config()).unwrap();

        assert!(load_settings_for_recovery(&path).is_err());
    }

    #[test]
    fn settings_json_refuses_the_last_connected_profile() {
        let error = check_settings(
            r#"{ "schema_version": 1, "last_connected_profile": "3f2504e0-4f89-11d3-9a0c-0305e82c3301" }"#,
            Path::new("settings.json"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("last_connected_profile"));
    }

    #[test]
    fn a_missing_settings_json_next_to_split_profiles_starts_from_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        save_file_at(
            ConfigFile::Profiles,
            &paths.profiles,
            &crate::test_helpers::saved_config(),
        )
        .unwrap();

        let config = load(&paths);

        assert_eq!(
            config.settings.geo_routing.auto_update,
            profile::GeoAutoUpdate::Every7d
        );
        assert_eq!(read_json(&paths.settings)["hwid"], config.settings.hwid);
    }

    #[test]
    fn load_config_existing_file_ignores_omarchy() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        let current = state.path().join("omarchy").join("current");
        fs::create_dir_all(&current).unwrap();
        fs::write(current.join("theme.name"), "gruvbox\n").unwrap();

        let mut stored = crate::test_helpers::saved_config();
        stored.settings.theme = "tokyo-night".to_string();
        save_config(&stored).unwrap();

        let loaded = load_config().into_result().unwrap();
        assert_eq!(loaded.settings.theme, "tokyo-night");
    }

    #[test]
    fn load_config_generates_hwid_once_and_persists_it_in_settings_json() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };

        let config = load_config().into_result().unwrap();
        let hwid = config.settings.hwid.clone();
        assert!(hwid.starts_with("lnx-"));
        assert_eq!(hwid.len(), "lnx-".len() + 32);

        let paths = ConfigPaths::current().unwrap();
        assert_eq!(read_json(&paths.settings)["hwid"], hwid);
        assert!(!paths.profiles.exists());

        let reloaded = load_config().into_result().unwrap();
        assert_eq!(reloaded.settings.hwid, hwid);
    }

    #[test]
    fn a_first_run_that_cannot_save_its_hwid_blocks_settings() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();

        let load = load_config_files_at(&dir_paths(&dir));
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();

        let error = format!("{:#}", load.settings_error.unwrap());
        assert!(error.contains("Failed to save"), "got: {error}");
        assert!(load.profiles_error.is_none());
    }

    #[test]
    fn settings_and_subscriptions_without_hwid_fields_are_backward_compatible() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        let legacy = r#"{
            "schema_version": 3,
            "profiles": [],
            "subscriptions": [
                {
                    "id": "3f2504e0-4f89-11d3-9a0c-0305e82c3301",
                    "name": "Regular subscription",
                    "url": "https://example.com/regular"
                }
            ],
            "settings": {
                "tun_interface": "tun0",
                "dns_strategy": "prefer_ipv4",
                "theme": "tokyo-night",
                "log_level": "info"
            }
        }"#;
        fs::write(&paths.profiles, legacy).unwrap();

        let config = load_config_read_only_at(&paths).into_result().unwrap();
        assert!(config.settings.hwid.starts_with("lnx-"));
        assert!(!config.subscriptions[0].send_hwid);
        assert_eq!(config.subscriptions[0].hwid, None);
    }

    #[test]
    fn subscription_send_hwid_fields_roundtrip_through_json() {
        let json = r#"{
            "id": "3f2504e0-4f89-11d3-9a0c-0305e82c3301",
            "name": "HWID subscription",
            "url": "https://example.com/hwid",
            "send_hwid": true,
            "hwid": "provider-registered-device-id"
        }"#;
        let sub: profile::Subscription = serde_json::from_str(json).unwrap();
        assert!(sub.send_hwid);
        assert_eq!(sub.hwid.as_deref(), Some("provider-registered-device-id"));

        let serialized = serde_json::to_string(&sub).unwrap();
        let back: profile::Subscription = serde_json::from_str(&serialized).unwrap();
        assert_eq!(back, sub);
    }

    #[test]
    fn recovery_loads_read_both_parts_of_an_old_profiles_json_without_rewriting_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.json");
        let original = combined_v8("nord");
        fs::write(&path, &original).unwrap();

        assert_eq!(load_profiles_for_recovery(&path).unwrap().profiles.len(), 1);
        assert_eq!(
            load_settings_for_recovery(&path).unwrap().settings.theme,
            "nord"
        );
        assert_eq!(fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn an_old_profiles_json_is_split_settings_first_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        let original = combined_v8("nord");
        fs::write(&paths.profiles, &original).unwrap();

        let loaded = load(&paths);

        assert_eq!(loaded.settings.theme, "nord");
        assert_eq!(loaded.profiles.len(), 1);
        let profiles = read_json(&paths.profiles);
        assert_eq!(profiles["schema_version"], CURRENT_SCHEMA_VERSION);
        assert!(profiles.get("settings").is_none());
        assert_eq!(read_json(&paths.settings)["theme"], "nord");
        let backups = recovery_files(&paths, "profiles.json.before-migration-");
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(fs::read_to_string(&backups[0]).unwrap(), original);
    }

    #[test]
    fn an_interrupted_split_runs_again_from_the_old_profiles_json() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        fs::write(&paths.profiles, combined_v8("nord")).unwrap();
        let mut stale = crate::test_helpers::saved_config();
        stale.settings.theme = "gruvbox".into();
        save_file_at(ConfigFile::Settings, &paths.settings, &stale).unwrap();
        let replaced = fs::read_to_string(&paths.settings).unwrap();

        assert_eq!(load(&paths).settings.theme, "nord");

        assert_eq!(read_json(&paths.settings)["theme"], "nord");
        let preserved = recovery_files(&paths, "settings.json.invalid-");
        assert_eq!(preserved.len(), 1, "{preserved:?}");
        assert_eq!(fs::read_to_string(&preserved[0]).unwrap(), replaced);
    }

    #[test]
    fn a_repeated_split_keeps_no_copy_of_an_identical_settings_json() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        fs::write(&paths.profiles, combined_v8("nord")).unwrap();
        load(&paths);
        fs::write(&paths.profiles, combined_v8("nord")).unwrap();

        load(&paths);

        assert!(recovery_files(&paths, "settings.json.invalid-").is_empty());
    }

    #[test]
    fn normal_load_leaves_current_files_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        let mut config = crate::test_helpers::saved_config();
        config.settings.hwid = "lnx-d415f1264a264924aa76e7f55380b0c7".into();
        save_config_files_at(&paths, &config).unwrap();
        let written = [&paths.profiles, &paths.settings].map(|path| fs::read(path).unwrap());

        load(&paths);

        assert_eq!(
            [&paths.profiles, &paths.settings].map(|path| fs::read(path).unwrap()),
            written
        );
    }

    #[test]
    fn normal_load_refuses_schemas_newer_than_this_build() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        let profiles = format!(
            r#"{{"schema_version":{},"profiles":[]}}"#,
            CURRENT_SCHEMA_VERSION + 1
        );
        fs::write(&paths.profiles, &profiles).unwrap();
        let settings = format!(r#"{{"schema_version":{}}}"#, SETTINGS_SCHEMA_VERSION + 1);
        fs::write(&paths.settings, &settings).unwrap();

        let load = load_config_files_at(&paths);

        for error in [load.profiles_error.unwrap(), load.settings_error.unwrap()] {
            assert!(format!("{error:#}").contains("newer than this build supports"));
        }
        assert_eq!(fs::read_to_string(&paths.profiles).unwrap(), profiles);
        assert_eq!(fs::read_to_string(&paths.settings).unwrap(), settings);
    }

    #[test]
    fn read_only_load_splits_in_memory_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        let original =
            r#"{"schema_version":4,"profiles":[],"settings":{"tun_interface":" kvn0 "}}"#;
        fs::write(&paths.profiles, original).unwrap();

        let loaded = load_config_read_only_at(&paths).into_result().unwrap();

        assert_eq!(loaded.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.settings.tun_interface, "kvn0");
        assert_eq!(fs::read_to_string(&paths.profiles).unwrap(), original);
        assert!(!paths.settings.exists());
    }

    fn with_steam_direct() -> Config {
        let mut config = crate::test_helpers::saved_config();
        config.settings.geo_routing.legacy_service_routes =
            Some(serde_json::json!({ "steam": "direct" }));
        config
    }

    fn written_routing(path: &Path, config: &routing::RoutingConfig) {
        fs::write(path, serde_json::to_string_pretty(config).unwrap()).unwrap();
    }

    #[test]
    fn service_routes_move_into_routing_json_before_settings_json_drops_them() {
        let dir = tempfile::tempdir().unwrap();
        let paths = dir_paths(&dir);
        let routing_path = dir.path().join("routing.json");
        let mut config = with_steam_direct();
        config.settings.geo_routing.set_mode("bypass_ru");
        let mut routing = routing::RoutingConfig::for_new_file(&config.settings.geo_routing);
        written_routing(&routing_path, &routing);

        assert!(
            move_service_routes_at(&paths.settings, &routing_path, &mut config, &mut routing)
                .unwrap()
        );

        let on_disk = routing::load_routing_at(&routing_path, &Default::default(), false).unwrap();
        assert_eq!(on_disk, routing);
        assert_eq!(
            routing.mode("bypass_ru").unwrap().rules[0].matches,
            routing::RuleMatch::RuleSet("geosite-steam".into())
        );
        assert_eq!(config.settings.geo_routing.legacy_service_routes, None);
        let saved = load_config_read_only_at(&paths).into_result().unwrap();
        assert_eq!(saved.settings.geo_routing.legacy_service_routes, None);
        assert!(!paths.profiles.exists());
        assert!(
            !move_service_routes_at(&paths.settings, &routing_path, &mut config, &mut routing)
                .unwrap()
        );
    }

    #[test]
    fn a_move_interrupted_before_settings_json_is_saved_repeats_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let routing_path = dir.path().join("routing.json");
        let mut routing =
            routing::RoutingConfig::for_new_file(&with_steam_direct().settings.geo_routing);
        routing.adopt_service_routes(&serde_json::json!({ "steam": "direct" }));
        written_routing(&routing_path, &routing);
        let adopted = routing.clone();

        move_service_routes_at(
            &settings_path,
            &routing_path,
            &mut with_steam_direct(),
            &mut routing,
        )
        .unwrap();

        assert_eq!(routing, adopted);
    }

    #[test]
    fn an_invalid_routing_json_keeps_the_overrides_in_settings_json() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let routing_path = dir.path().join("routing.json");
        fs::write(&routing_path, "{ not json").unwrap();
        let mut config = with_steam_direct();
        let mut routing = routing::RoutingConfig::default();

        assert!(
            move_service_routes_at(&settings_path, &routing_path, &mut config, &mut routing)
                .is_err()
        );

        assert!(config.settings.geo_routing.legacy_service_routes.is_some());
        assert!(!settings_path.exists());
        assert_eq!(routing, routing::RoutingConfig::default());
    }
}
