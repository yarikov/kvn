use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::app::model::SourceRow;
use crate::app::msg::{ConfigEditResult, IpcCommand};
use crate::config::json_pointer::JsonIndex;
use crate::config::profile::{
    CURRENT_SCHEMA_VERSION, Config, ConfigDiagnostic, config_json_schema, schema_diagnostics,
};

mod retry;
mod routing;

use retry::retry_edit;
pub use routing::{open_routing_editor, resolve_routing_rules_conflict, submit_routing_rules};

const OMARCHY_LAUNCHER: &str = "omarchy-launch-editor";
const OMARCHY_FALLBACK_EDITOR: &str = "nvim";
const OMARCHY_BLOCKING_EDITORS: [&str; 8] = [
    "nvim", "vim", "nano", "micro", "hx", "helix", "fresh", "emacs",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorTarget {
    Profile(usize),
    Subscription(usize),
}

impl From<SourceRow> for EditorTarget {
    fn from(row: SourceRow) -> Self {
        match row {
            SourceRow::StandaloneProfile(idx)
            | SourceRow::SubscriptionProfile {
                profile_idx: idx, ..
            } => Self::Profile(idx),
            SourceRow::SubscriptionHeader(idx) => Self::Subscription(idx),
        }
    }
}

fn detect_editor() -> String {
    env::var("VISUAL")
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| {
            ["nvim", "vim", "vi", "nano"]
                .into_iter()
                .find(|candidate| command_exists(candidate))
                .unwrap_or("vi")
                .to_string()
        })
}

fn command_exists(command: &str) -> bool {
    Command::new("sh")
        .args(["-c", "command -v \"$1\"", "sh", command])
        .output()
        .is_ok_and(|output| output.status.success())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditorCommand {
    program: String,
    args: Vec<String>,
}

impl EditorCommand {
    fn parse(editor: &str) -> Self {
        let mut parts = editor.split_whitespace().map(String::from);
        let program = parts.next().unwrap_or_else(|| "vi".to_string());
        Self {
            program,
            args: parts.collect(),
        }
    }

    fn bypass_omarchy_launcher(self) -> Result<Self> {
        if program_name(&self.program) != OMARCHY_LAUNCHER {
            return Ok(self);
        }
        Self::omarchy_default(crate::omarchy::default_editor(), command_exists)
    }

    fn omarchy_default(
        configured: Option<String>,
        is_installed: impl Fn(&str) -> bool,
    ) -> Result<Self> {
        let program = configured
            .filter(|editor| is_installed(editor))
            .unwrap_or_else(|| OMARCHY_FALLBACK_EDITOR.to_string());
        if matches!(line_jump(&program), LineJump::PlusArgument)
            && !OMARCHY_BLOCKING_EDITORS.contains(&program_name(&program))
        {
            anyhow::bail!(
                "Omarchy opens {} detached, so kvn cannot tell when editing ends; \
                 set $VISUAL to an editor command that waits until the file is closed",
                program_name(&program)
            );
        }
        Ok(Self {
            program,
            args: vec![],
        })
    }

    fn wait_until_closed(mut self) -> Self {
        let detaches = !matches!(line_jump(&self.program), LineJump::PlusArgument);
        if detaches && !self.args.iter().any(|arg| arg == "--wait" || arg == "-w") {
            self.args.push("--wait".to_string());
        }
        self
    }
}

enum LineJump {
    GotoFlag,
    PathSuffix,
    PlusArgument,
}

fn line_jump(program: &str) -> LineJump {
    match program_name(program) {
        "code" | "code-oss" | "codium" | "cursor" => LineJump::GotoFlag,
        "zeditor" | "zed" | "sublime_text" | "subl" => LineJump::PathSuffix,
        _ => LineJump::PlusArgument,
    }
}

fn program_name(program: &str) -> &str {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
}

fn find_target_line(path: &Path, target: EditorTarget) -> Option<usize> {
    let content = fs::read_to_string(path).ok()?;
    let pointer = match target {
        EditorTarget::Profile(idx) => format!("/profiles/{idx}"),
        EditorTarget::Subscription(idx) => format!("/subscriptions/{idx}"),
    };
    JsonIndex::new(&content).line(&pointer)
}

fn editor_args(editor: &str, path: &Path, line: usize) -> Vec<String> {
    let path_with_line = format!("{}:{}", path.display(), line);
    match line_jump(editor) {
        LineJump::GotoFlag => vec!["--goto".to_string(), path_with_line],
        LineJump::PathSuffix => vec![path_with_line],
        LineJump::PlusArgument => vec![format!("+{}", line), path.display().to_string()],
    }
}

struct EditorSnapshot {
    path: PathBuf,
    remove_on_drop: bool,
    recovery_kind: crate::config::recovery::RecoveryKind,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum EditorOutcome {
    Saved,
    Unchanged,
    Cancelled { recovery: Option<PathBuf> },
}

impl EditorSnapshot {
    fn retain(mut self) -> PathBuf {
        self.remove_on_drop = false;
        self.path.clone()
    }
}

impl Drop for EditorSnapshot {
    fn drop(&mut self) {
        if self.remove_on_drop
            && let Err(error) = fs::remove_file(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "Failed to remove editor snapshot {:?}: {}",
                self.path,
                error
            );
        }
    }
}

fn preserve_invalid_edit(snapshot: EditorSnapshot) -> Result<PathBuf> {
    let result = fs::read(&snapshot.path)
        .with_context(|| {
            format!(
                "Failed to read invalid editor snapshot {}",
                snapshot.path.display()
            )
        })
        .and_then(|contents| crate::config::recovery::preserve(snapshot.recovery_kind, &contents));
    match result {
        Ok(path) => Ok(path),
        Err(error) => {
            let retained = snapshot.retain();
            Err(error).with_context(|| {
                format!(
                    "Failed to preserve invalid edit; editor snapshot retained at {}",
                    retained.display()
                )
            })
        }
    }
}

fn prepared_editor() -> Result<EditorCommand> {
    Ok(EditorCommand::parse(&detect_editor())
        .bypass_omarchy_launcher()?
        .wait_until_closed())
}

pub fn open_profiles_editor(target: Option<EditorTarget>, base: Config) -> Result<EditorOutcome> {
    let editor = prepared_editor()?;
    let runtime_dir = crate::paths::ensure_runtime_dir()?;
    let path = runtime_dir.join(format!("profiles-edit-{}.json", std::process::id()));
    let json_schema = write_editor_schema("profiles.schema.json", &config_json_schema());
    crate::config::save_editor_snapshot_at(&path, &base, json_schema.clone())
        .context("Failed to create editor snapshot")?;
    let snapshot = EditorSnapshot {
        path: path.clone(),
        remove_on_drop: true,
        recovery_kind: crate::config::recovery::RecoveryKind::InvalidEdit,
    };
    let mut original = fs::read(&path).context("Failed to read original editor snapshot")?;
    original.push(b'\n');
    crate::atomic_write::write(&path, &original).context("Failed to finish editor snapshot")?;

    let line = target.and_then(|target| find_target_line(&path, target));
    let mut session = ProfilesEdit { base, json_schema };
    let result = edit_until_saved(&path, &mut session, &editor, line, &original);
    finish_edit(snapshot, &original, result)
}

fn write_editor_schema(file_name: &str, schema: &serde_json::Value) -> Option<String> {
    let written = crate::paths::ensure_kvn_runtime_dir().and_then(|dir| {
        let path = dir.join(file_name);
        crate::atomic_write::write(&path, &serde_json::to_vec_pretty(schema)?)?;
        Ok(path)
    });
    match written {
        Ok(path) => url::Url::from_file_path(&path).ok().map(String::from),
        Err(error) => {
            tracing::warn!("Editing without a JSON schema: {error:#}");
            None
        }
    }
}

fn finish_edit(
    snapshot: EditorSnapshot,
    original: &[u8],
    result: Result<EditorOutcome>,
) -> Result<EditorOutcome> {
    if matches!(result, Ok(EditorOutcome::Saved))
        || fs::read(&snapshot.path).is_ok_and(|contents| contents == original)
    {
        return result;
    }
    let cancelled = match snapshot.recovery_kind {
        crate::config::recovery::RecoveryKind::RoutingInvalidEdit => {
            "Routing configuration edit cancelled"
        }
        _ => "Configuration edit cancelled",
    };
    let recovery = preserve_invalid_edit(snapshot).with_context(|| match &result {
        Err(error) => format!("{error:#}"),
        _ => cancelled.into(),
    })?;
    match result {
        Ok(_) => Ok(EditorOutcome::Cancelled {
            recovery: Some(recovery),
        }),
        Err(error) => {
            Err(error).with_context(|| format!("Edited version saved to {}", recovery.display()))
        }
    }
}

trait EditSession {
    type Document;

    fn check(&self, contents: &str, path: &Path) -> Result<Self::Document, EditIssue>;
    fn submit(&self, edited: &Self::Document) -> Result<ConfigEditResult<Self::Document>>;
    fn rebase(&mut self, current: Self::Document, edited: &Self::Document) -> String;
}

struct ProfilesEdit {
    base: Config,
    json_schema: Option<String>,
}

impl EditSession for ProfilesEdit {
    type Document = Config;

    fn check(&self, contents: &str, path: &Path) -> Result<Config, EditIssue> {
        check_edit(contents, path)
    }

    fn submit(&self, edited: &Config) -> Result<ConfigEditResult> {
        submit_edit(&self.base, edited)
    }

    fn rebase(&mut self, current: Config, edited: &Config) -> String {
        let document = conflict_document(&self.base, &current, edited, self.json_schema.as_deref());
        self.base = current;
        document
    }
}

fn edit_until_saved<S: EditSession>(
    path: &Path,
    session: &mut S,
    editor: &EditorCommand,
    mut line: Option<usize>,
    opened: &[u8],
) -> Result<EditorOutcome> {
    loop {
        let args = match line {
            Some(line) => editor_args(&editor.program, path, line),
            None => vec![path.display().to_string()],
        };
        let status = Command::new(&editor.program)
            .args(&editor.args)
            .args(args)
            .status()
            .with_context(|| format!("Failed to launch editor: {}", editor.program))?;
        anyhow::ensure!(status.success(), "Editor exited with non-zero status");
        let contents = fs::read_to_string(path).context("Failed to read editor snapshot")?;
        if contents.as_bytes() == opened {
            return Ok(EditorOutcome::Unchanged);
        }
        let edited = match session.check(&contents, path) {
            Ok(edited) => edited,
            Err(issue) => {
                line = issue.line;
                if retry_edit(&issue.message)? {
                    continue;
                }
                return Ok(EditorOutcome::Cancelled { recovery: None });
            }
        };
        match session.submit(&edited)? {
            ConfigEditResult::Saved => return Ok(EditorOutcome::Saved),
            ConfigEditResult::Conflict { current, paths } => {
                let document = session.rebase(*current, &edited);
                crate::atomic_write::write(path, document.as_bytes())?;
                line = crate::config::merge::resolution::first_marker_line(&document);
                if !retry_edit(&conflict_message(&paths))? {
                    return Ok(EditorOutcome::Cancelled { recovery: None });
                }
            }
            ConfigEditResult::Failed { message } => anyhow::bail!("{message}"),
        }
    }
}

fn conflict_message(paths: &[String]) -> String {
    format!(
        "Conflicts at {}. Keep the desired values and remove the conflict markers.",
        paths.join(", ")
    )
}

fn conflict_document(
    base: &Config,
    current: &Config,
    edited: &Config,
    json_schema: Option<&str>,
) -> String {
    let with_schema = |config: &Config| Config {
        json_schema: json_schema.map(String::from),
        ..config.clone()
    };
    crate::config::merge::resolution::document(
        &with_schema(base),
        &with_schema(current),
        &with_schema(edited),
    )
}

#[derive(Debug)]
struct EditIssue {
    message: String,
    line: Option<usize>,
}

fn unresolved_conflicts(contents: &str) -> Result<(), EditIssue> {
    match crate::config::merge::resolution::first_marker_line(contents) {
        Some(line) => Err(EditIssue {
            message:
                "Unresolved conflicts: keep the desired values and remove all conflict markers."
                    .into(),
            line: Some(line),
        }),
        None => Ok(()),
    }
}

fn add_unreported(
    contents: &str,
    mut diagnostics: Vec<ConfigDiagnostic>,
    more: Vec<ConfigDiagnostic>,
) -> Result<(), EditIssue> {
    let reported: HashSet<String> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.pointer.clone())
        .collect();
    diagnostics.extend(
        more.into_iter()
            .filter(|diagnostic| !reported.contains(&diagnostic.pointer)),
    );
    if diagnostics.is_empty() {
        return Ok(());
    }
    Err(diagnostics_issue(contents, diagnostics))
}

fn check_edit(contents: &str, path: &Path) -> Result<Config, EditIssue> {
    unresolved_conflicts(contents)?;
    let document = serde_json::from_str::<serde_json::Value>(contents).ok();
    let diagnostics = document
        .as_ref()
        .map(schema_diagnostics)
        .unwrap_or_default();
    if document.is_some_and(|document| !has_current_schema_version(&document)) {
        return Err(diagnostics_issue(contents, diagnostics));
    }
    let mut config = match crate::config::load_config_bytes_read_only(contents.as_bytes(), path) {
        Ok(config) => config,
        Err(error) if diagnostics.is_empty() => return Err(load_issue(error)),
        Err(_) => return Err(diagnostics_issue(contents, diagnostics)),
    };
    config.json_schema = None;
    let more = config.diagnostics();
    add_unreported(contents, diagnostics, more)?;
    Ok(config)
}

fn has_current_schema_version(document: &serde_json::Value) -> bool {
    document.get("schema_version") == Some(&CURRENT_SCHEMA_VERSION.into())
}

fn load_issue(error: anyhow::Error) -> EditIssue {
    EditIssue {
        line: error.chain().find_map(|cause| {
            cause
                .downcast_ref::<serde_json::Error>()
                .map(|error| error.line().max(1))
        }),
        message: format!("{error:#}"),
    }
}

fn diagnostics_issue(contents: &str, diagnostics: Vec<ConfigDiagnostic>) -> EditIssue {
    let index = JsonIndex::new(contents);
    let mut located: Vec<_> = diagnostics
        .into_iter()
        .map(|diagnostic| (index.nearest_line(&diagnostic.pointer), diagnostic.message))
        .collect();
    located.sort_by_key(|(line, _)| line.unwrap_or(usize::MAX));
    let count = match located.len() {
        1 => "1 problem".to_string(),
        count => format!("{count} problems"),
    };
    let problems: Vec<_> = located
        .iter()
        .map(|(line, message)| match line {
            Some(line) => format!("• line {line}: {message}"),
            None => format!("• {message}"),
        })
        .collect();
    EditIssue {
        line: located.first().and_then(|(line, _)| *line),
        message: format!("The configuration has {count}:\n\n{}", problems.join("\n")),
    }
}

fn submit_edit(base: &Config, edited: &Config) -> Result<ConfigEditResult> {
    let mut client = crate::ipc::IpcClient::connect()?;
    let request = client.send_request(&IpcCommand::ApplyEditedConfig {
        base: Box::new(base.clone()),
        edited: Box::new(edited.clone()),
    })?;
    let snapshot = client.read_response(request, Duration::from_secs(10))
        .context("Could not confirm whether the configuration was saved; check the current configuration before retrying")?;
    snapshot.config_edit_result.context(
        "The daemon did not return an editor save result; restart kvn before editing again",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::Settings;
    use tempfile::TempDir;

    #[test]
    fn invalid_json_reports_its_line_and_accepts_correction() {
        let path = Path::new("snapshot.json");
        let issue = check_edit("{\n invalid\n}", path).unwrap_err();
        assert_eq!(issue.line, Some(2));
        assert!(issue.message.contains("line 2"));
        let fixed = serde_json::to_string(&Config::default()).unwrap();
        assert!(check_edit(&fixed, path).is_ok());
    }

    #[test]
    fn validation_lists_every_problem_and_points_at_the_first() {
        let mut config = Config::default();
        let profile = crate::config::profile::Profile::new_vless(
            "A".into(),
            "example.com".into(),
            443,
            uuid::Uuid::new_v4().to_string(),
        );
        config.profiles = vec![profile.clone(), profile];
        config.settings.theme = "missing-theme".into();
        config.profiles[0].port = 0;
        let path = Path::new("snapshot.json");
        let contents = serde_json::to_string_pretty(&config).unwrap();
        let lines: Vec<_> = contents.lines().collect();
        let issue = check_edit(&contents, path).unwrap_err();
        let line_of = |key: &str| lines.iter().position(|line| line.contains(key)).unwrap() + 1;
        let port_line = line_of("\"port\": 0");
        let theme_line = line_of("\"theme\"");
        let duplicate_line = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.contains("\"id\""))
            .nth(1)
            .unwrap()
            .0
            + 1;
        assert_eq!(issue.line, Some(port_line));
        let theme_problem = schema_diagnostics(&serde_json::from_str(&contents).unwrap())
            .into_iter()
            .find(|diagnostic| diagnostic.pointer == "/settings/theme")
            .unwrap()
            .message;
        assert_eq!(
            issue.message,
            format!(
                "The configuration has 3 problems:\n\n\
                 • line {port_line}: /profiles/0/port: value is less than the minimum of 1\n\
                 • line {duplicate_line}: Profile 2: duplicate id {}\n\
                 • line {theme_line}: {theme_problem}",
                config.profiles[0].id,
            )
        );
        config.profiles.pop();
        config.settings.theme = Settings::default().theme;
        config.profiles[0].port = 443;
        assert!(check_edit(&serde_json::to_string(&config).unwrap(), path).is_ok());
    }

    #[test]
    fn parse_failures_list_every_structural_problem() {
        let contents = r#"{
  "$schema": "file:///run/user/1000/kvn/profiles.schema.json",
  "schema_version": CURRENT,
  "profiles": [
    {
      "protocol": "trojan",
      "address": "a.example",
      "port": "443",
      "password": "p"
    }
  ],
  "settings": { "theme": 7 }
}"#
        .replace("CURRENT", &CURRENT_SCHEMA_VERSION.to_string());
        let issue = check_edit(&contents, Path::new("snapshot.json")).unwrap_err();
        assert_eq!(issue.line, Some(5));
        assert_eq!(
            issue.message,
            "The configuration has 3 problems:\n\n\
             • line 5: /profiles/0: \"name\" is a required property\n\
             • line 8: /profiles/0/port: value is not of type \"integer\"\n\
             • line 12: /settings/theme: value is not of type \"string\""
        );
    }

    #[test]
    fn removing_schema_version_is_rejected_instead_of_rerunning_migrations() {
        let contents = serde_json::to_string_pretty(&Config::default()).unwrap();
        let removed = contents.replacen(
            &format!("  \"schema_version\": {CURRENT_SCHEMA_VERSION},\n"),
            "",
            1,
        );
        assert_ne!(removed, contents);
        let issue = check_edit(&removed, Path::new("snapshot.json")).unwrap_err();
        assert_eq!(issue.line, Some(1));
        assert!(
            issue
                .message
                .contains("\"schema_version\" is a required property")
        );
    }

    #[test]
    fn conflict_document_keeps_the_schema_reference() {
        let base = Config::default();
        let mut current = base.clone();
        current.settings.theme = "nord".into();
        let mut edited = base.clone();
        edited.settings.theme = "catppuccin".into();
        let schema = "file:///run/user/1000/kvn/profiles.schema.json";
        let document = conflict_document(&base, &current, &edited, Some(schema));
        assert!(
            document.starts_with(&format!("{{\n  \"$schema\": \"{schema}\",")),
            "{document}"
        );
        assert!(!conflict_document(&base, &current, &edited, None).contains("$schema"));
    }

    #[test]
    fn schema_reference_is_accepted_and_not_submitted() {
        let mut config = Config {
            json_schema: Some("file:///run/user/1000/kvn/profiles.schema.json".into()),
            ..Config::default()
        };
        let contents = serde_json::to_string_pretty(&config).unwrap();
        config.json_schema = None;
        assert_eq!(
            check_edit(&contents, Path::new("snapshot.json")).unwrap(),
            config
        );
    }

    #[test]
    fn unresolved_conflicts_explain_how_to_continue() {
        let issue = check_edit("{\n<<<<<<< YOUR EDIT\n", Path::new("snapshot.json")).unwrap_err();
        assert_eq!(issue.line, Some(2));
        assert!(issue.message.contains("remove all conflict markers"));
    }

    #[test]
    fn saving_without_net_changes_before_editor_failure_does_not_create_recovery() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", dir.path());
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let script = dir.path().join("editor.sh");
        let _visual = crate::test_helpers::EnvVarGuard::set(
            "VISUAL",
            format!("/bin/sh {}", script.display()),
        );
        let recovery = crate::paths::profiles_path()
            .unwrap()
            .parent()
            .unwrap()
            .join("recovery");
        for interim_edit in [":", "printf 'BROKEN\\n' > \"$1\""] {
            fs::write(&script, format!(
                "contents=$(cat -- \"$1\")\n{interim_edit}\nprintf '%s\\n' \"$contents\" > \"$1\"\nexit 1\n"
            )).unwrap();
            let error = open_profiles_editor(None, Config::default()).unwrap_err();
            assert!(format!("{error:#}").contains("non-zero status"));
            assert!(!recovery.exists());
            assert!(!format!("{error:#}").contains("Edited version saved"));
        }
    }

    #[test]
    fn closing_the_editor_without_changes_saves_nothing() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", dir.path());
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let _visual = crate::test_helpers::EnvVarGuard::set("VISUAL", "true");

        assert_eq!(
            open_profiles_editor(None, Config::default()).unwrap(),
            EditorOutcome::Unchanged
        );
    }

    #[test]
    fn editor_launch_failure_does_not_create_recovery() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", dir.path());
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let _visual =
            crate::test_helpers::EnvVarGuard::set("VISUAL", dir.path().join("missing-editor"));
        let error = open_profiles_editor(None, Config::default()).unwrap_err();
        assert!(format!("{error:#}").contains("Failed to launch editor"));
        assert!(!dir.path().join("kvn-tui/recovery").exists());
    }

    #[test]
    fn cancelling_an_unchanged_snapshot_needs_no_recovery() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("snapshot.json");
        fs::write(&path, "original").unwrap();
        let result = finish_edit(
            EditorSnapshot {
                path: path.clone(),
                remove_on_drop: true,
                recovery_kind: crate::config::recovery::RecoveryKind::InvalidEdit,
            },
            b"original",
            Ok(EditorOutcome::Cancelled { recovery: None }),
        )
        .unwrap();
        assert_eq!(result, EditorOutcome::Cancelled { recovery: None });
        assert!(!path.exists());
    }

    #[test]
    fn cancelling_a_generated_conflict_document_preserves_unsaved_edits() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let path = dir.path().join("snapshot.json");
        let base = Config::default();
        let mut current = base.clone();
        current.settings.theme = "nord".into();
        let mut edited = base.clone();
        edited.settings.theme = "catppuccin".into();
        let document = crate::config::merge::resolution::document(&base, &current, &edited);
        fs::write(&path, &document).unwrap();
        let result = finish_edit(
            EditorSnapshot {
                path,
                remove_on_drop: true,
                recovery_kind: crate::config::recovery::RecoveryKind::InvalidEdit,
            },
            &serde_json::to_vec_pretty(&base).unwrap(),
            Ok(EditorOutcome::Cancelled { recovery: None }),
        )
        .unwrap();
        let EditorOutcome::Cancelled {
            recovery: Some(path),
        } = result
        else {
            panic!("expected cancellation with recovery")
        };
        assert_eq!(fs::read_to_string(path).unwrap(), document);
    }

    #[test]
    fn editor_failure_preserves_changed_contents() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let _config = crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", dir.path());
        let path = dir.path().join("snapshot.json");
        fs::write(&path, "edited").unwrap();
        let error = finish_edit(
            EditorSnapshot {
                path,
                remove_on_drop: true,
                recovery_kind: crate::config::recovery::RecoveryKind::InvalidEdit,
            },
            b"original",
            Err(anyhow::anyhow!("Editor exited with non-zero status")),
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("non-zero status"));
        assert!(message.contains("Edited version saved"));
        let recovery = fs::read_dir(dir.path().join("kvn-tui/recovery"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read_to_string(recovery).unwrap(), "edited");
    }

    #[test]
    fn editor_snapshot_removes_file_on_drop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("snapshot.json");
        fs::write(&path, "snapshot").unwrap();
        drop(EditorSnapshot {
            path: path.clone(),
            remove_on_drop: true,
            recovery_kind: crate::config::recovery::RecoveryKind::InvalidEdit,
        });
        assert!(!path.exists());
    }

    #[test]
    fn retained_editor_snapshot_survives_drop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("snapshot.json");
        fs::write(&path, "snapshot").unwrap();
        let retained = EditorSnapshot {
            path: path.clone(),
            remove_on_drop: true,
            recovery_kind: crate::config::recovery::RecoveryKind::InvalidEdit,
        }
        .retain();
        assert_eq!(retained, path);
        assert!(path.exists());
    }

    #[test]
    fn failed_recovery_write_retains_editor_snapshot() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let invalid_config_home = dir.path().join("not-a-directory");
        fs::write(&invalid_config_home, "block directory creation").unwrap();
        let _config_home =
            crate::test_helpers::EnvVarGuard::set("XDG_CONFIG_HOME", &invalid_config_home);
        let path = dir.path().join("snapshot.json");
        fs::write(&path, "invalid edit").unwrap();

        let error = preserve_invalid_edit(EditorSnapshot {
            path: path.clone(),
            remove_on_drop: true,
            recovery_kind: crate::config::recovery::RecoveryKind::InvalidEdit,
        })
        .unwrap_err();

        assert!(path.exists());
        assert!(error.to_string().contains(path.to_str().unwrap()));
    }

    #[test]
    fn find_profile_line_first_profile() {
        use crate::config::profile::Profile;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("profiles.json");

        let mut config = Config::default();
        config.profiles.push(Profile::new_vless(
            "First".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        ));

        let json = serde_json::to_string_pretty(&config).unwrap();
        fs::write(&path, json).unwrap();

        let line = find_target_line(&path, EditorTarget::Profile(0));
        assert!(line.is_some());

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines[line.unwrap() - 1].trim(), "{");
        assert!(lines[line.unwrap()].contains("id"));
    }

    #[test]
    fn find_profile_line_second_profile() {
        use crate::config::profile::Profile;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("profiles.json");

        let mut config = Config::default();
        config.profiles.push(Profile::new_vless(
            "First".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        ));
        config.profiles.push(Profile::new_vless(
            "Second".to_string(),
            "2.2.2.2".to_string(),
            443,
            "u2".to_string(),
        ));

        let json = serde_json::to_string_pretty(&config).unwrap();
        fs::write(&path, json).unwrap();

        let line0 = find_target_line(&path, EditorTarget::Profile(0)).unwrap();
        let line1 = find_target_line(&path, EditorTarget::Profile(1)).unwrap();
        assert!(line1 > line0);

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines[line1 - 1].trim(), "{");
        assert!(lines[line1].contains("Second") || lines[line1 + 1].contains("Second"));
    }

    #[test]
    fn find_profile_line_out_of_bounds() {
        use crate::config::profile::Profile;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("profiles.json");

        let mut config = Config::default();
        config.profiles.push(Profile::new_vless(
            "Only".to_string(),
            "1.1.1.1".to_string(),
            443,
            "u1".to_string(),
        ));

        let json = serde_json::to_string_pretty(&config).unwrap();
        fs::write(&path, json).unwrap();

        assert_eq!(find_target_line(&path, EditorTarget::Profile(5)), None);
    }

    #[test]
    fn find_profile_line_empty_profiles() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("profiles.json");

        let config = Config::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        fs::write(&path, json).unwrap();

        assert_eq!(find_target_line(&path, EditorTarget::Profile(0)), None);
    }

    #[test]
    fn find_subscription_line_first_and_second_subscription() {
        use crate::config::profile::Subscription;
        use uuid::Uuid;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("profiles.json");
        let config = Config {
            subscriptions: vec![
                Subscription {
                    id: Uuid::new_v4(),
                    name: "First subscription".to_string(),
                    url: "https://example.com/first".to_string(),
                    auto_update: Default::default(),
                    last_updated: None,
                    next_auto_update: None,
                    retry_state: None,
                    send_hwid: false,
                    hwid: None,
                },
                Subscription {
                    id: Uuid::new_v4(),
                    name: "Second subscription".to_string(),
                    url: "https://example.com/second".to_string(),
                    auto_update: Default::default(),
                    last_updated: None,
                    next_auto_update: None,
                    retry_state: None,
                    send_hwid: false,
                    hwid: None,
                },
            ],
            ..Default::default()
        };
        fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let line0 = find_target_line(&path, EditorTarget::Subscription(0)).unwrap();
        let line1 = find_target_line(&path, EditorTarget::Subscription(1)).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = content.lines().collect();

        assert!(line1 > line0);
        assert_eq!(lines[line0 - 1].trim(), "{");
        assert_eq!(lines[line1 - 1].trim(), "{");
        assert!(lines[line0 + 1].contains("First subscription"));
        assert!(lines[line1 + 1].contains("Second subscription"));
    }

    #[test]
    fn editor_target_maps_source_rows() {
        assert_eq!(
            EditorTarget::from(SourceRow::StandaloneProfile(2)),
            EditorTarget::Profile(2)
        );
        assert_eq!(
            EditorTarget::from(SourceRow::SubscriptionProfile {
                sub_idx: 1,
                profile_idx: 4,
            }),
            EditorTarget::Profile(4)
        );
        assert_eq!(
            EditorTarget::from(SourceRow::SubscriptionHeader(3)),
            EditorTarget::Subscription(3)
        );
    }

    fn editor(program: &str, args: &[&str]) -> EditorCommand {
        EditorCommand {
            program: program.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
        }
    }

    #[test]
    fn parse_editor_bare_program() {
        assert_eq!(EditorCommand::parse("nvim"), editor("nvim", &[]));
    }

    #[test]
    fn parse_editor_program_with_flags() {
        assert_eq!(
            EditorCommand::parse("omarchy-launch-editor --inline"),
            editor("omarchy-launch-editor", &["--inline"])
        );
    }

    #[test]
    fn parse_editor_empty_falls_back_to_vi() {
        assert_eq!(EditorCommand::parse(""), editor("vi", &[]));
    }

    #[test]
    fn editor_args_vim_uses_plus_line() {
        let path = Path::new("/tmp/profiles.json");
        let args = editor_args("vim", path, 42);
        assert_eq!(args, vec!["+42".to_string(), "/tmp/profiles.json".into()]);
    }

    #[test]
    fn editor_args_nvim_uses_plus_line() {
        let args = editor_args("nvim", Path::new("/tmp/p.json"), 7);
        assert_eq!(args[0], "+7");
        assert!(args[1].ends_with("p.json"));
    }

    #[test]
    fn editor_args_nano_uses_plus_line() {
        let args = editor_args("nano", Path::new("/x.json"), 3);
        assert_eq!(args[0], "+3");
    }

    #[test]
    fn editor_args_code_uses_goto_flag() {
        let args = editor_args("code", Path::new("/tmp/profiles.json"), 12);
        assert_eq!(
            args,
            vec!["--goto".to_string(), "/tmp/profiles.json:12".into()]
        );
    }

    #[test]
    fn editor_args_code_oss_and_codium_use_goto() {
        let a = editor_args("code-oss", Path::new("/p.json"), 1);
        let b = editor_args("codium", Path::new("/p.json"), 1);
        assert_eq!(a[0], "--goto");
        assert_eq!(b[0], "--goto");
    }

    #[test]
    fn editor_args_cursor_uses_goto_flag() {
        let args = editor_args("cursor", Path::new("/p.json"), 5);
        assert_eq!(args, vec!["--goto".to_string(), "/p.json:5".into()]);
    }

    #[test]
    fn editor_args_zed_and_sublime_append_the_line_to_the_path() {
        for editor in ["zeditor", "zed", "sublime_text", "subl"] {
            let args = editor_args(editor, Path::new("/p.json"), 5);
            assert_eq!(args, vec!["/p.json:5".to_string()], "{editor}");
        }
    }

    #[test]
    fn editor_args_unknown_editor_uses_plus_line() {
        let args = editor_args("helix", Path::new("/x.json"), 9);
        assert_eq!(args[0], "+9");
        assert_eq!(args[1], "/x.json");
    }

    #[test]
    fn editor_args_handles_full_path_to_editor() {
        let args = editor_args("/usr/bin/code", Path::new("/p.json"), 2);
        assert_eq!(args[0], "--goto");
    }

    #[test]
    fn omarchy_default_runs_a_terminal_editor_directly() {
        assert_eq!(
            EditorCommand::omarchy_default(Some("/usr/bin/helix".into()), |_| true).unwrap(),
            editor("/usr/bin/helix", &[])
        );
    }

    #[test]
    fn graphical_editors_wait_until_the_file_is_closed() {
        for program in ["code", "/usr/bin/codium", "cursor", "zeditor", "subl"] {
            assert_eq!(
                EditorCommand::parse(program).wait_until_closed(),
                editor(program, &["--wait"]),
                "{program}"
            );
        }
        for command in ["code --wait", "code -w --new-window"] {
            let parsed = EditorCommand::parse(command);
            assert_eq!(parsed.clone().wait_until_closed(), parsed, "{command}");
        }
        let vim = EditorCommand::parse("vim");
        assert_eq!(vim.clone().wait_until_closed(), vim);
    }

    #[test]
    fn omarchy_default_falls_back_to_nvim_like_the_launcher() {
        let nvim = editor("nvim", &[]);
        assert_eq!(
            EditorCommand::omarchy_default(None, |_| true).unwrap(),
            nvim
        );
        assert_eq!(
            EditorCommand::omarchy_default(Some("micro".into()), |_| false).unwrap(),
            nvim
        );
    }

    #[test]
    fn omarchy_default_rejects_an_editor_it_cannot_wait_for() {
        let error = EditorCommand::omarchy_default(Some("kate".into()), |_| true).unwrap_err();
        assert!(error.to_string().contains("kate"));
    }

    #[test]
    fn non_omarchy_editor_is_not_rewritten() {
        let vim = editor("vim", &["-u", "NONE"]);
        assert_eq!(vim.clone().bypass_omarchy_launcher().unwrap(), vim);
    }

    #[test]
    fn detect_editor_prefers_visual_over_editor() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let prev_visual = env::var("VISUAL").ok();
        let prev_editor = env::var("EDITOR").ok();
        unsafe {
            env::set_var("VISUAL", "my-visual-editor");
            env::set_var("EDITOR", "my-other-editor");
        }
        assert_eq!(detect_editor(), "my-visual-editor");
        unsafe {
            match prev_visual {
                Some(v) => env::set_var("VISUAL", v),
                None => env::remove_var("VISUAL"),
            }
            match prev_editor {
                Some(v) => env::set_var("EDITOR", v),
                None => env::remove_var("EDITOR"),
            }
        }
    }

    #[test]
    fn detect_editor_falls_back_to_command_v_chain_when_no_env_set() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let prev_visual = env::var("VISUAL").ok();
        let prev_editor = env::var("EDITOR").ok();
        unsafe {
            env::remove_var("VISUAL");
            env::remove_var("EDITOR");
        }
        let editor = detect_editor();
        assert!(
            ["nvim", "vim", "vi", "nano"].contains(&editor.as_str()),
            "unexpected fallback editor: {editor}"
        );
        unsafe {
            match prev_visual {
                Some(v) => env::set_var("VISUAL", v),
                None => env::remove_var("VISUAL"),
            }
            match prev_editor {
                Some(v) => env::set_var("EDITOR", v),
                None => env::remove_var("EDITOR"),
            }
        }
    }

    #[test]
    fn detect_editor_falls_back_to_editor_var() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let prev_visual = env::var("VISUAL").ok();
        let prev_editor = env::var("EDITOR").ok();
        unsafe {
            env::remove_var("VISUAL");
            env::set_var("EDITOR", "fallback-editor");
        }
        assert_eq!(detect_editor(), "fallback-editor");
        unsafe {
            match prev_visual {
                Some(v) => env::set_var("VISUAL", v),
                None => env::remove_var("VISUAL"),
            }
            match prev_editor {
                Some(v) => env::set_var("EDITOR", v),
                None => env::remove_var("EDITOR"),
            }
        }
    }
}
