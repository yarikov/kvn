use serde_json::Value;

use crate::config::profile::Config;

pub(crate) fn document(base: &Config, current: &Config, edited: &Config) -> String {
    let base = serde_json::to_value(base).expect("Config serialization cannot fail");
    let current = serde_json::to_value(current).expect("Config serialization cannot fail");
    let edited = serde_json::to_value(edited).expect("Config serialization cannot fail");
    let merge = |prefer_edited| {
        super::merge_value(
            Some(&base),
            Some(&current),
            Some(&edited),
            "",
            &mut Vec::new(),
            prefer_edited,
        )
        .expect("root config cannot be deleted")
    };
    render(&merge(true), &merge(false), 0) + "\n"
}

fn pretty(value: &Value, depth: usize) -> String {
    serde_json::to_string_pretty(value)
        .expect("JSON serialization cannot fail")
        .replace('\n', &format!("\n{}", "  ".repeat(depth)))
}

fn render(edited: &Value, current: &Value, depth: usize) -> String {
    if edited == current {
        return pretty(edited, depth);
    }
    if let (Value::Object(edited), Value::Object(current)) = (edited, current)
        && edited.keys().eq(current.keys())
    {
        let fields = edited
            .iter()
            .map(|(key, value)| {
                format!(
                    "{}{}: {}",
                    "  ".repeat(depth + 1),
                    serde_json::to_string(key).expect("JSON key serialization cannot fail"),
                    render(value, &current[key], depth + 1)
                )
            })
            .collect::<Vec<_>>()
            .join(",\n");
        return format!("{{\n{fields}\n{}}}", "  ".repeat(depth));
    }
    if let (Value::Array(edited), Value::Array(current)) = (edited, current)
        && edited.len() == current.len()
        && edited
            .iter()
            .zip(current)
            .all(|(e, c)| e.get("id").is_some() && e.get("id") == c.get("id"))
    {
        let items = edited
            .iter()
            .zip(current)
            .map(|(e, c)| format!("{}{}", "  ".repeat(depth + 1), render(e, c, depth + 1)))
            .collect::<Vec<_>>()
            .join(",\n");
        return format!("[\n{items}\n{}]", "  ".repeat(depth));
    }
    let indent = "  ".repeat(depth);
    format!(
        "\n<<<<<<< YOUR EDIT\n{indent}{}\n=======\n{indent}{}\n>>>>>>> CURRENT\n{indent}",
        pretty(edited, depth),
        pretty(current, depth)
    )
}

pub(crate) fn first_marker_line(contents: &str) -> Option<usize> {
    contents
        .lines()
        .position(|line| {
            let line = line.trim();
            line.starts_with("<<<<<<<") || line == "=======" || line.starts_with(">>>>>>>")
        })
        .map(|line| line + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::Profile;

    fn choose(document: &str, edited: bool) -> Config {
        let mut include = true;
        let mut lines = Vec::new();
        for line in document.lines() {
            match line.trim() {
                "<<<<<<< YOUR EDIT" => include = edited,
                "=======" => include = !edited,
                ">>>>>>> CURRENT" => include = true,
                _ if include => lines.push(line),
                _ => {}
            }
        }
        serde_json::from_str(&lines.join("\n")).unwrap()
    }

    fn base() -> Config {
        let mut config = Config::default();
        for name in ["A", "B", "C"] {
            config.profiles.push(Profile::new_vless(
                name.into(),
                "example.com".into(),
                443,
                uuid::Uuid::new_v4().to_string(),
            ));
        }
        config
    }

    #[test]
    fn field_conflicts_keep_independent_changes_in_both_choices() {
        let base = base();
        let mut current = base.clone();
        current.profiles[0].name = "Current".into();
        current.profiles[1].port = 8443;
        let mut edited = base.clone();
        edited.profiles[0].name = "Edited".into();
        edited.settings.theme = "nord".into();
        let text = document(&base, &current, &edited);
        assert_eq!(text.matches("<<<<<<<").count(), 1);
        for (side, name) in [(true, "Edited"), (false, "Current")] {
            let resolved = choose(&text, side);
            assert_eq!(resolved.profiles[0].name, name);
            assert_eq!(resolved.profiles[1].port, 8443);
            assert_eq!(resolved.settings.theme, "nord");
        }
    }

    #[test]
    fn deletion_conflict_offers_valid_arrays_and_keeps_other_edits() {
        let base = base();
        let mut current = base.clone();
        current.profiles.remove(0);
        current.profiles[0].port = 8443;
        let mut edited = base.clone();
        edited.profiles[0].name = "Edited".into();
        let text = document(&base, &current, &edited);
        let restored = choose(&text, true);
        assert_eq!(
            restored
                .profiles
                .iter()
                .find(|p| p.id == base.profiles[0].id)
                .unwrap()
                .name,
            "Edited"
        );
        assert_eq!(
            restored
                .profiles
                .iter()
                .find(|p| p.name == "B")
                .unwrap()
                .port,
            8443
        );
        assert_eq!(choose(&text, false).profiles.len(), 2);
        assert_eq!(choose(&text, false).profiles[0].port, 8443);
    }

    #[test]
    fn order_conflict_preserves_each_order_and_independent_changes() {
        let base = base();
        let mut current = base.clone();
        current.profiles.swap(0, 1);
        current.profiles[0].port = 8443;
        let mut edited = base.clone();
        edited.profiles.swap(1, 2);
        let text = document(&base, &current, &edited);
        for (side, expected) in [(true, &edited), (false, &current)] {
            let resolved = choose(&text, side);
            assert_eq!(
                resolved.profiles.iter().map(|p| p.id).collect::<Vec<_>>(),
                expected.profiles.iter().map(|p| p.id).collect::<Vec<_>>()
            );
            assert_eq!(
                resolved
                    .profiles
                    .iter()
                    .find(|p| p.name == "B")
                    .unwrap()
                    .port,
                8443
            );
        }
    }

    #[test]
    fn markers_are_detected_without_matching_json_strings() {
        assert_eq!(first_marker_line("{\n<<<<<<< YOUR EDIT\n"), Some(2));
        assert_eq!(first_marker_line("=======\n"), Some(1));
        assert_eq!(first_marker_line(">>>>>>> CURRENT\n"), Some(1));
        assert_eq!(first_marker_line("{\"name\": \"<<<<<<< YOUR EDIT\"}"), None);
    }

    #[test]
    fn optional_field_conflict_can_choose_deletion() {
        let edited = serde_json::json!({"name": "A"});
        let current = serde_json::json!({"name": "A", "optional": "changed"});
        let text = render(&edited, &current, 1);
        assert!(text.contains("<<<<<<< YOUR EDIT"));
        assert!(text.contains("\"optional\": \"changed\""));
    }

    #[test]
    fn another_change_during_resolution_is_detected_again() {
        let base = Config::default();
        let mut current = base.clone();
        current.settings.theme = "nord".into();
        let mut edited = base.clone();
        edited.settings.theme = "catppuccin".into();
        let resolved = choose(&document(&base, &current, &edited), true);
        let mut latest = current.clone();
        latest.settings.theme = "gruvbox".into();
        let repeated = document(&current, &latest, &resolved);
        assert_eq!(choose(&repeated, true).settings.theme, "catppuccin");
        assert_eq!(choose(&repeated, false).settings.theme, "gruvbox");
    }
}
