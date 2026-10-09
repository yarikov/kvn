use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
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

pub fn binary_path() -> Option<PathBuf> {
    locate_binary(singbox_binary(), std::env::var_os("PATH").as_deref())
}

fn locate_binary(binary: &str, search_path: Option<&OsStr>) -> Option<PathBuf> {
    if binary.contains('/') {
        return Some(PathBuf::from(binary));
    }
    std::env::split_paths(search_path?)
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) && process_may_execute(path)
}

fn process_may_execute(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    #[allow(unsafe_code)]
    let result =
        unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::X_OK, libc::AT_EACCESS) };
    result == 0
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
    if let Some(state_dir) = crate::paths::state_dir() {
        fs::create_dir_all(&state_dir)
            .with_context(|| format!("Failed to create state directory {:?}", state_dir))?;
    }
    let path = crate::paths::temp_singbox_config_path()?;

    crate::atomic_write::write(&path, serde_json::to_string_pretty(&config)?.as_bytes())
        .with_context(|| format!("Failed to write config to {:?}", path))?;

    Ok(path)
}

/// Validate the sing-box configuration by running `sing-box check`.
fn check_config(path: &PathBuf, cancelled: &dyn Fn() -> bool) -> Result<()> {
    let mut command = Command::new(singbox_binary());
    command
        .arg("check")
        .arg("--disable-color")
        .arg("-c")
        .arg(path);
    let (status, stderr) = run_until_exit(command, cancelled)
        .with_context(|| format!("Failed to run {} check", singbox_binary()))?;

    if !status.success() {
        anyhow::bail!("sing-box config validation failed: {}", stderr);
    }

    Ok(())
}

fn run_until_exit(
    mut command: Command,
    cancelled: &dyn Fn() -> bool,
) -> Result<(ExitStatus, String)> {
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        })
    });
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancelled() {
            stop_child(&mut child);
            anyhow::bail!(START_CANCELLED);
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stderr = stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    Ok((status, stderr))
}

const START_CANCELLED: &str = "sing-box start was cancelled";

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
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
pub fn start(
    profile: &Profile,
    settings: &Settings,
    cancelled: &dyn Fn() -> bool,
) -> Result<ProcessHandle> {
    let geo = collect_geo_availability();
    let mut config_validated = false;
    start_with(
        CLASH_API_START_ATTEMPTS,
        crate::net::allocate_loopback_port,
        |clash_api_port| {
            let config_path = write_config(profile, settings, &geo, clash_api_port)?;
            if !config_validated {
                check_config(&config_path, cancelled)?;
                config_validated = true;
            }
            if cancelled() {
                anyhow::bail!(START_CANCELLED);
            }
            spawn_and_wait(&config_path, clash_api_port, cancelled)
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

fn spawn_and_wait(
    config_path: &PathBuf,
    clash_api_port: u16,
    cancelled: &dyn Fn() -> bool,
) -> Result<ProcessHandle> {
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
                if cancelled() {
                    stop_child(&mut child);
                    anyhow::bail!(START_CANCELLED);
                }
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

    fn owner_may_not_execute_mode() -> u32 {
        #[allow(unsafe_code)]
        let running_as_root = unsafe { libc::geteuid() } == 0;
        if running_as_root { 0o644 } else { 0o641 }
    }

    #[test]
    fn locate_binary_takes_a_path_as_is_and_searches_path_for_an_executable() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        fs::write(first.path().join("sing-box"), "").unwrap();
        fs::set_permissions(
            first.path().join("sing-box"),
            fs::Permissions::from_mode(owner_may_not_execute_mode()),
        )
        .unwrap();
        fs::write(second.path().join("sing-box"), "").unwrap();
        fs::set_permissions(
            second.path().join("sing-box"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let search_path = std::env::join_paths([first.path(), second.path()]).unwrap();

        assert_eq!(
            locate_binary("/opt/sing-box", Some(&search_path)),
            Some(PathBuf::from("/opt/sing-box"))
        );
        assert_eq!(
            locate_binary("sing-box", Some(&search_path)),
            Some(second.path().join("sing-box"))
        );
        assert_eq!(locate_binary("missing", Some(&search_path)), None);
        assert_eq!(locate_binary("sing-box", None), None);
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

    fn sing_box_on_path() -> bool {
        let found = Command::new("sh")
            .args(["-c", "command -v sing-box >/dev/null 2>&1"])
            .status()
            .is_ok_and(|status| status.success());
        if !found {
            eprintln!("skipping: sing-box not on PATH");
        }
        found
    }

    fn check_generated_config(profile: &Profile) -> Result<()> {
        let path = write_config(
            profile,
            &Settings::default(),
            &GeoAvailability::default(),
            TEST_CLASH_PORT,
        )?;
        let result = check_config(&path, &|| false);
        let _ = fs::remove_file(&path);
        result
    }

    fn vmess_with_transport(transport: crate::config::profile::TransportConfig) -> Profile {
        let mut profile =
            Profile::new_vless("T".to_string(), "1.1.1.1".to_string(), 443, "u".to_string());
        profile.config =
            crate::config::profile::ProtocolConfig::Vmess(crate::config::profile::VmessConfig {
                uuid: crate::test_helpers::TEST_UUID.to_string(),
                transport: Some(transport),
                ..Default::default()
            });
        profile
    }

    /// Validation against the real `sing-box` binary. Skipped automatically
    /// when the binary is not on PATH so CI without sing-box stays green.
    #[test]
    fn check_config_accepts_a_minimal_profile() {
        // Same XDG_RUNTIME_DIR dependency as write_config_creates_valid_json.
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        let profile =
            Profile::new_vless("T".to_string(), "1.1.1.1".to_string(), 443, "u".to_string());
        check_generated_config(&profile).expect("sing-box rejected a minimal vless profile");
    }

    #[test]
    fn check_config_accepts_every_generated_transport() {
        use crate::config::profile::{TransportConfig, TransportType};
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        for kind in [
            TransportType::Ws,
            TransportType::Http,
            TransportType::Grpc,
            TransportType::HttpUpgrade,
            TransportType::Quic,
        ] {
            let profile = vmess_with_transport(TransportConfig {
                kind: kind.clone(),
                path: Some("/p".to_string()),
                host: Some("h.example.com".to_string()),
                service_name: Some("svc".to_string()),
                headers: Default::default(),
            });
            check_generated_config(&profile)
                .unwrap_or_else(|error| panic!("sing-box rejected {kind:?}: {error:#}"));
        }
    }

    #[test]
    fn check_config_runs_shadowsocks_plugins_sing_box_ships_and_names_others() {
        use crate::config::profile::{ProtocolConfig, ShadowsocksCipher, ShadowsocksConfig};
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        let with_plugin = |plugin: &str| {
            let mut profile = Profile::new_vless(
                "T".to_string(),
                "1.1.1.1".to_string(),
                8388,
                "u".to_string(),
            );
            profile.config = ProtocolConfig::Shadowsocks(ShadowsocksConfig {
                method: ShadowsocksCipher::Aes128Gcm,
                password: "p".to_string(),
            });
            profile
                .share_link_params
                .insert("plugin".to_string(), plugin.into());
            profile
        };

        for plugin in ["obfs-local;obfs=http;obfs-host=a.example", "v2ray-plugin"] {
            check_generated_config(&with_plugin(plugin))
                .unwrap_or_else(|error| panic!("sing-box rejected {plugin}: {error:#}"));
        }
        let error = check_generated_config(&with_plugin("kcptun"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("plugin not found: kcptun"), "{error}");
    }

    #[test]
    fn check_config_accepts_hysteria2_port_hopping() {
        use crate::config::profile::{Hysteria2Config, ProtocolConfig};
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        let mut profile =
            Profile::new_vless("T".to_string(), "1.1.1.1".to_string(), 443, "u".to_string());
        profile.config = ProtocolConfig::Hysteria2(Hysteria2Config {
            password: "p".to_string(),
            ..Default::default()
        });
        profile
            .share_link_params
            .insert("mport".to_string(), "20000-30000,443".into());

        check_generated_config(&profile).expect("sing-box rejected Hysteria2 port hopping");
    }

    #[test]
    fn check_config_accepts_plain_vmess_and_tcp_http_header_without_tls() {
        use crate::config::profile::{ProtocolConfig, Security, TransportConfig, TransportType};
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        for kind in [TransportType::Ws, TransportType::Http] {
            let mut profile = vmess_with_transport(TransportConfig {
                kind,
                path: Some("/p".to_string()),
                host: Some("h.example.com".to_string()),
                service_name: None,
                headers: Default::default(),
            });
            if let ProtocolConfig::Vmess(cfg) = &mut profile.config {
                cfg.stream_security = Some(Security::None);
            }
            profile
                .share_link_params
                .insert("headerType".to_string(), "http".into());
            check_generated_config(&profile).expect("sing-box rejected VMess without TLS");
        }
    }

    #[test]
    fn check_config_reports_an_unknown_transport_without_colors() {
        use crate::config::profile::{TransportConfig, TransportType};
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        let profile = vmess_with_transport(TransportConfig {
            kind: TransportType::Other("xhttp".to_string()),
            path: None,
            host: None,
            service_name: None,
            headers: Default::default(),
        });

        let error = check_generated_config(&profile).unwrap_err().to_string();

        assert!(error.contains("unknown transport type: xhttp"), "{error}");
        assert!(!error.contains('\u{1b}'), "{error:?}");
    }

    #[test]
    fn run_until_exit_stops_a_cancelled_process() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut command = Command::new("sleep");
        command.arg("5");
        let started = std::time::Instant::now();
        let error = run_until_exit(command, &|| true).unwrap_err();
        assert_eq!(error.to_string(), START_CANCELLED);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn run_until_exit_returns_the_status_and_stderr() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "echo rejected >&2; exit 3"]);
        let (status, stderr) = run_until_exit(command, &|| false).unwrap();
        assert_eq!(status.code(), Some(3));
        assert_eq!(stderr.trim(), "rejected");
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
