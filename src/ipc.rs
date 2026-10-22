use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

use anyhow::Context;

use crate::app::msg::{IpcCommand, Msg, StateSnapshot};

const BROADCAST_WRITE_TIMEOUT: Duration = Duration::from_millis(200);

pub const IPC_VERSION: u32 = 1;

pub fn socket_path() -> anyhow::Result<std::path::PathBuf> {
    let dir = dirs::runtime_dir().ok_or_else(|| {
        anyhow::anyhow!("XDG_RUNTIME_DIR is not set; kvn requires a desktop user session")
    })?;
    Ok(dir.join("kvn-tui.sock"))
}

pub fn cleanup_socket() {
    if let Ok(path) = socket_path() {
        let _ = std::fs::remove_file(path);
    }
}

pub fn is_daemon_running() -> bool {
    socket_path().is_ok_and(|path| UnixStream::connect(path).is_ok())
}

pub fn wait_for_daemon(timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    let mut delay = Duration::from_millis(10);
    let cap = Duration::from_millis(320);
    while start.elapsed() < timeout {
        if is_daemon_running() {
            return true;
        }
        thread::sleep(delay);
        delay = (delay * 2).min(cap);
    }
    is_daemon_running()
}

pub fn wait_for_daemon_exit(timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !is_daemon_running() {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    !is_daemon_running()
}

pub struct IpcServer {
    clients: Arc<Mutex<Vec<UnixStream>>>,
}

impl IpcServer {
    pub fn bind(tx: Sender<Msg>) -> anyhow::Result<Self> {
        let path = socket_path()?;
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                anyhow::bail!("kvn daemon is already running at {}", path.display());
            }
            std::fs::remove_file(&path)
                .with_context(|| format!("Failed to remove stale socket {}", path.display()))?;
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .context("Failed to chmod IPC socket")?;
        let clients: Arc<Mutex<Vec<UnixStream>>> = Arc::new(Mutex::new(Vec::new()));
        let clients_clone = clients.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let writer = match stream.try_clone() {
                            Ok(w) => w,
                            Err(e) => {
                                tracing::warn!("Failed to clone IPC stream for broadcast: {e}");
                                continue;
                            }
                        };
                        if let Err(e) = writer.set_write_timeout(Some(BROADCAST_WRITE_TIMEOUT)) {
                            tracing::warn!("Failed to set IPC write timeout: {e}");
                        }
                        let tx = tx.clone();
                        let clients = clients_clone.clone();
                        clients.lock().unwrap().push(writer);
                        thread::spawn(move || {
                            let reader = BufReader::new(stream);
                            for line in reader.lines() {
                                match line {
                                    Ok(line) => {
                                        if let Ok(mut value) =
                                            serde_json::from_str::<serde_json::Value>(&line)
                                        {
                                            let request_id = value
                                                .as_object_mut()
                                                .and_then(|object| object.remove("request_id"))
                                                .and_then(|value| value.as_str().map(str::to_owned))
                                                .and_then(|value| Uuid::parse_str(&value).ok());
                                            if let Ok(command) =
                                                serde_json::from_value::<IpcCommand>(value)
                                            {
                                                let message = match request_id {
                                                    Some(request_id) => Msg::IpcRequest {
                                                        command,
                                                        request_id,
                                                    },
                                                    None => Msg::IpcCommand(command),
                                                };
                                                let _ = tx.send(message);
                                            }
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self { clients })
    }

    pub fn broadcast(&self, snapshot: &StateSnapshot) {
        let json = match serde_json::to_string(snapshot) {
            Ok(s) => s + "\n",
            Err(e) => {
                tracing::warn!("Failed to serialize state snapshot: {e}");
                return;
            }
        };

        let writers: Vec<(RawFd, UnixStream)> = {
            let guard = self.clients.lock().unwrap();
            guard
                .iter()
                .filter_map(|s| s.try_clone().ok().map(|c| (s.as_raw_fd(), c)))
                .collect()
        };

        let mut dead_fds: HashSet<RawFd> = HashSet::new();
        for (fd, mut client) in writers {
            if let Err(e) = client.write_all(json.as_bytes()) {
                tracing::debug!("Dropping IPC client (fd={fd}): {e}");
                let _ = client.shutdown(Shutdown::Both);
                dead_fds.insert(fd);
            }
        }

        if !dead_fds.is_empty() {
            let mut guard = self.clients.lock().unwrap();
            guard.retain(|s| !dead_fds.contains(&s.as_raw_fd()));
        }
    }
}

#[derive(Debug)]
pub struct DaemonConnectionLost;

impl std::fmt::Display for DaemonConnectionLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lost connection to the kvn daemon")
    }
}

pub struct IpcClient {
    stream: UnixStream,
}

impl IpcClient {
    pub fn connect() -> anyhow::Result<Self> {
        Self::connect_at(&socket_path()?)
    }

    pub fn connect_at(path: &std::path::Path) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(false)?;
        Ok(Self { stream })
    }

    pub fn daemon_executable(&self) -> anyhow::Result<std::path::PathBuf> {
        let pid = self.daemon_pid()?;
        let executable = std::fs::read_link(format!("/proc/{pid}/exe"))
            .with_context(|| format!("Failed to find the executable of daemon process {pid}"))?;
        anyhow::ensure!(
            executable.exists(),
            "the daemon's executable {} was replaced; start kvn again",
            executable.display()
        );
        Ok(executable)
    }

    fn daemon_pid(&self) -> anyhow::Result<libc::pid_t> {
        let mut credentials = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        #[allow(unsafe_code)]
        let result = unsafe {
            libc::getsockopt(
                self.stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut credentials).cast(),
                &raw mut length,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .context("Failed to read the daemon's process credentials");
        }
        Ok(credentials.pid)
    }

    pub fn send(&mut self, cmd: &IpcCommand) -> anyhow::Result<()> {
        self.write_line(serde_json::to_string(cmd)? + "\n")
    }

    pub fn send_request(&mut self, cmd: &IpcCommand) -> anyhow::Result<Uuid> {
        let request_id = Uuid::new_v4();
        let mut value = serde_json::to_value(cmd)?;
        value
            .as_object_mut()
            .context("IPC command did not serialize as an object")?
            .insert("request_id".into(), request_id.to_string().into());
        self.write_line(serde_json::to_string(&value)? + "\n")?;
        Ok(request_id)
    }

    fn write_line(&mut self, line: String) -> anyhow::Result<()> {
        use std::io::ErrorKind;

        self.stream
            .write_all(line.as_bytes())
            .and_then(|()| self.stream.flush())
            .map_err(|error| {
                let connection_lost = matches!(
                    error.kind(),
                    ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::NotConnected
                );
                let error = anyhow::Error::new(error);
                if connection_lost {
                    error.context(DaemonConnectionLost)
                } else {
                    error
                }
            })
    }

    pub fn read_response(
        &mut self,
        request_id: Uuid,
        timeout: Duration,
    ) -> anyhow::Result<StateSnapshot> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .context("Timed out waiting for the daemon to acknowledge the command")?;
            let snapshot = self.read_snapshot(remaining)?;
            if snapshot.response_to == Some(request_id) {
                return Ok(snapshot);
            }
        }
    }

    pub fn read_snapshot(&mut self, timeout: Duration) -> anyhow::Result<StateSnapshot> {
        let value = self.read_snapshot_value(timeout)?;
        serde_json::from_value(value).context("Malformed state snapshot from the daemon")
    }

    pub fn read_snapshot_value(&mut self, timeout: Duration) -> anyhow::Result<serde_json::Value> {
        use std::io::Read;
        self.stream
            .set_read_timeout(Some(timeout))
            .context("Failed to set IPC read timeout")?;
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match self.stream.read(&mut byte) {
                Ok(0) => anyhow::bail!("Daemon closed the connection"),
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => line.push(byte[0]),
                Err(e) => {
                    return Err(anyhow::Error::new(e)
                        .context("Timed out waiting for a state snapshot from the daemon"));
                }
            }
            if line.len() > 16 * 1024 * 1024 {
                anyhow::bail!("State snapshot exceeds 16 MiB");
            }
        }
        serde_json::from_slice(&line).context("Malformed JSON from the daemon")
    }

    pub fn spawn_reader(&self, tx: Sender<Msg>) -> anyhow::Result<()> {
        let stream = self
            .stream
            .try_clone()
            .context("Failed to clone IPC socket for snapshot reader")?;
        stream
            .set_read_timeout(None)
            .context("Failed to clear IPC read timeout")?;
        thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            loop {
                line.clear();
                let complete = reader.read_until(b'\n', &mut line).is_ok() && line.ends_with(b"\n");
                if !complete {
                    let _ = tx.send(Msg::DaemonDisconnected);
                    break;
                }
                match serde_json::from_slice::<StateSnapshot>(&line) {
                    Ok(snapshot) => {
                        let _ = tx.send(Msg::StateUpdate {
                            snapshot: Box::new(snapshot),
                        });
                    }
                    Err(error) => {
                        let _ = tx.send(Msg::IpcReadFailed {
                            message: format!("Malformed state snapshot from the daemon: {error}"),
                        });
                        break;
                    }
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::{ConnectionState, Overlay, TrafficStats};
    use crate::app::msg::StateSnapshot;
    use crate::config::profile::Settings;
    use std::sync::mpsc::channel;
    use std::time::Instant;

    fn drain_one(rx: &std::sync::mpsc::Receiver<Msg>, timeout: Duration) -> Option<Msg> {
        let deadline = Instant::now() + timeout;
        loop {
            match rx.try_recv() {
                Ok(msg) => return Some(msg),
                Err(_) if Instant::now() >= deadline => return None,
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    fn sample_snapshot() -> StateSnapshot {
        StateSnapshot {
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            ipc_version: IPC_VERSION,
            supports_viewport_scroll: true,
            response_to: None,
            config_edit_result: None,
            scroll_result: None,
            connection: ConnectionState::Idle,
            status: "ok".into(),
            status_is_error: false,
            status_revision: 1,
            status_events: Vec::new(),
            singbox_pid: None,
            active_profile_id: None,
            selected: 0,
            routing_selected: 0,
            geo_region_selected: 0,
            dns_selected: 0,
            dns_preset_draft: None,
            dns_fakeip_draft: None,
            theme_draft: None,
            onboarding_awaiting: None,
            onboarding_omarchy: false,
            onboarding_complete: true,
            integration_setup: crate::onboarding::IntegrationSetup::default(),
            settings_menu_selected: 0,
            routing_settings_draft: None,
            connection_settings_draft: None,
            auto_connect_pending: false,
            kill_switch_pending: None,
            interface_settings_draft: None,
            geo_updating: false,
            routing_fallback: None,
            geo_last_updated: None,
            geo_last_checked_at: None,
            service_checked_at: Default::default(),
            overlay: Overlay::None,
            main_pane_focus: Default::default(),
            profiles: vec![],
            subscriptions: vec![],
            settings: Settings::default(),
            traffic: TrafficStats::default(),
            log_session_offsets: None,
            profile_latencies: Default::default(),
            testing_profiles: Default::default(),
        }
    }

    #[test]
    fn ipc_roundtrip_command_and_broadcast() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        cleanup_socket();

        let (server_tx, server_rx) = channel::<Msg>();
        let server = IpcServer::bind(server_tx).expect("server bind");

        let mut client = IpcClient::connect().expect("client connect");
        let (client_tx, client_rx) = channel::<Msg>();
        client.spawn_reader(client_tx).expect("reader spawn");

        client.send(&IpcCommand::Quit).expect("send quit");
        match drain_one(&server_rx, Duration::from_secs(2)) {
            Some(Msg::IpcCommand(IpcCommand::Quit)) => {}
            Some(_) => panic!("expected Msg::IpcCommand(Quit), got a different Msg variant"),
            None => panic!("timed out waiting for Msg::IpcCommand(Quit)"),
        }

        thread::sleep(Duration::from_millis(50));
        server.broadcast(&sample_snapshot());
        match drain_one(&client_rx, Duration::from_secs(2)) {
            Some(Msg::StateUpdate { snapshot: snap }) => {
                assert_eq!(snap.status, "ok");
                assert!(matches!(snap.connection, ConnectionState::Idle));
            }
            Some(_) => panic!("expected Msg::StateUpdate, got a different Msg variant"),
            None => panic!("timed out waiting for Msg::StateUpdate"),
        }

        cleanup_socket();
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    }

    #[test]
    fn request_response_skips_unrelated_broadcasts() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        cleanup_socket();

        let (server_tx, server_rx) = channel::<Msg>();
        let server = IpcServer::bind(server_tx).expect("server bind");
        let mut client = IpcClient::connect().expect("client connect");
        let request_id = client
            .send_request(&IpcCommand::Toggle)
            .expect("send request");

        match drain_one(&server_rx, Duration::from_secs(2)) {
            Some(Msg::IpcRequest {
                command: IpcCommand::Toggle,
                request_id: received,
            }) => assert_eq!(received, request_id),
            Some(_) => panic!("expected correlated toggle request"),
            None => panic!("timed out waiting for correlated toggle request"),
        }

        server.broadcast(&sample_snapshot());
        let mut response = sample_snapshot();
        response.response_to = Some(request_id);
        server.broadcast(&response);

        let received = client
            .read_response(request_id, Duration::from_secs(2))
            .expect("read correlated response");
        assert_eq!(received.response_to, Some(request_id));

        cleanup_socket();
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    }

    #[test]
    fn snapshots_without_editor_results_remain_compatible() {
        let value = serde_json::to_value(sample_snapshot()).unwrap();
        assert!(value.get("config_edit_result").is_none());
        let snapshot: StateSnapshot = serde_json::from_value(value).unwrap();
        assert!(snapshot.config_edit_result.is_none());
    }

    #[test]
    fn a_snapshot_from_a_daemon_without_the_tour_flag_still_parses() {
        let mut value = serde_json::to_value(sample_snapshot()).unwrap();
        value.as_object_mut().unwrap().remove("onboarding_complete");
        assert!(serde_json::from_value::<StateSnapshot>(value).is_ok());
    }

    #[test]
    fn snapshot_reader_clears_handshake_timeout_while_idle() {
        let (client_stream, mut daemon_stream) = UnixStream::pair().unwrap();
        client_stream
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let client = IpcClient {
            stream: client_stream,
        };
        let (tx, rx) = channel();
        client.spawn_reader(tx).unwrap();

        thread::sleep(Duration::from_millis(100));
        serde_json::to_writer(&mut daemon_stream, &sample_snapshot()).unwrap();
        daemon_stream.write_all(b"\n").unwrap();
        daemon_stream.flush().unwrap();

        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Msg::StateUpdate { snapshot }) => assert_eq!(snapshot.status, "ok"),
            Ok(_) => panic!("expected StateUpdate after idle period, got another message"),
            Err(error) => panic!("reader did not receive snapshot after idle period: {error}"),
        }
    }

    #[test]
    fn snapshot_reader_reports_disconnect() {
        let (client_stream, daemon_stream) = UnixStream::pair().unwrap();
        let client = IpcClient {
            stream: client_stream,
        };
        let (tx, rx) = channel();
        client.spawn_reader(tx).unwrap();
        drop(daemon_stream);

        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)),
            Ok(Msg::DaemonDisconnected)
        ));
    }

    #[test]
    fn snapshot_reader_treats_a_line_cut_by_disconnect_as_a_disconnect() {
        let (client_stream, mut daemon_stream) = UnixStream::pair().unwrap();
        let client = IpcClient {
            stream: client_stream,
        };
        let (tx, rx) = channel();
        client.spawn_reader(tx).unwrap();
        daemon_stream.write_all(br#"{"daemon_version":"#).unwrap();
        drop(daemon_stream);

        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)),
            Ok(Msg::DaemonDisconnected)
        ));
    }

    #[test]
    fn snapshot_reader_reports_a_malformed_snapshot() {
        let (client_stream, mut daemon_stream) = UnixStream::pair().unwrap();
        let client = IpcClient {
            stream: client_stream,
        };
        let (tx, rx) = channel();
        client.spawn_reader(tx).unwrap();
        daemon_stream.write_all(b"{}\n").unwrap();

        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)),
            Ok(Msg::IpcReadFailed { message }) if message.contains("Malformed")
        ));
    }

    #[test]
    fn daemon_executable_is_the_binary_of_the_peer_process() {
        let (client_stream, _daemon_stream) = UnixStream::pair().unwrap();
        let client = IpcClient {
            stream: client_stream,
        };

        assert_eq!(
            client.daemon_executable().unwrap(),
            std::env::current_exe().unwrap()
        );
    }

    #[test]
    fn sending_after_the_daemon_closes_the_connection_reports_it_as_lost() {
        let (client_stream, daemon_stream) = UnixStream::pair().unwrap();
        let mut client = IpcClient {
            stream: client_stream,
        };
        daemon_stream.shutdown(Shutdown::Both).unwrap();

        let error = client.send(&IpcCommand::Attach).unwrap_err();
        assert!(error.downcast_ref::<DaemonConnectionLost>().is_some());
    }

    #[test]
    fn socket_path_uses_xdg_runtime_dir_when_set() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        let p = socket_path().unwrap();
        assert!(p.starts_with(tmp.path()));
        assert_eq!(p.file_name().unwrap(), "kvn-tui.sock");
        unsafe {
            match prev {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }

    #[test]
    fn socket_path_requires_xdg_runtime_dir() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::remove("XDG_RUNTIME_DIR");
        let error = socket_path().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requires a desktop user session")
        );
    }

    #[test]
    fn ipc_socket_has_0600_permissions() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        cleanup_socket();

        let (server_tx, _server_rx) = channel::<Msg>();
        let _server = IpcServer::bind(server_tx).expect("server bind");
        let mode = std::fs::metadata(socket_path().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);

        cleanup_socket();
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    }

    #[test]
    fn second_server_does_not_replace_live_socket() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        cleanup_socket();

        let (first_tx, _first_rx) = channel::<Msg>();
        let _first = IpcServer::bind(first_tx).expect("first server bind");
        let (second_tx, _second_rx) = channel::<Msg>();
        let err = match IpcServer::bind(second_tx) {
            Ok(_) => panic!("second server must be rejected"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("already running"));
        assert!(UnixStream::connect(socket_path().unwrap()).is_ok());

        cleanup_socket();
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    }

    #[test]
    fn server_replaces_stale_socket() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        crate::paths::ensure_runtime_dir().unwrap();
        std::fs::write(socket_path().unwrap(), b"stale").unwrap();

        let (server_tx, _server_rx) = channel::<Msg>();
        let _server = IpcServer::bind(server_tx).expect("replace stale socket");

        assert!(UnixStream::connect(socket_path().unwrap()).is_ok());

        cleanup_socket();
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    }

    #[test]
    fn daemons_racing_over_a_stale_socket_leave_one_live_server() {
        use crate::runtime_lock::RuntimeLock;
        use std::sync::{Arc, Barrier};

        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", tmp.path());
        drop(UnixListener::bind(socket_path().unwrap()).unwrap());

        let start = Arc::new(Barrier::new(2));
        let starters: Vec<_> = (0..2)
            .map(|_| {
                let start = start.clone();
                thread::spawn(move || {
                    start.wait();
                    let lock = RuntimeLock::try_acquire_within(
                        &crate::paths::daemon_lock_path().unwrap(),
                        Duration::ZERO,
                    )
                    .unwrap()?;
                    let (tx, rx) = channel::<Msg>();
                    let server = IpcServer::bind(tx).unwrap();
                    Some((lock, server, rx))
                })
            })
            .collect();
        let running: Vec<_> = starters
            .into_iter()
            .filter_map(|starter| starter.join().unwrap())
            .collect();

        assert_eq!(running.len(), 1);
        assert!(UnixStream::connect(socket_path().unwrap()).is_ok());
    }

    #[test]
    fn is_daemon_running_returns_false_when_no_socket() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        assert!(!is_daemon_running());
        unsafe {
            match prev {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }

    #[test]
    fn wait_for_daemon_times_out_without_server() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        let started = std::time::Instant::now();
        assert!(!wait_for_daemon(Duration::from_millis(50)));
        assert!(started.elapsed() >= Duration::from_millis(40));
        unsafe {
            match prev {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }

    #[test]
    fn server_ignores_malformed_command_json() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        cleanup_socket();

        let (server_tx, server_rx) = channel::<Msg>();
        let _server = IpcServer::bind(server_tx).expect("server bind");

        let mut stream = UnixStream::connect(socket_path().unwrap()).expect("client connect");
        stream
            .write_all(b"this is not json\n")
            .expect("write garbage");
        stream.flush().unwrap();

        assert!(drain_one(&server_rx, Duration::from_millis(200)).is_none());

        cleanup_socket();
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    }

    #[test]
    fn broadcast_drops_dead_clients() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", tmp.path()) };
        cleanup_socket();

        let (server_tx, _server_rx) = channel::<Msg>();
        let server = IpcServer::bind(server_tx).expect("server bind");

        {
            let _client = IpcClient::connect().expect("connect");
        }
        thread::sleep(Duration::from_millis(50));

        for _ in 0..5 {
            server.broadcast(&sample_snapshot());
        }
        let live = server.clients.lock().unwrap().len();
        assert_eq!(live, 0, "expected dead client to be reaped");

        cleanup_socket();
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    }

    #[test]
    fn broadcast_disconnects_a_client_that_stops_reading() {
        use std::io::Read;

        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _runtime = crate::test_helpers::EnvVarGuard::set("XDG_RUNTIME_DIR", tmp.path());

        let (server_tx, server_rx) = channel::<Msg>();
        let server = IpcServer::bind(server_tx).expect("server bind");

        let mut stalled = IpcClient::connect().expect("stalled connect");
        stalled.send(&IpcCommand::Attach).unwrap();
        server_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("Attach reaches the daemon");

        let mut large = sample_snapshot();
        large.status = "x".repeat(256 * 1024);
        for _ in 0..20 {
            if server.clients.lock().unwrap().is_empty() {
                break;
            }
            server.broadcast(&large);
        }

        stalled
            .stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stalled
            .stream
            .read_to_end(&mut Vec::new())
            .expect("stalled client reaches EOF");

        cleanup_socket();
    }
}
