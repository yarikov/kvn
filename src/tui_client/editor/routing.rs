use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

use super::{
    EditIssue, EditSession, EditorOutcome, EditorSnapshot, diagnostics_issue, edit_until_saved,
    finish_edit, load_issue, prepared_editor, unresolved_conflicts, write_editor_schema,
};
use crate::app::msg::{IpcCommand, RoutingEditResult};
use crate::config::json_pointer::JsonIndex;
use crate::config::routing::{RoutingConfig, RoutingEditBase, RoutingProblem, routing_json_schema};

struct RoutingEdit {
    base: RoutingEditBase,
    json_schema: Option<String>,
}

impl EditSession for RoutingEdit {
    type Document = RoutingConfig;

    fn check(&self, contents: &str, path: &Path) -> Result<RoutingConfig, EditIssue> {
        check_routing_edit(contents, path)
    }

    fn submit(&self, edited: &RoutingConfig) -> Result<RoutingEditResult> {
        let mut client = crate::ipc::IpcClient::connect()?;
        let request = client.send_request(&IpcCommand::ApplyEditedRouting {
            base: self.base.clone(),
            edited: Box::new(edited.clone()),
        })?;
        let snapshot = client
            .read_response(request, Duration::from_secs(10))
            .context(
                "Could not confirm whether routing.json was saved; check it before retrying",
            )?;
        snapshot.routing_edit_result.context(
            "The daemon did not return an editor save result; restart kvn before editing again",
        )
    }

    fn rebase(&mut self, current: RoutingConfig, edited: &RoutingConfig) -> String {
        let with_schema = |routing: &RoutingConfig| RoutingConfig {
            json_schema: self.json_schema.clone(),
            ..routing.clone()
        };
        let base = match &self.base {
            RoutingEditBase::Parsed(base) => with_schema(base),
            RoutingEditBase::Raw(_) => with_schema(&current),
        };
        let document = crate::config::merge::resolution::document(
            &base,
            &with_schema(&current),
            &with_schema(edited),
        );
        self.base = RoutingEditBase::Parsed(Box::new(current));
        document
    }
}

pub fn open_routing_editor(mode: &str) -> Result<EditorOutcome> {
    let editor = prepared_editor()?;
    let routing_path = crate::paths::routing_path().context("Failed to determine routing path")?;
    let on_disk = fs::read_to_string(&routing_path)
        .with_context(|| format!("Failed to read {}", routing_path.display()))?;
    let json_schema = write_editor_schema("routing.schema.json", &routing_json_schema());
    let (base, contents, line) = match check_routing_edit(&on_disk, &routing_path) {
        Ok(routing) => {
            let contents = snapshot_contents(&routing, json_schema.clone())?;
            let line = routing
                .modes
                .iter()
                .position(|candidate| candidate.id == mode)
                .and_then(|index| JsonIndex::new(&contents).line(&format!("/modes/{index}")));
            (RoutingEditBase::Parsed(Box::new(routing)), contents, line)
        }
        Err(issue) => (RoutingEditBase::Raw(on_disk.clone()), on_disk, issue.line),
    };
    let path = crate::paths::ensure_kvn_runtime_dir()?
        .join(format!("routing-edit-{}.json", std::process::id()));
    crate::atomic_write::write(&path, contents.as_bytes())
        .context("Failed to create editor snapshot")?;
    let snapshot = EditorSnapshot {
        path: path.clone(),
        remove_on_drop: true,
        recovery_kind: crate::config::recovery::RecoveryKind::RoutingInvalidEdit,
    };
    let mut session = RoutingEdit { base, json_schema };
    let result = edit_until_saved(&path, &mut session, &editor, line, contents.as_bytes());
    finish_edit(snapshot, contents.as_bytes(), result)
}

fn snapshot_contents(routing: &RoutingConfig, json_schema: Option<String>) -> Result<String> {
    let snapshot = RoutingConfig {
        json_schema,
        ..routing.clone()
    };
    Ok(serde_json::to_string_pretty(&snapshot)? + "\n")
}

fn check_routing_edit(contents: &str, path: &Path) -> Result<RoutingConfig, EditIssue> {
    unresolved_conflicts(contents)?;
    crate::config::routing::check_routing(contents).map_err(|problem| match problem {
        RoutingProblem::Invalid(diagnostics) => diagnostics_issue(contents, diagnostics),
        RoutingProblem::NewerSchema(_) => EditIssue {
            line: JsonIndex::new(contents).line("/schema_version"),
            message: format!("{:#}", problem.into_error(path)),
        },
        RoutingProblem::Json(_) | RoutingProblem::Shape(_) => load_issue(problem.into_error(path)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::routing::ROUTING_SCHEMA_VERSION;

    fn pretty(routing: &RoutingConfig) -> String {
        serde_json::to_string_pretty(routing).unwrap()
    }

    #[test]
    fn a_routing_edit_lists_every_problem_with_its_line() {
        let mut routing = RoutingConfig::default();
        routing.rule_sets.remove("geoip-ru");
        routing.modes[0].final_outbound = crate::config::routing::Outbound::Direct;
        let contents = pretty(&routing);
        let lines: Vec<_> = contents.lines().collect();
        let final_line = lines
            .iter()
            .position(|line| line.contains("\"final\": \"direct\""))
            .unwrap()
            + 1;

        let issue = check_routing_edit(&contents, Path::new("routing.json")).unwrap_err();

        assert_eq!(issue.line, Some(final_line));
        assert!(issue.message.contains("3 problems"), "{}", issue.message);
        assert!(issue.message.contains("geoip-ru"), "{}", issue.message);
    }

    #[test]
    fn a_routing_edit_reports_broken_json_and_a_newer_schema_at_their_lines() {
        let path = Path::new("routing.json");
        assert_eq!(
            check_routing_edit("{\n  broken\n}", path).unwrap_err().line,
            Some(2)
        );

        let mut document = serde_json::to_value(RoutingConfig::default()).unwrap();
        document["schema_version"] = (ROUTING_SCHEMA_VERSION + 1).into();
        let contents = serde_json::to_string_pretty(&document).unwrap();
        let issue = check_routing_edit(&contents, path).unwrap_err();
        assert!(issue.message.contains("newer than this build supports"));
        assert_eq!(
            issue.line,
            JsonIndex::new(&contents).line("/schema_version")
        );
    }

    #[test]
    fn a_valid_routing_edit_keeps_its_schema_reference_for_the_daemon() {
        let routing = RoutingConfig {
            json_schema: Some("file:///run/user/1000/kvn/routing.schema.json".into()),
            ..RoutingConfig::default()
        };

        assert_eq!(
            check_routing_edit(&pretty(&routing), Path::new("routing.json")).unwrap(),
            routing
        );
    }

    #[test]
    fn a_conflict_document_rebases_onto_the_current_file() {
        let base = RoutingConfig::default();
        let rename = |name: &str| {
            let mut routing = base.clone();
            routing.modes[0].name = name.into();
            routing
        };
        let mut session = RoutingEdit {
            base: RoutingEditBase::Parsed(Box::new(base.clone())),
            json_schema: Some("file:///schema.json".into()),
        };

        let document = session.rebase(rename("Current"), &rename("Edited"));

        assert!(document.contains("<<<<<<< YOUR EDIT"), "{document}");
        assert!(document.contains("\"$schema\": \"file:///schema.json\""));
        assert_eq!(
            session.base,
            RoutingEditBase::Parsed(Box::new(rename("Current")))
        );
    }
}
