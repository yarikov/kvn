use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{ChildStderr, Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::profile::{Profile, Settings};
use crate::singbox::config::{GeoAvailability, generate_config};
use crate::singbox::process_handle::ProcessHandle;

fn resolve_singbox_binary() -> String {
    std::env::var("SING_BOX_PATH").unwrap_or_else(|_| "sing-box".to_string())
}

static SINGBOX_BINARY: OnceLock<String> = OnceLock::new();

/// Path to the sing-box binary. Can be overridden by SING_BOX_PATH env variable.
fn singbox_binary() -> &'static str {
    SINGBOX_BINARY.get_or_init(resolve_singbox_binary)
}

/// Write the generated sing-box configuration to a temporary file.
fn write_config(
    profile: &Profile,
    settings: &Settings,
    geo: &GeoAvailability,
    clash_api_port: u16,
) -> Result<PathBuf> {
    let config = generate_config(profile, settings, geo, clash_api_port)
        .context("Failed to generate sing-box config")?;
    crate::paths::ensure_runtime_dir()?;
    let path = crate::paths::temp_singbox_config_path()?;

    crate::atomic_write::write(&path, serde_json::to_string_pretty(&config)?.as_bytes())
        .with_context(|| format!("Failed to write config to {:?}", path))?;

    Ok(path)
}

/// Validate the sing-box configuration by running `sing-box check`.
fn check_config(path: &PathBuf) -> Result<()> {
    let output = Command::new(singbox_binary())
        .arg("check")
        .arg("-c")
        .arg(path)
        .output()
        .with_context(|| format!("Failed to run {} check", singbox_binary()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("sing-box config validation failed: {}", stderr);
    }

    Ok(())
}

fn collect_geo_availability() -> GeoAvailability {
    let mut geo = GeoAvailability::default();
    let Ok(gm) = crate::geo::GeoManager::new() else {
        return geo;
    };
    for region in crate::config::profile::GeoRegion::ALL {
        let Some((geoip, geosite)) = gm.local_paths(region) else {
            continue;
        };
        if geoip.exists() && geosite.exists() {
            geo.regions.insert(region, (geoip, geosite));
        }
    }
    for service in crate::config::profile::RoutedService::ALL {
        if gm.has_service_databases(service) {
            geo.services
                .insert(service, gm.service_local_paths(service));
        }
    }
    geo
}

const CLASH_API_START_ATTEMPTS: usize = 3;

/// Start the sing-box process with the given profile.
/// Validates config first, then spawns the process and verifies it stays alive.
pub fn start(profile: &Profile, settings: &Settings) -> Result<ProcessHandle> {
    let geo = collect_geo_availability();
    let mut config_validated = false;
    start_with(
        CLASH_API_START_ATTEMPTS,
        crate::net::allocate_loopback_port,
        |clash_api_port| {
            let config_path = write_config(profile, settings, &geo, clash_api_port)?;
            if !config_validated {
                check_config(&config_path)?;
                config_validated = true;
            }
            spawn_and_wait(&config_path, clash_api_port)
        },
    )
}

fn start_with<T>(
    attempts: usize,
    mut allocate_port: impl FnMut() -> Result<u16>,
    mut attempt: impl FnMut(u16) -> Result<T>,
) -> Result<T> {
    let mut tried = 1;
    loop {
        let port = allocate_port()?;
        let error = match attempt(port) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if !is_clash_port_conflict(&format!("{error:#}"), port) {
            return Err(error);
        }
        if tried >= attempts {
            return Err(error.context(format!(
                "sing-box could not bind the Clash API port after {attempts} attempts (last tried 127.0.0.1:{port})"
            )));
        }
        tracing::warn!("Clash API port {port} was taken before sing-box could bind it; retrying");
        tried += 1;
    }
}

fn is_clash_port_conflict(error: &str, port: u16) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("address already in use") && mentions_exact_loopback_port(&error, port)
}

fn mentions_exact_loopback_port(error: &str, port: u16) -> bool {
    let address = format!("127.0.0.1:{port}");
    error
        .match_indices(&address)
        .any(|(start, _)| !error[start + address.len()..].starts_with(|c: char| c.is_ascii_digit()))
}

fn spawn_and_wait(config_path: &PathBuf, clash_api_port: u16) -> Result<ProcessHandle> {
    // We don't consume sing-box stdout; drop it to /dev/null so a verbose
    // logger can't fill the pipe buffer (~64K) and wedge the child on write.
    // stderr stays piped — on immediate exit we read it for diagnostics, and
    // on success we hand it to a drain thread (see `spawn_stderr_drain`).
    let mut child = Command::new(singbox_binary())
        .arg("run")
        .arg("-c")
        .arg(config_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to start sing-box (binary: {})", singbox_binary()))?;

    // Poll for either an immediate exit (config rejected, port busy, etc.)
    // or a stable run. The 300ms budget is the same heuristic as before —
    // sing-box that survives this window is considered up — but we now
    // detect an early failure within ~10ms instead of always waiting the
    // full window.
    let deadline = std::time::Instant::now() + Duration::from_millis(300);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stderr = String::new();
                if let Some(ref mut err) = child.stderr
                    && let Err(e) = err.read_to_string(&mut stderr)
                {
                    tracing::warn!("Failed to read sing-box stderr: {}", e);
                }
                anyhow::bail!(
                    "sing-box exited immediately (code: {:?}). stderr: {}",
                    status.code(),
                    stderr.trim()
                );
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // Process survived the readiness window — drain stderr so
                    // the child doesn't block on a full pipe buffer, then hand
                    // the child off.
                    if let Some(stderr) = child.stderr.take() {
                        spawn_stderr_drain(stderr);
                    }
                    return Ok(ProcessHandle::new(child, clash_api_port));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                anyhow::bail!("Failed to check sing-box status: {}", e);
            }
        }
    }
}

/// Forward sing-box stderr lines to the `singbox` tracing target so verbose
/// output never wedges the child. The thread exits naturally on EOF (i.e.
/// when sing-box is killed and the pipe closes).
fn spawn_stderr_drain(stderr: ChildStderr) {
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) => tracing::info!(target: "singbox", "{line}"),
                Err(_) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::Profile;
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    const TEST_CLASH_PORT: u16 = 41390;

    #[test]
    fn singbox_binary_resolution() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        // Default (no env override)
        unsafe { std::env::remove_var("SING_BOX_PATH") };
        assert_eq!(resolve_singbox_binary(), "sing-box");

        // With env override
        unsafe { std::env::set_var("SING_BOX_PATH", "/usr/local/bin/sing-box") };
        assert_eq!(resolve_singbox_binary(), "/usr/local/bin/sing-box");
        unsafe { std::env::remove_var("SING_BOX_PATH") };
    }

    #[test]
    fn write_config_creates_valid_json() {
        // temp_singbox_config_path() reads XDG_RUNTIME_DIR, which other tests
        // point at short-lived tempdirs — hold ENV_LOCK so the path stays valid.
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        let profile = Profile::new_vless(
            "Test".to_string(),
            "1.2.3.4".to_string(),
            443,
            "uuid".to_string(),
        );
        let settings = Settings::default();
        let path = write_config(
            &profile,
            &settings,
            &GeoAvailability::default(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        assert!(path.exists());

        let contents = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert!(json.get("log").is_some());
        assert!(json.get("outbounds").is_some());
        assert_eq!(
            json["experimental"]["clash_api"]["external_controller"],
            format!("127.0.0.1:{TEST_CLASH_PORT}")
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // Clean up
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn collect_geo_availability_is_empty_when_no_files() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let geo = collect_geo_availability();
        assert!(geo.regions.is_empty());
    }

    #[test]
    fn collect_geo_availability_populates_each_region_independently() {
        use crate::config::profile::GeoRegion;
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = crate::geo::GeoManager::new().unwrap();

        // Add one region at a time, asserting that prior regions remain
        // available and unconfigured regions stay absent.
        let mut written = Vec::new();
        for region in [GeoRegion::Ru, GeoRegion::Cn, GeoRegion::Ir] {
            let (geoip, geosite) = gm.local_paths(region).unwrap();
            fs::write(&geoip, b"x").unwrap();
            fs::write(&geosite, b"x").unwrap();
            written.push(region);

            let geo = collect_geo_availability();
            for r in [GeoRegion::Ru, GeoRegion::Cn, GeoRegion::Ir] {
                assert_eq!(
                    geo.regions.contains_key(&r),
                    written.contains(&r),
                    "region {:?} after writing {:?}",
                    r,
                    written
                );
            }
        }
    }

    #[test]
    fn collect_geo_availability_skips_region_when_only_one_file_present() {
        use crate::config::profile::GeoRegion;
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = crate::geo::GeoManager::new().unwrap();
        let (geoip_ru, _) = gm.local_paths(GeoRegion::Ru).unwrap();
        fs::write(&geoip_ru, b"x").unwrap();
        // geosite missing → Ru must stay absent.
        let geo = collect_geo_availability();
        assert!(!geo.regions.contains_key(&GeoRegion::Ru));
    }

    #[test]
    fn collect_geo_availability_includes_service_when_all_files_present() {
        use crate::config::profile::RoutedService;

        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = crate::geo::GeoManager::new().unwrap();
        let paths = gm.service_local_paths(RoutedService::Steam);

        assert!(
            !collect_geo_availability()
                .services
                .contains_key(&RoutedService::Steam)
        );
        fs::write(&paths[0].1, b"x").unwrap();
        assert!(
            !collect_geo_availability()
                .services
                .contains_key(&RoutedService::Steam),
            "geoip missing"
        );
        fs::write(&paths[1].1, b"x").unwrap();
        assert_eq!(
            collect_geo_availability()
                .services
                .get(&RoutedService::Steam),
            Some(&paths)
        );
    }

    /// Validation against the real `sing-box` binary. Skipped automatically
    /// when the binary is not on PATH so CI without sing-box stays green.
    #[test]
    fn check_config_accepts_a_minimal_profile() {
        // Same XDG_RUNTIME_DIR dependency as write_config_creates_valid_json.
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if Command::new("sh")
            .args(["-c", "command -v sing-box >/dev/null 2>&1"])
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            eprintln!("skipping: sing-box not on PATH");
            return;
        }
        let profile =
            Profile::new_vless("T".to_string(), "1.1.1.1".to_string(), 443, "u".to_string());
        let settings = Settings::default();
        let path = write_config(
            &profile,
            &settings,
            &GeoAvailability::default(),
            TEST_CLASH_PORT,
        )
        .unwrap();
        check_config(&path).expect("sing-box rejected a minimal vless profile");
        let _ = fs::remove_file(&path);
    }

    fn conflict_stderr(port: impl std::fmt::Display) -> anyhow::Error {
        anyhow::anyhow!(
            "sing-box exited immediately (code: Some(1)). stderr: FATAL[0000] start service: listen tcp 127.0.0.1:{port}: bind: address already in use"
        )
    }

    #[test]
    fn clash_port_conflict_matches_our_port() {
        assert!(is_clash_port_conflict(
            &format!("{:#}", conflict_stderr(41390)),
            41390
        ));
    }

    #[test]
    fn clash_port_conflict_ignores_another_port() {
        assert!(!is_clash_port_conflict(
            &format!("{:#}", conflict_stderr(53)),
            41390
        ));
    }

    #[test]
    fn clash_port_conflict_ignores_a_longer_port_with_the_same_prefix() {
        assert!(!is_clash_port_conflict(
            &format!("{:#}", conflict_stderr(413905)),
            41390
        ));
    }

    #[test]
    fn clash_port_conflict_matches_real_singbox_stderr() {
        let observed = "sing-box exited immediately (code: Some(1)). stderr: \u{1b}[31mFATAL\u{1b}[0m[0000] start service: finish-start clash server: external controller listen error: listen tcp 127.0.0.1:47551: bind: address already in use";
        assert!(is_clash_port_conflict(observed, 47551));
        assert!(!is_clash_port_conflict(observed, 4755));
    }

    #[test]
    fn clash_port_conflict_requires_address_in_use() {
        let error = "sing-box exited immediately (code: Some(1)). stderr: listen tcp 127.0.0.1:41390: bind: permission denied";
        assert!(!is_clash_port_conflict(error, 41390));
    }

    #[test]
    fn start_with_returns_the_first_success() {
        let allocations = RefCell::new(0);
        let result = start_with(
            3,
            || {
                *allocations.borrow_mut() += 1;
                Ok(41390)
            },
            Ok,
        )
        .unwrap();
        assert_eq!(result, 41390);
        assert_eq!(*allocations.borrow(), 1);
    }

    #[test]
    fn start_with_retries_a_conflict_on_a_fresh_port() {
        let ports = RefCell::new(vec![41390u16, 41391]);
        let tried = RefCell::new(Vec::new());
        let result = start_with(
            3,
            || Ok(ports.borrow_mut().remove(0)),
            |port| {
                tried.borrow_mut().push(port);
                if port == 41390 {
                    Err(conflict_stderr(port))
                } else {
                    Ok(port)
                }
            },
        )
        .unwrap();
        assert_eq!(result, 41391);
        assert_eq!(*tried.borrow(), vec![41390, 41391]);
    }

    #[test]
    fn start_with_does_not_retry_unrelated_failures() {
        let tried = RefCell::new(0);
        let error = start_with(
            3,
            || Ok(41390),
            |_| -> Result<u16> {
                *tried.borrow_mut() += 1;
                anyhow::bail!("sing-box config validation failed: bad outbound")
            },
        )
        .unwrap_err();
        assert_eq!(*tried.borrow(), 1);
        assert_eq!(
            format!("{error:#}"),
            "sing-box config validation failed: bad outbound"
        );
    }

    #[test]
    fn start_with_does_not_retry_a_conflict_on_another_port() {
        let tried = RefCell::new(0);
        start_with(
            3,
            || Ok(41390),
            |_| -> Result<u16> {
                *tried.borrow_mut() += 1;
                Err(conflict_stderr(53))
            },
        )
        .unwrap_err();
        assert_eq!(*tried.borrow(), 1);
    }

    #[test]
    fn start_with_reports_exhausted_attempts() {
        let ports = RefCell::new(vec![41390u16, 41391, 41392]);
        let tried = RefCell::new(0);
        let error = start_with(
            CLASH_API_START_ATTEMPTS,
            || Ok(ports.borrow_mut().remove(0)),
            |port| -> Result<u16> {
                *tried.borrow_mut() += 1;
                Err(conflict_stderr(port))
            },
        )
        .unwrap_err();
        assert_eq!(*tried.borrow(), CLASH_API_START_ATTEMPTS);
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains(&format!(
                "could not bind the Clash API port after {CLASH_API_START_ATTEMPTS} attempts"
            )),
            "{rendered}"
        );
        assert!(rendered.contains("127.0.0.1:41392"), "{rendered}");
        assert!(rendered.contains("address already in use"), "{rendered}");
    }

    #[test]
    fn start_with_propagates_an_allocation_failure() {
        let tried = RefCell::new(0);
        let error = start_with(
            3,
            || anyhow::bail!("no free port"),
            |_| -> Result<u16> {
                *tried.borrow_mut() += 1;
                Ok(0)
            },
        )
        .unwrap_err();
        assert_eq!(*tried.borrow(), 0);
        assert_eq!(format!("{error:#}"), "no free port");
    }
}
