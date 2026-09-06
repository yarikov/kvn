use std::os::unix::fs::symlink;
use std::process::{Command, Output};

const LEGACY_WARNING: &str = "Warning: `kvn-tui` is a legacy command and will be removed in a future release.\nUse `kvn` instead.\n\n";

fn invoke_through_symlink(name: &str, argument: &str) -> Output {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join(name);
    symlink(env!("CARGO_BIN_EXE_kvn-tui"), &executable).unwrap();
    Command::new(&executable).arg(argument).output().unwrap()
}

#[test]
fn help_is_identical_except_for_the_legacy_stderr_warning() {
    let canonical = invoke_through_symlink("kvn", "--help");
    let legacy = invoke_through_symlink("kvn-tui", "--help");

    assert!(canonical.status.success());
    assert_eq!(canonical.status, legacy.status);
    assert_eq!(canonical.stdout, legacy.stdout);
    assert!(canonical.stderr.is_empty());
    assert_eq!(String::from_utf8(legacy.stderr).unwrap(), LEGACY_WARNING);
    assert!(
        String::from_utf8(canonical.stdout)
            .unwrap()
            .contains("Usage: kvn")
    );
}

#[test]
fn version_is_identical_except_for_the_legacy_stderr_warning() {
    let canonical = invoke_through_symlink("kvn", "--version");
    let legacy = invoke_through_symlink("kvn-tui", "--version");

    assert!(canonical.status.success());
    assert_eq!(canonical.status, legacy.status);
    assert_eq!(canonical.stdout, legacy.stdout);
    assert_eq!(
        String::from_utf8(canonical.stdout).unwrap(),
        format!("kvn {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(canonical.stderr.is_empty());
    assert_eq!(String::from_utf8(legacy.stderr).unwrap(), LEGACY_WARNING);
}
