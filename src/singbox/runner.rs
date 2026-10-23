use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::profile::{Profile, RoutingFallback};
use crate::singbox::config::{GeoAvailability, RoutingInput, generate_config};
use crate::singbox::process_handle::ProcessHandle;

fn resolve_singbox_binary() -> String {
    std::env::var("SING_BOX_PATH").unwrap_or_else(|_| "sing-box".to_string())
}

static SINGBOX_BINARY: OnceLock<String> = OnceLock::new();

fn singbox_binary() -> &'static str {
    SINGBOX_BINARY.get_or_init(resolve_singbox_binary)
}

fn write_config(
    profile: &Profile,
    input: RoutingInput,
    geo: &GeoAvailability,
    fallback: Option<RoutingFallback>,
    clash_api_port: u16,
) -> Result<PathBuf> {
    let config = generate_config(profile, input, geo, clash_api_port, fallback)
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
    let mut child = spawn_singbox(command.stdout(Stdio::null()).stderr(Stdio::piped()))?;
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

fn collect_geo_availability(required: &[crate::geo::RuleSet]) -> GeoAvailability {
    let mut geo = GeoAvailability::default();
    let Ok(gm) = crate::geo::GeoManager::new() else {
        return geo;
    };
    let states = gm.states(required);
    for rule_set in required {
        if states
            .get(&rule_set.key())
            .is_some_and(|state| state.present)
        {
            geo.rule_sets
                .insert(rule_set.tag.to_string(), gm.rule_set_path(rule_set));
        }
    }
    geo
}

const CLASH_API_START_ATTEMPTS: usize = 3;

pub struct StartedSingBox {
    pub handle: ProcessHandle,
    pub routing_fallback: Option<RoutingFallback>,
}

pub fn start(
    profile: &Profile,
    input: RoutingInput,
    cancelled: &dyn Fn() -> bool,
) -> Result<StartedSingBox> {
    let RoutingInput { settings, routing } = input;
    let geo = collect_geo_availability(&crate::route_plan::active_rule_sets(settings, routing));
    let routing_fallback = geo.routing_fallback(&crate::route_plan::RoutePlan::for_settings(
        settings, routing,
    ));
    let mut config_validated = false;
    let handle = start_with(
        CLASH_API_START_ATTEMPTS,
        crate::net::allocate_loopback_port,
        |clash_api_port| {
            let config_path = write_config(profile, input, &geo, routing_fallback, clash_api_port)?;
            if !config_validated {
                check_config(&config_path, cancelled)?;
                config_validated = true;
            }
            if cancelled() {
                anyhow::bail!(START_CANCELLED);
            }
            spawn_and_wait(&config_path, clash_api_port, cancelled)
        },
    )?;
    Ok(StartedSingBox {
        handle,
        routing_fallback,
    })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotReady {
    BinaryBeingWritten,
    TunNotPermitted,
}

#[derive(Debug)]
struct BinaryBeingWritten;

impl std::fmt::Display for BinaryBeingWritten {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the sing-box binary is being written")
    }
}

fn spawn_singbox(command: &mut Command) -> Result<Child> {
    command.spawn().map_err(|error| {
        let being_written = error.raw_os_error() == Some(libc::ETXTBSY);
        let error = anyhow::Error::from(error);
        if being_written {
            error.context(BinaryBeingWritten)
        } else {
            error
        }
    })
}

pub fn not_ready(error: &anyhow::Error) -> Option<NotReady> {
    if error.downcast_ref::<BinaryBeingWritten>().is_some() {
        return Some(NotReady::BinaryBeingWritten);
    }
    mentions_tun_permission_denied(&format!("{error:#}")).then_some(NotReady::TunNotPermitted)
}

fn mentions_tun_permission_denied(error: &str) -> bool {
    error.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        (line.contains("tun interface") || line.contains("/dev/net/tun"))
            && line.contains("operation not permitted")
    })
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
    let mut child = spawn_singbox(
        Command::new(singbox_binary())
            .arg("run")
            .arg("-c")
            .arg(config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped()),
    )
    .with_context(|| format!("Failed to start sing-box (binary: {})", singbox_binary()))?;

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
    use crate::config::profile::{Profile, Settings};
    use crate::config::routing::RoutingConfig;
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    const TEST_CLASH_PORT: u16 = 41390;

    #[test]
    fn singbox_binary_resolution() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("SING_BOX_PATH") };
        assert_eq!(resolve_singbox_binary(), "sing-box");

        unsafe { std::env::set_var("SING_BOX_PATH", "/usr/local/bin/sing-box") };
        assert_eq!(resolve_singbox_binary(), "/usr/local/bin/sing-box");
        unsafe { std::env::remove_var("SING_BOX_PATH") };
    }

    #[test]
    fn write_config_creates_valid_json() {
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
            RoutingInput {
                settings: &settings,
                routing: &RoutingConfig::default(),
            },
            &GeoAvailability::default(),
            None,
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

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn collect_geo_availability_is_empty_when_no_files() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let geo = collect_geo_availability(crate::geo::RULE_SETS);
        assert!(geo.rule_sets.is_empty());
    }

    #[test]
    fn collect_geo_availability_lists_the_usable_required_rule_sets_by_tag() {
        use crate::config::profile::{GeoRegion, RoutingMode};
        use crate::route_plan::RoutePlan;
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = crate::geo::GeoManager::new().unwrap();
        let bypass_ru = RoutePlan::for_mode(
            RoutingMode::Bypass(GeoRegion::Ru),
            &RoutingConfig::default(),
        );
        let required: Vec<_> = bypass_ru.rule_sets().cloned().collect();
        let [geosite, geoip] = &required[..] else {
            unreachable!()
        };
        let [_, geoip_cn] = &crate::test_helpers::region_rule_sets(GeoRegion::Cn)[..] else {
            unreachable!()
        };

        fs::write(gm.rule_set_path(geoip), crate::test_helpers::RULE_SET_BYTES).unwrap();
        fs::write(
            gm.rule_set_path(geoip_cn),
            crate::test_helpers::RULE_SET_BYTES,
        )
        .unwrap();
        let geo = collect_geo_availability(&required);
        assert_eq!(geo.path(geoip), Some(&gm.rule_set_path(geoip)));
        assert_eq!(geo.path(geosite), None);
        assert_eq!(geo.path(geoip_cn), None);
        assert!(geo.lacks_rule_sets_of(&bypass_ru));

        fs::write(
            gm.rule_set_path(geosite),
            crate::test_helpers::RULE_SET_BYTES,
        )
        .unwrap();
        assert!(!collect_geo_availability(&required).lacks_rule_sets_of(&bypass_ru));
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
            RoutingInput {
                settings: &Settings::default(),
                routing: &RoutingConfig::default(),
            },
            &GeoAvailability::default(),
            None,
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

    #[test]
    fn check_config_accepts_a_minimal_profile() {
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
    fn check_config_accepts_hysteria2_gecko() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        let profile = crate::config::profile::parse_share_link(
            "hysteria2://p@1.1.1.1:443?obfs=gecko&obfs-password=ob&minPacketSize=512&maxPacketSize=1200#G",
        )
        .unwrap();
        check_generated_config(&profile).expect("sing-box rejected a gecko hysteria2 profile");
    }

    #[test]
    fn check_config_accepts_hysteria2_port_hopping_interval() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        let mut profile = crate::config::profile::parse_share_link(
            "hysteria2://p@1.1.1.1:443?mport=20000-30000#H",
        )
        .unwrap();
        if let crate::config::profile::ProtocolConfig::Hysteria2(cfg) = &mut profile.config {
            cfg.hop_interval_secs = Some(10);
            cfg.hop_interval_max_secs = Some(20);
        }
        check_generated_config(&profile).expect("sing-box rejected a hop interval range");
    }

    #[test]
    fn check_config_accepts_disable_sni() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        let profile = crate::config::profile::parse_share_link(
            "trojan://pw@1.1.1.1:443?security=tls&sni=cover.example&disable_sni=1#T",
        )
        .unwrap();
        check_generated_config(&profile).expect("sing-box rejected disable_sni");
    }

    #[test]
    fn check_config_accepts_websocket_early_data() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        let profile = crate::config::profile::parse_share_link(
            "vless://00000000-0000-4000-8000-000000000001@1.1.1.1:443?type=ws&security=tls&sni=v.example&path=%2Fws%3Fed%3D2048#S",
        )
        .unwrap();
        check_generated_config(&profile).expect("sing-box rejected websocket early data");
    }

    #[test]
    fn check_config_accepts_a_fragmented_tls_hello() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", runtime.path());
        if !sing_box_on_path() {
            return;
        }
        let fm = urlencoding::encode(
            r#"{"tcp":[{"type":"fragment","settings":{"packets":"tlshello"}}]}"#,
        );
        let profile = crate::config::profile::parse_share_link(&format!(
            "vless://00000000-0000-4000-8000-000000000001@1.1.1.1:443?type=tcp&security=tls&sni=v.example&fm={fm}#V"
        ))
        .unwrap();
        check_generated_config(&profile).expect("sing-box rejected a fragmented TLS hello");
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
                early_data: None,
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
                early_data: None,
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
            early_data: None,
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
    fn only_a_spawn_of_a_binary_still_open_for_writing_reads_as_being_written() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("sing-box");
        let writer = fs::File::create(&binary).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let spawn_failure = |path: &std::path::Path| {
            let error = spawn_singbox(&mut Command::new(path)).unwrap_err();
            not_ready(&error.context("Failed to run sing-box check"))
        };

        assert_eq!(spawn_failure(&binary), Some(NotReady::BinaryBeingWritten));
        drop(writer);
        assert_eq!(spawn_failure(&dir.path().join("missing")), None);

        let write_failure = anyhow::Error::from(std::io::Error::from_raw_os_error(libc::ETXTBSY))
            .context("Failed to write config");
        assert_eq!(not_ready(&write_failure), None);
    }

    #[test]
    fn tun_permission_denial_is_recognized_only_on_a_tun_line() {
        assert!(mentions_tun_permission_denied(
            "sing-box exited immediately (code: Some(1)). stderr: FATAL[0000] start service: start inbound/tun[tun-in]: configure tun interface: operation not permitted"
        ));
        assert!(!mentions_tun_permission_denied(
            "sing-box exited immediately (code: Some(1)). stderr: WARN[0000] set socket mark: operation not permitted\nFATAL[0000] decode config: bad outbound"
        ));
        assert!(!mentions_tun_permission_denied(
            "sing-box config validation failed: FATAL[0000] start inbound/tun[tun-in]: invalid address"
        ));
        assert!(!mentions_tun_permission_denied(
            "sing-box exited immediately (code: Some(1)). stderr: FATAL[0000] start endpoint: open tunnel: operation not permitted"
        ));
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
