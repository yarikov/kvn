use chrono::{DateTime, Local, Timelike};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Style;
use ratatui::{Frame, Terminal};
use std::convert::Infallible;
use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard};
use uuid::Uuid;

use crate::app::effect::Effect;
use crate::app::model::{ConnectionState, Model};
use crate::config::profile::{
    Config, Profile, ProtocolConfig, Subscription, SubscriptionAutoUpdate, VlessConfig,
};

/// Mutex that recovers after a test panics while holding the lock.
///
/// A failed environment-sensitive test must not turn every later test into a
/// `PoisonError`. Returning an infallible `Result` preserves the familiar
/// `ENV_LOCK.lock().unwrap()` call sites while making them poison-tolerant.
pub struct RecoverableMutex(Mutex<()>);

impl RecoverableMutex {
    pub const fn new() -> Self {
        Self(Mutex::new(()))
    }

    pub fn lock(&self) -> Result<MutexGuard<'_, ()>, Infallible> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    }
}

/// Global lock for tests that mutate environment variables.
/// Prevents race conditions when running tests in parallel.
pub static ENV_LOCK: RecoverableMutex = RecoverableMutex::new();

/// Restores an environment variable when an environment-sensitive test ends,
/// including during panic unwinding.
pub struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    pub fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    pub fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Convert a ratatui Buffer to a multi-line string for snapshot testing.
pub fn buffer_to_string(buffer: &Buffer) -> String {
    let rows = buffer
        .content
        .chunks(buffer.area.width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    format!("{rows}\n[{}x{}]", buffer.area.width, buffer.area.height)
}

const STYLE_KEYS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

fn style_label(style: &Style) -> String {
    let mut parts = Vec::new();
    if let Some(fg) = style.fg {
        parts.push(format!("fg={fg:?}"));
    }
    if let Some(bg) = style.bg {
        parts.push(format!("bg={bg:?}"));
    }
    if !style.add_modifier.is_empty() {
        parts.push(format!("{:?}", style.add_modifier));
    }
    if !style.sub_modifier.is_empty() {
        parts.push(format!("-{:?}", style.sub_modifier));
    }
    parts.join(" ")
}

pub fn buffer_to_styled_string(buffer: &Buffer) -> String {
    let width = buffer.area.width as usize;
    let mut seen: Vec<Style> = Vec::new();
    let mut map = String::new();
    for (index, cell) in buffer.content.iter().enumerate() {
        if index > 0 && index % width == 0 {
            map.push('\n');
        }
        let style = cell.style();
        if style == Style::default() {
            map.push(' ');
            continue;
        }
        let key = match seen.iter().position(|known| *known == style) {
            Some(key) => key,
            None => {
                seen.push(style);
                seen.len() - 1
            }
        };
        map.push(STYLE_KEYS.chars().nth(key).unwrap_or('?'));
    }
    let legend = seen
        .iter()
        .enumerate()
        .map(|(key, style)| {
            format!(
                "{} {}",
                STYLE_KEYS.chars().nth(key).unwrap_or('?'),
                style_label(style)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\n--- styles ---\n{map}\n{legend}",
        buffer_to_string(buffer)
    )
}

pub fn render_to_buffer<F>(width: u16, height: u16, draw: F) -> Buffer
where
    F: FnOnce(&mut Frame),
{
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(draw).unwrap();
    terminal.backend().buffer().clone()
}

pub fn render_to_string<F>(width: u16, height: u16, draw: F) -> String
where
    F: FnOnce(&mut Frame),
{
    buffer_to_string(&render_to_buffer(width, height, draw))
}

pub const APP_WINDOW_COLS: u16 = 113;

pub const APP_WINDOW_ROWS: u16 = 35;

/// A canonical valid UUID string for tests that need to pass
/// [`Config::validate`] without caring about the specific value.
pub const TEST_UUID: &str = "11111111-1111-1111-1111-111111111111";

/// Generate a small set of sample profiles for unit tests.
pub fn sample_profiles() -> Vec<Profile> {
    vec![
        Profile::new_vless(
            "A".to_string(),
            "1.1.1.1".to_string(),
            443,
            TEST_UUID.to_string(),
        ),
        Profile::new_vless(
            "B".to_string(),
            "2.2.2.2".to_string(),
            443,
            TEST_UUID.to_string(),
        ),
        Profile::new_vless(
            "C".to_string(),
            "3.3.3.3".to_string(),
            443,
            TEST_UUID.to_string(),
        ),
    ]
}

/// Build a `Model` pre-filled with the given profiles for testing.
pub fn model_with_profiles(profiles: Vec<Profile>) -> Model {
    let config = Config {
        profiles,
        ..Default::default()
    };
    Model::test_new(config)
}

/// Create a simple `KeyEvent` from a character for testing input handlers.
pub fn key(c: char) -> KeyEvent {
    KeyEvent::from(KeyCode::Char(c))
}

pub fn enter() -> KeyEvent {
    KeyEvent::from(KeyCode::Enter)
}

pub fn esc() -> KeyEvent {
    KeyEvent::from(KeyCode::Esc)
}

pub fn connected_model() -> (Model, Uuid) {
    let a = Profile::new_vless("A".into(), "1.1.1.1".into(), 443, "u1".into());
    let a_id = a.id;
    let mut model = model_with_profiles(vec![a]);
    model.connection = ConnectionState::Connected;
    model.active_profile_id = Some(a_id);
    (model, a_id)
}

pub fn after_update_window() -> DateTime<Local> {
    Local::now()
        .with_hour(9)
        .unwrap()
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
}

pub fn app_log_info(message: &str) -> Effect {
    Effect::AppendAppLog {
        level: "INFO".to_string(),
        message: message.to_string(),
    }
}

pub fn app_log_error(message: &str) -> Effect {
    Effect::AppendAppLog {
        level: "ERROR".to_string(),
        message: message.to_string(),
    }
}

pub fn vless_cfg(profile: &Profile) -> &VlessConfig {
    match &profile.config {
        ProtocolConfig::Vless(c) => c,
        other => panic!("expected VLESS, got {:?}", other.protocol()),
    }
}

pub fn subscription_with_hwid(send_hwid: bool, hwid: Option<&str>) -> Subscription {
    Subscription {
        id: Uuid::new_v4(),
        name: "Test subscription".to_string(),
        url: "https://example.com/subscription".to_string(),
        auto_update: SubscriptionAutoUpdate::Off,
        last_updated: None,
        next_auto_update: None,
        retry_state: None,
        send_hwid,
        hwid: hwid.map(str::to_string),
    }
}
