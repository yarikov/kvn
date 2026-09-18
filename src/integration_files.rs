use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const KILLSWITCH_HELPER: &str = include_str!("../contrib/killswitch-helper.sh");
pub(crate) const KILLSWITCH_RULESET: &str = include_str!("../contrib/killswitch.nft");
pub(crate) const KILLSWITCH_UNIT: &str = include_str!("../contrib/kvn-tui-killswitch.service");
pub(crate) const KILLSWITCH_SUDOERS: &str = include_str!("../contrib/kvn-tui-killswitch.sudoers");
pub(crate) const POLKIT_RULE: &str = include_str!("../contrib/49-kvn-tui.rules");

pub(crate) const KILLSWITCH_HELPER_PATH: &str = "/usr/lib/kvn-tui/killswitch-helper.sh";
const KILLSWITCH_RULESET_PATH: &str = "/etc/kvn-tui/killswitch.nft";
const KILLSWITCH_UNIT_PATH: &str = "/etc/systemd/system/kvn-tui-killswitch.service";
const KILLSWITCH_SUDOERS_STAMP_PATH: &str = "/var/lib/kvn/integrations/killswitch-sudoers.sha256";
const POLKIT_STAMP_PATH: &str = "/var/lib/kvn/integrations/polkit.sha256";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StampState {
    Current,
    Outdated,
    Missing,
}

pub(crate) fn under_root(root: &Path, path: &str) -> PathBuf {
    root.join(path.trim_start_matches('/'))
}

pub(crate) fn outdated_killswitch_files(root: &Path) -> Vec<&'static str> {
    let readable = [
        (
            "killswitch-helper.sh",
            KILLSWITCH_HELPER_PATH,
            KILLSWITCH_HELPER,
        ),
        (
            "killswitch.nft",
            KILLSWITCH_RULESET_PATH,
            KILLSWITCH_RULESET,
        ),
        (
            "kvn-tui-killswitch.service",
            KILLSWITCH_UNIT_PATH,
            KILLSWITCH_UNIT,
        ),
    ];
    let mut outdated: Vec<&'static str> = readable
        .into_iter()
        .filter(|(_, path, expected)| !installed_matches(&under_root(root, path), expected))
        .map(|(name, _, _)| name)
        .collect();
    if stamp_state(root, KILLSWITCH_SUDOERS_STAMP_PATH, KILLSWITCH_SUDOERS) != StampState::Current {
        outdated.push("sudoers rule");
    }
    outdated
}

pub(crate) fn polkit_rule_state(root: &Path) -> StampState {
    stamp_state(root, POLKIT_STAMP_PATH, POLKIT_RULE)
}

fn installed_matches(path: &Path, expected: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|installed| installed.trim_end() == expected.trim_end())
}

fn stamp_state(root: &Path, stamp: &str, expected: &str) -> StampState {
    match fs::read_to_string(under_root(root, stamp)) {
        Ok(recorded) if recorded.trim() == sha256_hex(expected.as_bytes()) => StampState::Current,
        Ok(_) => StampState::Outdated,
        Err(_) => StampState::Missing,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
pub(crate) fn install_current(root: &Path) {
    for (path, contents) in [
        (KILLSWITCH_HELPER_PATH, KILLSWITCH_HELPER.to_owned()),
        (KILLSWITCH_RULESET_PATH, KILLSWITCH_RULESET.to_owned()),
        (KILLSWITCH_UNIT_PATH, KILLSWITCH_UNIT.to_owned()),
        (
            KILLSWITCH_SUDOERS_STAMP_PATH,
            format!("{}\n", sha256_hex(KILLSWITCH_SUDOERS.as_bytes())),
        ),
        (
            POLKIT_STAMP_PATH,
            format!("{}\n", sha256_hex(POLKIT_RULE.as_bytes())),
        ),
    ] {
        let path = under_root(root, path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn current_installation_has_no_outdated_files() {
        let root = tempfile::tempdir().unwrap();
        install_current(root.path());
        assert!(outdated_killswitch_files(root.path()).is_empty());
        assert_eq!(polkit_rule_state(root.path()), StampState::Current);
    }

    #[test]
    fn helper_with_extra_trailing_newline_is_current() {
        let root = tempfile::tempdir().unwrap();
        install_current(root.path());
        fs::write(
            under_root(root.path(), KILLSWITCH_HELPER_PATH),
            format!("{KILLSWITCH_HELPER}\n"),
        )
        .unwrap();
        assert!(outdated_killswitch_files(root.path()).is_empty());
    }

    #[test]
    fn changed_and_missing_files_are_reported() {
        let root = tempfile::tempdir().unwrap();
        install_current(root.path());
        fs::write(
            under_root(root.path(), KILLSWITCH_RULESET_PATH),
            "table inet kvn_tui_killswitch {}\n",
        )
        .unwrap();
        fs::write(
            under_root(root.path(), KILLSWITCH_SUDOERS_STAMP_PATH),
            sha256_hex(b"%network ALL=(root) NOPASSWD: /usr/lib/kvn-tui/killswitch-helper.sh\n"),
        )
        .unwrap();
        fs::remove_file(under_root(root.path(), KILLSWITCH_UNIT_PATH)).unwrap();
        assert_eq!(
            outdated_killswitch_files(root.path()),
            [
                "killswitch.nft",
                "kvn-tui-killswitch.service",
                "sudoers rule"
            ]
        );
    }

    #[test]
    fn missing_sudoers_stamp_is_outdated() {
        let root = tempfile::tempdir().unwrap();
        install_current(root.path());
        fs::remove_file(under_root(root.path(), KILLSWITCH_SUDOERS_STAMP_PATH)).unwrap();
        assert_eq!(outdated_killswitch_files(root.path()), ["sudoers rule"]);
    }

    #[test]
    fn polkit_stamp_states() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(polkit_rule_state(root.path()), StampState::Missing);
        install_current(root.path());
        fs::write(under_root(root.path(), POLKIT_STAMP_PATH), "0".repeat(64)).unwrap();
        assert_eq!(polkit_rule_state(root.path()), StampState::Outdated);
    }

    #[test]
    fn payloads_grant_only_the_dedicated_group() {
        assert!(KILLSWITCH_SUDOERS.contains("%kvn-tui ALL=(root) NOPASSWD:"));
        assert!(!KILLSWITCH_SUDOERS.contains("%network"));
        for action in [
            "org.freedesktop.resolve1.set-dns-servers",
            "org.freedesktop.resolve1.set-domains",
            "org.freedesktop.resolve1.set-default-route",
        ] {
            assert!(POLKIT_RULE.contains(action));
        }
        assert!(POLKIT_RULE.contains("subject.isInGroup(\"kvn-tui\")"));
        assert!(!POLKIT_RULE.contains("org.freedesktop.NetworkManager"));
        assert!(!POLKIT_RULE.contains("subject.isInGroup(\"network\")"));
    }
}
