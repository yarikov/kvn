use anyhow::{Context, Result};
use std::ffi::{CStr, CString};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{
    SNAPSHOT_TIMEOUT, clean_killswitch, clean_polkit, confirm, run_embedded_script_command,
    sudo_user_socket_path,
};
use crate::app::msg::IpcCommand;
use crate::ipc::IpcClient;

const INTEGRATION_STATE_DIR: &str = "/var/lib/kvn";
const OMARCHY_BACKUP_MARKER: &str = ".bak.before-kvn-tui";

const OMARCHY_ARTIFACTS: [&str; 4] = [
    ".config/omarchy/plugins/yarikov.omakvn",
    ".local/bin/omarchy-launch-kvn-tui",
    ".local/share/applications/kvn-tui.desktop",
    ".local/share/icons/hicolor/scalable/apps/kvn-tui.svg",
];

const OMARCHY_EDITED_FILES: [(&str, &[&str]); 3] = [
    (
        ".config/omarchy/shell.json",
        &["\"yarikov.omakvn\"", "\"kvn-tui\"", "\"kvn.tui\""],
    ),
    (".config/hypr/bindings.lua", &["-- kvn-tui "]),
    (".config/hypr/hyprland.lua", &["-- kvn-tui "]),
];

const OMARCHY_BACKUP_DIRS: [&str; 5] = [
    ".config/omarchy",
    ".config/hypr",
    ".config/waybar",
    ".local/share/applications",
    ".local/share/icons/hicolor/scalable/apps",
];

#[derive(Debug, PartialEq, Eq)]
struct InvokingUser {
    name: String,
    uid: u32,
    home: PathBuf,
}

pub(super) fn run(yes: bool) -> Result<()> {
    #[allow(unsafe_code)]
    let effective_uid = unsafe { libc::geteuid() };
    let name = invoking_user_name(effective_uid, std::env::var("SUDO_USER").ok().as_deref())?;
    let user = lookup_user(&name)?;
    let footprint = user_footprint(&user.home, user.uid);
    let omarchy = has_omarchy_integration(&user.home);

    if !yes && !confirm_clean_all(&user, omarchy, &footprint)? {
        println!("Clean cancelled");
        return Ok(());
    }

    stop_daemon(user.uid)?;
    clean_killswitch()?;
    clean_polkit()?;
    remove_empty_dir(Path::new(INTEGRATION_STATE_DIR))?;
    if omarchy {
        run_as_user(
            &user,
            "remove-omarchy.sh",
            include_str!("../../contrib/remove-omarchy.sh"),
        )?;
        run_as_user(
            &user,
            "clean-omarchy.sh",
            include_str!("../../contrib/clean-omarchy.sh"),
        )?;
    }

    let removed = remove_all(&footprint)?;
    if removed.is_empty() {
        println!("No kvn user files found.");
    }
    for path in removed {
        println!("Removed {}", path.display());
    }
    Ok(())
}

fn invoking_user_name(effective_uid: u32, sudo_user: Option<&str>) -> Result<String> {
    anyhow::ensure!(
        effective_uid == 0,
        "removing system integrations requires root privileges; run `sudo kvn clean --all`"
    );
    match sudo_user {
        Some(user) if !user.is_empty() && user != "root" => Ok(user.to_string()),
        _ => anyhow::bail!(
            "could not identify a non-root invoking user; run this command via sudo from a non-root account"
        ),
    }
}

fn lookup_user(name: &str) -> Result<InvokingUser> {
    let c_name = CString::new(name).context("invalid user name")?;
    #[allow(unsafe_code)]
    let entry = unsafe { libc::getpwnam(c_name.as_ptr()) };
    anyhow::ensure!(!entry.is_null(), "user {name} does not exist");
    #[allow(unsafe_code)]
    let (uid, home) = unsafe {
        let entry = &*entry;
        (entry.pw_uid, CStr::from_ptr(entry.pw_dir).to_owned())
    };
    let home = PathBuf::from(home.into_string().context("home directory is not UTF-8")?);
    anyhow::ensure!(
        home.is_absolute() && home != Path::new("/"),
        "refusing to clean user files under home directory {}",
        home.display()
    );
    Ok(InvokingUser {
        name: name.to_string(),
        uid,
        home,
    })
}

fn user_footprint(home: &Path, uid: u32) -> Vec<PathBuf> {
    let runtime = PathBuf::from(format!("/run/user/{uid}"));
    vec![
        home.join(".config/kvn-tui"),
        home.join(".local/state/kvn"),
        runtime.join("kvn-tui"),
        runtime.join("kvn"),
        runtime.join("kvn-tui.sock"),
    ]
}

fn has_omarchy_integration(home: &Path) -> bool {
    let artifact_exists = OMARCHY_ARTIFACTS
        .iter()
        .any(|path| std::fs::symlink_metadata(home.join(path)).is_ok());
    let edit_present = OMARCHY_EDITED_FILES.iter().any(|(path, needles)| {
        std::fs::read_to_string(home.join(path))
            .is_ok_and(|contents| needles.iter().any(|needle| contents.contains(needle)))
    });
    let backup_present = OMARCHY_BACKUP_DIRS.iter().any(|dir| {
        std::fs::read_dir(home.join(dir)).is_ok_and(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(OMARCHY_BACKUP_MARKER)
            })
        })
    });
    artifact_exists || edit_present || backup_present
}

fn clean_all_question(user: &InvokingUser, omarchy: bool, footprint: &[PathBuf]) -> String {
    let paths: String = footprint
        .iter()
        .map(|path| format!("  {}\n", path.display()))
        .collect();
    let omarchy_integration = if omarchy {
        "the Omarchy integration (bar plugin and widget, Hyprland shortcut and window rule, \
         launcher and backup files), "
    } else {
        ""
    };
    format!(
        "This stops the kvn daemon, removes the polkit and kill-switch integrations, \
         {omarchy_integration}and permanently deletes all kvn data of {}, \
         including every profile:\n{paths}Continue?",
        user.name
    )
}

fn confirm_clean_all(user: &InvokingUser, omarchy: bool, footprint: &[PathBuf]) -> Result<bool> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    confirm(
        &mut stdin.lock(),
        &mut stdout.lock(),
        &clean_all_question(user, omarchy, footprint),
    )
}

fn stop_daemon(uid: u32) -> Result<()> {
    let Some(socket) = sudo_user_socket_path(Some(&uid.to_string())) else {
        return Ok(());
    };
    let Ok(mut client) = IpcClient::connect_at(&socket) else {
        return Ok(());
    };
    client
        .send(&IpcCommand::Quit)
        .context("failed to ask the kvn daemon to stop")?;
    let deadline = Instant::now() + SNAPSHOT_TIMEOUT;
    while IpcClient::connect_at(&socket).is_ok() {
        anyhow::ensure!(
            Instant::now() < deadline,
            "the kvn daemon did not stop; run `systemctl --user stop kvn-tui.service` and retry"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    println!("Stopped the kvn daemon.");
    Ok(())
}

fn run_as_user(user: &InvokingUser, name: &str, script: &str) -> Result<()> {
    let mut command = std::process::Command::new("runuser");
    command.args(user_script_args(user, name, script));
    run_embedded_script_command(name, command)
}

fn user_script_args(user: &InvokingUser, name: &str, script: &str) -> Vec<String> {
    vec![
        "-u".to_string(),
        user.name.clone(),
        "--".to_string(),
        "env".to_string(),
        format!("HOME={}", user.home.display()),
        format!("XDG_RUNTIME_DIR=/run/user/{}", user.uid),
        "bash".to_string(),
        "-c".to_string(),
        script.to_string(),
        name.to_string(),
    ]
}

fn remove_all(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for path in paths {
        if remove_path(path)? {
            removed.push(path.clone());
        }
    }
    Ok(removed)
}

fn remove_path(path: &Path) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    let result = if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.with_context(|| format!("failed to remove {}", path.display()))?;
    Ok(true)
}

fn remove_empty_dir(path: &Path) -> Result<()> {
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn clean_all_requires_root_and_a_non_root_sudo_user() {
        assert_eq!(invoking_user_name(0, Some("alice")).unwrap(), "alice");

        let as_user = invoking_user_name(1000, Some("alice"))
            .unwrap_err()
            .to_string();
        assert!(as_user.contains("sudo kvn clean --all"));

        for sudo_user in [None, Some(""), Some("root")] {
            let error = invoking_user_name(0, sudo_user).unwrap_err().to_string();
            assert!(error.contains("non-root invoking user"));
        }
    }

    #[test]
    fn user_footprint_covers_config_state_and_runtime_files() {
        assert_eq!(
            user_footprint(Path::new("/home/alice"), 1000),
            [
                "/home/alice/.config/kvn-tui",
                "/home/alice/.local/state/kvn",
                "/run/user/1000/kvn-tui",
                "/run/user/1000/kvn",
                "/run/user/1000/kvn-tui.sock",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn clean_all_question_names_the_user_and_every_path() {
        let user = InvokingUser {
            name: "alice".to_string(),
            uid: 1000,
            home: PathBuf::from("/home/alice"),
        };
        let footprint = user_footprint(&user.home, user.uid);
        let question = clean_all_question(&user, false, &footprint);
        assert!(question.contains("alice"));
        assert!(!question.contains("Omarchy"));
        for path in &footprint {
            assert!(question.contains(&format!("  {}\n", path.display())));
        }
        assert!(clean_all_question(&user, true, &footprint).contains("the Omarchy integration"));
    }

    #[test]
    fn omarchy_integration_is_detected_from_any_trace_setup_leaves() {
        let traces: [(&str, &str); 5] = [
            (".config/omarchy/plugins/yarikov.omakvn/manifest.json", "{}"),
            (".local/bin/omarchy-launch-kvn-tui", ""),
            (
                ".config/omarchy/shell.json",
                r#"{"bar":{"layout":{"right":[{"id":"yarikov.omakvn"}]}}}"#,
            ),
            (
                ".config/hypr/hyprland.lua",
                "-- kvn-tui window rule: begin\n-- kvn-tui window rule: end\n",
            ),
            (
                ".config/hypr/bindings.lua.bak.before-kvn-tui.20260821143012",
                "",
            ),
        ];
        for (path, contents) in traces {
            let home = tempfile::tempdir().unwrap();
            let omarchy = home.path().join(".config/omarchy");
            fs::create_dir_all(&omarchy).unwrap();
            fs::write(omarchy.join("shell.json"), r#"{"bar":{}}"#).unwrap();
            assert!(!has_omarchy_integration(home.path()));

            let trace = home.path().join(path);
            fs::create_dir_all(trace.parent().unwrap()).unwrap();
            fs::write(&trace, contents).unwrap();
            assert!(has_omarchy_integration(home.path()), "{path} not detected");
        }
    }

    #[test]
    fn user_scripts_run_as_the_invoking_user_in_their_session() {
        let user = InvokingUser {
            name: "alice".to_string(),
            uid: 1000,
            home: PathBuf::from("/home/alice"),
        };
        assert_eq!(
            user_script_args(&user, "remove-omarchy.sh", "echo ok"),
            [
                "-u",
                "alice",
                "--",
                "env",
                "HOME=/home/alice",
                "XDG_RUNTIME_DIR=/run/user/1000",
                "bash",
                "-c",
                "echo ok",
                "remove-omarchy.sh",
            ]
        );
    }

    #[test]
    fn remove_all_deletes_directories_and_files_and_skips_missing_paths() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config");
        fs::create_dir_all(dir.join("geo")).unwrap();
        fs::write(dir.join("profiles.json"), "{}").unwrap();
        let file = root.path().join("kvn-tui.sock");
        fs::write(&file, "").unwrap();
        let missing = root.path().join("missing");

        let removed = remove_all(&[dir.clone(), file.clone(), missing]).unwrap();

        assert_eq!(removed, [dir.clone(), file.clone()]);
        assert!(!dir.exists());
        assert!(!file.exists());
    }

    #[test]
    fn remove_path_does_not_follow_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), "").unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(remove_path(&link).unwrap());

        assert!(fs::symlink_metadata(&link).is_err());
        assert!(target.join("keep").exists());
    }

    #[test]
    fn remove_empty_dir_keeps_directories_with_contents() {
        let root = tempfile::tempdir().unwrap();
        let empty = root.path().join("empty");
        let full = root.path().join("full");
        fs::create_dir(&empty).unwrap();
        fs::create_dir(&full).unwrap();
        fs::write(full.join("stamp"), "").unwrap();

        remove_empty_dir(&empty).unwrap();
        remove_empty_dir(&full).unwrap();
        remove_empty_dir(&root.path().join("missing")).unwrap();

        assert!(!empty.exists());
        assert!(full.exists());
    }
}
