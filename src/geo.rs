use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::config::profile::{GeoRegion, RetryState};

const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_mins(1);
const GEO_BATCH_TIMEOUT: Duration = Duration::from_mins(3);
const RULE_SET_MAGIC: &[u8] = b"SRS";

fn is_rule_set(bytes: &[u8]) -> bool {
    bytes.len() > RULE_SET_MAGIC.len() && bytes.starts_with(RULE_SET_MAGIC)
}

fn rule_set_usable(path: &Path) -> bool {
    let mut header = [0u8; 4];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok()
        && is_rule_set(&header)
}

pub(crate) use crate::config::routing::RuleSetKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuleSet {
    pub(crate) tag: Cow<'static, str>,
    pub(crate) kind: RuleSetKind,
    pub(crate) url: Cow<'static, str>,
}

impl RuleSet {
    pub(crate) const fn builtin(tag: &'static str, kind: RuleSetKind, url: &'static str) -> Self {
        Self {
            tag: Cow::Borrowed(tag),
            kind,
            url: Cow::Borrowed(url),
        }
    }

    pub(crate) fn key(&self) -> String {
        rule_set_key(&self.tag)
    }
}

fn rule_set_key(tag: &str) -> String {
    format!("{tag}.srs")
}

const GEOIP_RU: RuleSet = RuleSet::builtin(
    "geoip-ru",
    RuleSetKind::Geoip,
    "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-ru.srs",
);

const GEOSITE_CATEGORY_RU: RuleSet = RuleSet::builtin(
    "geosite-category-ru",
    RuleSetKind::Geosite,
    "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-category-ru.srs",
);

const GEOIP_CN: RuleSet = RuleSet::builtin(
    "geoip-cn",
    RuleSetKind::Geoip,
    "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-cn.srs",
);

const GEOSITE_CN: RuleSet = RuleSet::builtin(
    "geosite-cn",
    RuleSetKind::Geosite,
    "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-cn.srs",
);

const GEOIP_IR: RuleSet = RuleSet::builtin(
    "geoip-ir",
    RuleSetKind::Geoip,
    "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-ir.srs",
);

const GEOSITE_CATEGORY_IR: RuleSet = RuleSet::builtin(
    "geosite-category-ir",
    RuleSetKind::Geosite,
    "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-category-ir.srs",
);

pub(crate) const RULE_SETS: &[RuleSet] = &[
    GEOIP_RU,
    GEOSITE_CATEGORY_RU,
    GEOIP_CN,
    GEOSITE_CN,
    GEOIP_IR,
    GEOSITE_CATEGORY_IR,
];

pub(crate) fn region_rule_set_tags(region: GeoRegion) -> &'static [&'static str] {
    match region {
        GeoRegion::Global => &[],
        GeoRegion::Ru => &["geosite-category-ru", "geoip-ru"],
        GeoRegion::Cn => &["geosite-cn", "geoip-cn"],
        GeoRegion::Ir => &["geosite-category-ir", "geoip-ir"],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct RuleSetRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checked_at: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry: Option<RetryState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_update: Option<NaiveDate>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct GeoMetadata {
    #[serde(default, skip_serializing_if = "is_zero")]
    schedule_epoch: u64,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    rule_sets: HashMap<String, RuleSetRecord>,
}

impl RuleSetRecord {
    fn describes(&self, rule_set: &RuleSet) -> bool {
        self.url.as_deref().is_none_or(|url| url == rule_set.url)
    }
}

impl GeoMetadata {
    fn record(&self, rule_set: &RuleSet) -> RuleSetRecord {
        self.rule_sets
            .get(&rule_set.key())
            .filter(|record| record.describes(rule_set))
            .cloned()
            .unwrap_or_default()
    }

    fn describes(&self, rule_set: &RuleSet) -> bool {
        self.rule_sets
            .get(&rule_set.key())
            .is_none_or(|record| record.describes(rule_set))
    }

    fn source_record_mut(&mut self, rule_set: &RuleSet) -> Option<&mut RuleSetRecord> {
        let record = self.rule_sets.entry(rule_set.key()).or_default();
        record.describes(rule_set).then_some(record)
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Default, Deserialize)]
struct GeoMetadataRaw {
    #[serde(default)]
    schedule_epoch: u64,
    #[serde(default)]
    rule_sets: HashMap<String, RuleSetRecord>,
    #[serde(default)]
    etags: HashMap<String, String>,
    #[serde(default)]
    checked_at: HashMap<GeoRegion, DateTime<Local>>,
    #[serde(default, alias = "retry_states")]
    region_retry_states: HashMap<GeoRegion, RetryState>,
    #[serde(default)]
    region_next_update: HashMap<GeoRegion, NaiveDate>,
    #[serde(default)]
    geoip_ru_etag: Option<String>,
    #[serde(default)]
    geosite_ru_etag: Option<String>,
    #[serde(default)]
    geoip_cn_etag: Option<String>,
    #[serde(default)]
    geosite_cn_etag: Option<String>,
    #[serde(default)]
    geoip_ir_etag: Option<String>,
    #[serde(default)]
    geosite_ir_etag: Option<String>,
}

impl From<GeoMetadataRaw> for GeoMetadata {
    fn from(raw: GeoMetadataRaw) -> Self {
        let mut rule_sets = raw.rule_sets;
        let legacy_etags = raw.etags.into_iter().chain(
            [
                ("geoip-ru.srs", raw.geoip_ru_etag),
                ("geosite-category-ru.srs", raw.geosite_ru_etag),
                ("geoip-cn.srs", raw.geoip_cn_etag),
                ("geosite-cn.srs", raw.geosite_cn_etag),
                ("geoip-ir.srs", raw.geoip_ir_etag),
                ("geosite-category-ir.srs", raw.geosite_ir_etag),
            ]
            .into_iter()
            .filter_map(|(file, etag)| Some((file.to_string(), etag?))),
        );
        for (file, etag) in legacy_etags {
            rule_sets.entry(file).or_default().etag.get_or_insert(etag);
        }
        for region in GeoRegion::ALL {
            for tag in region_rule_set_tags(region) {
                let record = rule_sets.entry(rule_set_key(tag)).or_default();
                if record.checked_at.is_none() {
                    record.checked_at = raw.checked_at.get(&region).copied();
                }
                if record.retry.is_none() {
                    record.retry = raw.region_retry_states.get(&region).copied();
                }
                if record.next_update.is_none() {
                    record.next_update = raw.region_next_update.get(&region).copied();
                }
            }
        }
        rule_sets.retain(|_, record| *record != RuleSetRecord::default());
        GeoMetadata {
            schedule_epoch: raw.schedule_epoch,
            rule_sets,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSetState {
    pub present: bool,
    pub checked_at: Option<DateTime<Local>>,
    pub retry: Option<RetryState>,
    pub next_update: Option<NaiveDate>,
}

pub type RuleSetStates = HashMap<String, RuleSetState>;

pub fn rule_set_present(states: &RuleSetStates, rule_set: &RuleSet) -> bool {
    states
        .get(&rule_set.key())
        .is_some_and(|state| state.present)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleSetOutcome {
    Updated,
    UpToDate,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSetReport {
    pub key: String,
    pub tag: String,
    pub outcome: RuleSetOutcome,
}

struct RecordedOutcome {
    etag: Option<String>,
    checked: Option<DateTime<Local>>,
    failed: bool,
    already_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleWrite {
    pub interval_days: i64,
    pub epoch: u64,
}

static METADATA_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub struct GeoManager {
    geo_dir: PathBuf,
    metadata_path: PathBuf,
    agent: ureq::Agent,
    deadline: Instant,
}

impl GeoManager {
    pub fn new() -> Result<Self> {
        let geo_dir = crate::paths::geo_dir();

        fs::create_dir_all(&geo_dir)
            .with_context(|| format!("Failed to create geo dir {:?}", geo_dir))?;

        let metadata_path = geo_dir.join("metadata.json");
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_resolve(Some(std::time::Duration::from_secs(15)))
            .timeout_connect(Some(std::time::Duration::from_secs(15)))
            .timeout_recv_response(Some(std::time::Duration::from_secs(30)))
            .build()
            .into();

        Ok(Self {
            geo_dir,
            metadata_path,
            agent,
            deadline: Instant::now() + GEO_BATCH_TIMEOUT,
        })
    }

    pub(crate) fn rule_set_path(&self, rule_set: &RuleSet) -> PathBuf {
        self.geo_dir.join(rule_set.key())
    }

    fn has_current_rule_set(&self, meta: &GeoMetadata, rule_set: &RuleSet) -> bool {
        meta.describes(rule_set) && rule_set_usable(&self.rule_set_path(rule_set))
    }

    pub fn schedule_epoch(&self) -> u64 {
        self.load_metadata().unwrap_or_default().schedule_epoch
    }

    pub fn states(&self, rule_sets: &[RuleSet]) -> RuleSetStates {
        let meta = self.load_metadata().unwrap_or_default();
        rule_sets
            .iter()
            .map(|rule_set| {
                let record = meta.record(rule_set);
                let state = RuleSetState {
                    present: self.has_current_rule_set(&meta, rule_set),
                    checked_at: record.checked_at,
                    retry: record.retry,
                    next_update: record.next_update,
                };
                (rule_set.key(), state)
            })
            .collect()
    }

    pub fn update_rule_sets(
        &self,
        rule_sets: &[RuleSet],
        only_missing: bool,
        schedule: Option<ScheduleWrite>,
    ) -> Vec<RuleSetReport> {
        let saved = self.load_metadata().unwrap_or_default();
        rule_sets
            .iter()
            .map(|rule_set| {
                let key = rule_set.key();
                if let Err(e) = self.adopt_source(rule_set) {
                    tracing::warn!("Failed to record the source of {}: {e:#}", rule_set.tag);
                }
                let already_present = only_missing && self.has_current_rule_set(&saved, rule_set);
                let (outcome, etag) = if already_present {
                    (RuleSetOutcome::UpToDate, None)
                } else {
                    let saved_etag = saved.record(rule_set).etag;
                    self.refresh_rule_set(&saved, rule_set, saved_etag.as_deref())
                };
                let checked = (!already_present && !matches!(outcome, RuleSetOutcome::Failed(_)))
                    .then(Local::now);
                let failed = matches!(outcome, RuleSetOutcome::Failed(_));
                if let Err(e) = self.record_outcome(
                    rule_set,
                    RecordedOutcome {
                        etag,
                        checked,
                        failed,
                        already_present,
                    },
                    schedule,
                ) {
                    tracing::warn!("Failed to record {} update: {e:#}", rule_set.tag);
                }
                RuleSetReport {
                    key,
                    tag: rule_set.tag.to_string(),
                    outcome,
                }
            })
            .collect()
    }

    fn refresh_rule_set(
        &self,
        saved: &GeoMetadata,
        rule_set: &RuleSet,
        saved_etag: Option<&str>,
    ) -> (RuleSetOutcome, Option<String>) {
        match self.rule_set_needs_download(saved, rule_set, saved_etag) {
            Ok(false) => (RuleSetOutcome::UpToDate, None),
            Ok(true) => match self.download_file(&rule_set.url, &self.rule_set_path(rule_set)) {
                Ok(etag) => (RuleSetOutcome::Updated, etag),
                Err(e) => (
                    RuleSetOutcome::Failed(format!(
                        "{:#}",
                        e.context(format!("Failed to download {}", rule_set.key()))
                    )),
                    None,
                ),
            },
            Err(e) => (RuleSetOutcome::Failed(format!("{e:#}")), None),
        }
    }

    fn adopt_source(&self, rule_set: &RuleSet) -> Result<()> {
        self.update_metadata(|meta| {
            let record = meta.rule_sets.entry(rule_set.key()).or_default();
            if record.url.as_deref() == Some(&*rule_set.url) {
                return false;
            }
            if !record.describes(rule_set) {
                *record = RuleSetRecord::default();
                if let Err(e) = fs::remove_file(self.rule_set_path(rule_set))
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!("Failed to remove the stale {}: {e:#}", rule_set.key());
                }
            }
            record.url = Some(rule_set.url.to_string());
            true
        })
    }

    fn record_outcome(
        &self,
        rule_set: &RuleSet,
        outcome: RecordedOutcome,
        schedule: Option<ScheduleWrite>,
    ) -> Result<()> {
        let now = Local::now();
        self.update_metadata(|meta| {
            let schedule = schedule.filter(|write| write.epoch == meta.schedule_epoch);
            if outcome.etag.is_none() && outcome.checked.is_none() && schedule.is_none() {
                return false;
            }
            let Some(record) = meta.source_record_mut(rule_set) else {
                return false;
            };
            if let Some(etag) = outcome.etag {
                record.etag = Some(etag);
            }
            if let Some(checked) = outcome.checked {
                record.checked_at = Some(checked);
            }
            if let Some(schedule) = schedule {
                if outcome.failed {
                    record.retry = Some(RetryState::after_failure(record.retry.as_ref(), now));
                } else {
                    record.retry = None;
                    if !outcome.already_present {
                        record.next_update =
                            Some(now.date_naive() + chrono::Duration::days(schedule.interval_days));
                    }
                }
            }
            true
        })
    }

    pub fn reset_update_schedules(&self, rule_sets: &[RuleSet], enabled: bool) -> Result<u64> {
        let next = crate::config::profile::next_update_window_date(Local::now());
        let mut schedule_epoch = 0;
        self.update_metadata(|meta| {
            meta.schedule_epoch = meta.schedule_epoch.wrapping_add(1);
            schedule_epoch = meta.schedule_epoch;
            for rule_set in rule_sets {
                if let Some(record) = meta.source_record_mut(rule_set) {
                    record.retry = None;
                    record.next_update = enabled.then_some(next);
                }
            }
            true
        })?;
        Ok(schedule_epoch)
    }

    pub fn ensure_update_schedules(
        &self,
        rule_sets: &[RuleSet],
        enabled: bool,
        schedule_epoch: u64,
    ) -> Result<()> {
        if !enabled {
            return Ok(());
        }
        let next = crate::config::profile::next_update_window_date(Local::now());
        self.update_schedule(schedule_epoch, |meta| {
            let mut changed = false;
            for rule_set in rule_sets {
                if let Some(record) = meta.source_record_mut(rule_set)
                    && record.next_update.is_none()
                {
                    record.next_update = Some(next);
                    changed = true;
                }
            }
            changed
        })
    }

    fn load_metadata(&self) -> Result<GeoMetadata> {
        if !self.metadata_path.exists() {
            return Ok(GeoMetadata::default());
        }
        let text = fs::read_to_string(&self.metadata_path)
            .with_context(|| format!("Failed to read {:?}", self.metadata_path))?;
        let raw: GeoMetadataRaw = serde_json::from_str(&text)
            .with_context(|| format!("Failed to parse {:?}", self.metadata_path))?;
        Ok(raw.into())
    }

    fn update_metadata(&self, change: impl FnOnce(&mut GeoMetadata) -> bool) -> Result<()> {
        let _guard = METADATA_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut meta = self.load_metadata().unwrap_or_default();
        if change(&mut meta) {
            self.save_metadata(&meta)?;
        }
        Ok(())
    }

    fn update_schedule(
        &self,
        schedule_epoch: u64,
        change: impl FnOnce(&mut GeoMetadata) -> bool,
    ) -> Result<()> {
        self.update_metadata(|meta| meta.schedule_epoch == schedule_epoch && change(meta))
    }

    fn save_metadata(&self, meta: &GeoMetadata) -> Result<()> {
        let text = serde_json::to_string_pretty(meta)?;
        self.write_atomic(&self.metadata_path, text.as_bytes())?;
        Ok(())
    }

    fn rule_set_needs_download(
        &self,
        saved: &GeoMetadata,
        rule_set: &RuleSet,
        saved_etag: Option<&str>,
    ) -> Result<bool> {
        if !self.has_current_rule_set(saved, rule_set) {
            return Ok(true);
        }
        self.check_single(&rule_set.url, saved_etag)
    }

    fn check_single(&self, url: &str, saved_etag: Option<&str>) -> Result<bool> {
        let deadline = Instant::now() + self.request_timeout()?;
        let resp = crate::http::retry_interrupted(deadline, |remaining| {
            self.agent
                .head(url)
                .config()
                .timeout_global(Some(remaining))
                .build()
                .call()
        })
        .with_context(|| format!("HEAD request failed for {}", url))?;

        if resp.status() != 200 {
            return Ok(true);
        }

        let remote_etag = resp.headers().get("etag").and_then(|v| v.to_str().ok());

        match (saved_etag, remote_etag) {
            (Some(saved), Some(remote)) => Ok(saved != remote),
            (None, _) => Ok(true),
            _ => Ok(true),
        }
    }

    fn download_file(&self, url: &str, dest: &Path) -> Result<Option<String>> {
        let deadline = Instant::now() + self.request_timeout()?;
        let resp = crate::http::retry_interrupted(deadline, |remaining| {
            self.agent
                .get(url)
                .config()
                .timeout_global(Some(remaining))
                .build()
                .call()
        })
        .with_context(|| format!("GET {}", url))?;

        if resp.status() != 200 {
            anyhow::bail!("HTTP {} for {}", resp.status(), url);
        }

        let etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        let bytes = resp
            .into_body()
            .read_to_vec()
            .context("Failed to read response body")?;
        anyhow::ensure!(
            is_rule_set(&bytes),
            "{url} did not return a sing-box rule-set"
        );
        self.write_atomic(dest, &bytes)?;
        Ok(etag)
    }

    fn request_timeout(&self) -> Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("Geo update batch timed out after 180 seconds");
        }
        Ok(remaining.min(HTTP_REQUEST_TIMEOUT))
    }

    fn write_atomic(&self, dest: &Path, data: &[u8]) -> Result<()> {
        crate::atomic_write::write(dest, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_manager() -> (
        std::sync::MutexGuard<'static, ()>,
        tempfile::TempDir,
        GeoManager,
    ) {
        let guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        (guard, dir, gm)
    }

    const RULE_REGIONS: [GeoRegion; 3] = [GeoRegion::Ru, GeoRegion::Cn, GeoRegion::Ir];

    #[test]
    fn each_rule_region_uses_one_geosite_and_one_geoip_rule_set() {
        assert!(region_rule_set_tags(GeoRegion::Global).is_empty());
        let routing = crate::config::routing::RoutingConfig::default();
        for region in RULE_REGIONS {
            let kinds: Vec<RuleSetKind> = region_rule_set_tags(region)
                .iter()
                .map(|tag| routing.rule_set(tag).unwrap().kind)
                .collect();
            assert_eq!(
                kinds,
                [RuleSetKind::Geosite, RuleSetKind::Geoip],
                "{region:?}"
            );
        }
    }

    #[test]
    fn every_rule_set_downloads_the_file_named_after_its_tag() {
        for entry in RULE_SETS {
            assert!(entry.url.starts_with("https://"), "{}", entry.url);
            assert!(entry.url.ends_with(&entry.key()), "{}", entry.url);
        }
    }

    #[test]
    fn request_timeout_is_capped_by_request_and_batch_deadlines() {
        let (_guard, _dir, mut gm) = temp_manager();

        assert!(gm.request_timeout().unwrap() <= HTTP_REQUEST_TIMEOUT);

        gm.deadline = Instant::now() + Duration::from_millis(50);
        assert!(gm.request_timeout().unwrap() <= Duration::from_millis(50));

        gm.deadline = Instant::now() - Duration::from_millis(1);
        assert!(
            gm.request_timeout()
                .unwrap_err()
                .to_string()
                .contains("batch timed out")
        );
    }

    fn spawn_stub_http(
        max_requests: usize,
        handler: impl Fn(&str, &str) -> (u16, Option<String>, Vec<u8>) + Send + 'static,
    ) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..max_requests {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut head = Vec::new();
                let mut buf = [0u8; 2048];
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    head.extend_from_slice(&buf[..n]);
                    if head.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&head);
                let mut parts = text.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let (status, etag, body) = handler(&method, &path);
                let mut resp = format!(
                    "HTTP/1.1 {status} Stub\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                if let Some(e) = etag {
                    resp.push_str(&format!("ETag: {e}\r\n"));
                }
                resp.push_str("\r\n");
                let _ = stream.write_all(resp.as_bytes());
                if method != "HEAD" {
                    let _ = stream.write_all(&body);
                }
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn stub_rule_set(tag: &'static str, kind: RuleSetKind, url: String) -> RuleSet {
        RuleSet {
            tag: Cow::Borrowed(tag),
            kind,
            url: Cow::Owned(url),
        }
    }

    fn stub_rule_sets(base: &str) -> Vec<RuleSet> {
        vec![
            stub_rule_set("geoip-stub", RuleSetKind::Geoip, format!("{base}/geoip")),
            stub_rule_set(
                "geosite-stub",
                RuleSetKind::Geosite,
                format!("{base}/geosite"),
            ),
        ]
    }

    fn unreachable_rule_set(tag: &'static str) -> RuleSet {
        stub_rule_set(
            tag,
            RuleSetKind::Geoip,
            "http://127.0.0.1:1/dead".to_string(),
        )
    }

    fn outcomes(reports: &[RuleSetReport]) -> Vec<&RuleSetOutcome> {
        reports.iter().map(|report| &report.outcome).collect()
    }

    fn state(gm: &GeoManager, rule_set: &RuleSet) -> RuleSetState {
        gm.states(std::slice::from_ref(rule_set))
            .remove(&rule_set.key())
            .unwrap()
    }

    fn record(gm: &GeoManager, rule_set: &RuleSet) -> RuleSetRecord {
        gm.load_metadata()
            .unwrap()
            .rule_sets
            .remove(&rule_set.key())
            .unwrap_or_default()
    }

    fn sourced(rule_set: &RuleSet) -> RuleSetRecord {
        RuleSetRecord {
            url: Some(rule_set.url.to_string()),
            ..RuleSetRecord::default()
        }
    }

    fn schedule(epoch: u64) -> Option<ScheduleWrite> {
        Some(ScheduleWrite {
            interval_days: 7,
            epoch,
        })
    }

    #[test]
    fn each_rule_set_reports_its_own_outcome_and_keeps_its_etag() {
        let (_guard, _dir, gm) = temp_manager();
        let base = spawn_stub_http(4, |_, path| {
            if path == "/geoip" {
                (200, Some("etag-ok".to_string()), b"SRS\x01AAA".to_vec())
            } else {
                (500, None, Vec::new())
            }
        });
        let rule_sets = stub_rule_sets(&base);

        let reports = gm.update_rule_sets(&rule_sets, false, None);

        assert_eq!(reports[0].outcome, RuleSetOutcome::Updated);
        let RuleSetOutcome::Failed(message) = &reports[1].outcome else {
            panic!("{:?}", reports[1]);
        };
        assert!(message.contains("geosite-stub.srs"), "{message}");
        assert_eq!(reports[1].tag, "geosite-stub");
        assert!(state(&gm, &rule_sets[0]).present);
        assert!(!state(&gm, &rule_sets[1]).present);
        assert_eq!(record(&gm, &rule_sets[0]).etag.as_deref(), Some("etag-ok"));
        assert_eq!(record(&gm, &rule_sets[1]), sourced(&rule_sets[1]));
    }

    #[test]
    fn an_unchanged_etag_skips_the_download_and_still_counts_as_a_check() {
        let (_guard, _dir, gm) = temp_manager();
        let base = spawn_stub_http(8, |_, _| {
            (200, Some("same".to_string()), b"SRS\x01DATA".to_vec())
        });
        let rule_sets = stub_rule_sets(&base);

        let first = gm.update_rule_sets(&rule_sets, false, None);
        assert_eq!(
            outcomes(&first),
            [&RuleSetOutcome::Updated, &RuleSetOutcome::Updated]
        );
        assert_eq!(
            fs::read(gm.rule_set_path(&rule_sets[0])).unwrap(),
            b"SRS\x01DATA"
        );

        let second = gm.update_rule_sets(&rule_sets, false, None);
        assert_eq!(
            outcomes(&second),
            [&RuleSetOutcome::UpToDate, &RuleSetOutcome::UpToDate]
        );
        assert!(state(&gm, &rule_sets[1]).checked_at.is_some());
    }

    #[test]
    fn a_failed_check_does_not_starve_a_missing_sibling() {
        let (_guard, _dir, gm) = temp_manager();
        let base = spawn_stub_http(2, |_, _| {
            (200, Some("e1".to_string()), b"SRS\x01NEW".to_vec())
        });
        let dead = unreachable_rule_set("geoip-stub");
        let sibling = stub_rule_set(
            "geosite-stub",
            RuleSetKind::Geosite,
            format!("{base}/geosite"),
        );
        fs::write(gm.rule_set_path(&dead), b"SRS\x01OLD").unwrap();

        let reports = gm.update_rule_sets(&[dead.clone(), sibling.clone()], false, None);

        let RuleSetOutcome::Failed(message) = &reports[0].outcome else {
            panic!("{:?}", reports[0]);
        };
        assert!(message.contains("HEAD request failed"), "{message}");
        assert_eq!(reports[1].outcome, RuleSetOutcome::Updated);
        assert_eq!(record(&gm, &sibling).etag.as_deref(), Some("e1"));
    }

    #[test]
    fn metadata_stays_writable_while_a_rule_set_request_runs() {
        let (_guard, _dir, gm) = temp_manager();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Mutex::new(release_rx);
        let base = spawn_stub_http(2, move |_, _| {
            let _ = entered_tx.send(());
            let _ = release_rx.lock().unwrap().recv();
            (200, Some("etag".to_string()), b"SRS\x01DATA".to_vec())
        });
        let rule_sets = stub_rule_sets(&base);

        std::thread::scope(|scope| {
            let gm = &gm;
            let worker = scope.spawn(move || gm.update_rule_sets(&rule_sets[..1], false, None));
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let (written_tx, written_rx) = std::sync::mpsc::channel();
            scope.spawn(move || written_tx.send(gm.reset_update_schedules(&[], false).is_ok()));
            let written = written_rx.recv_timeout(Duration::from_secs(2));
            drop(release_tx);
            assert_eq!(
                written,
                Ok(true),
                "metadata write waited for a rule-set request"
            );
            assert_eq!(worker.join().unwrap()[0].outcome, RuleSetOutcome::Updated);
        });
    }

    const CAPTIVE_PORTAL_PAGE: &[u8] = b"<html>Sign in to the network</html>";

    #[test]
    fn a_corrupt_cache_is_replaced_despite_an_unchanged_etag() {
        let (_guard, _dir, gm) = temp_manager();
        let base = spawn_stub_http(4, |_, _| {
            (200, Some("same".to_string()), b"SRS\x01NEW".to_vec())
        });
        let rule_sets = stub_rule_sets(&base);
        for rule_set in &rule_sets {
            fs::write(gm.rule_set_path(rule_set), CAPTIVE_PORTAL_PAGE).unwrap();
        }
        gm.update_metadata(|meta| {
            for rule_set in &rule_sets {
                meta.rule_sets.entry(rule_set.key()).or_default().etag = Some("same".into());
            }
            true
        })
        .unwrap();

        let reports = gm.update_rule_sets(&rule_sets, false, None);

        assert_eq!(
            outcomes(&reports),
            [&RuleSetOutcome::Updated, &RuleSetOutcome::Updated]
        );
        for rule_set in &rule_sets {
            assert_eq!(fs::read(gm.rule_set_path(rule_set)).unwrap(), b"SRS\x01NEW");
        }
    }

    #[test]
    fn a_response_without_the_rule_set_header_is_not_written() {
        let (_guard, _dir, gm) = temp_manager();
        let base = spawn_stub_http(2, |_, _| {
            (
                200,
                Some("portal".to_string()),
                CAPTIVE_PORTAL_PAGE.to_vec(),
            )
        });
        let rule_sets = stub_rule_sets(&base);
        let geoip = gm.rule_set_path(&rule_sets[0]);
        fs::write(&geoip, crate::test_helpers::RULE_SET_BYTES).unwrap();

        let reports = gm.update_rule_sets(&rule_sets[..1], false, None);

        let RuleSetOutcome::Failed(message) = &reports[0].outcome else {
            panic!("{:?}", reports[0]);
        };
        assert!(message.contains("geoip-stub.srs"), "{message}");
        assert_eq!(
            fs::read(&geoip).unwrap(),
            crate::test_helpers::RULE_SET_BYTES
        );
        assert_eq!(record(&gm, &rule_sets[0]), sourced(&rule_sets[0]));
    }

    #[test]
    fn only_missing_skips_present_files_without_a_request_and_downloads_the_rest() {
        let (_guard, _dir, gm) = temp_manager();
        let base = spawn_stub_http(2, |_, _| {
            (200, Some("e1".to_string()), b"SRS\x01NEW".to_vec())
        });
        let present = unreachable_rule_set("geoip-present");
        let missing = stub_rule_set(
            "geosite-missing",
            RuleSetKind::Geosite,
            format!("{base}/geosite"),
        );
        fs::write(
            gm.rule_set_path(&present),
            crate::test_helpers::RULE_SET_BYTES,
        )
        .unwrap();
        let epoch = gm.schedule_epoch();

        let reports =
            gm.update_rule_sets(&[present.clone(), missing.clone()], true, schedule(epoch));

        assert_eq!(
            outcomes(&reports),
            [&RuleSetOutcome::UpToDate, &RuleSetOutcome::Updated]
        );
        let present_state = state(&gm, &present);
        assert!(present_state.checked_at.is_none());
        assert!(present_state.next_update.is_none());
        let missing_state = state(&gm, &missing);
        assert!(missing_state.present);
        assert!(missing_state.checked_at.is_some());
        assert_eq!(
            missing_state.next_update,
            Some(Local::now().date_naive() + chrono::Duration::days(7))
        );
    }

    #[test]
    fn a_scheduled_failure_backs_off_each_rule_set_on_its_own() {
        let (_guard, _dir, gm) = temp_manager();
        let first = unreachable_rule_set("geoip-first");
        let second = unreachable_rule_set("geoip-second");

        gm.update_rule_sets(&[first.clone(), second.clone()], false, schedule(0));
        gm.update_rule_sets(std::slice::from_ref(&first), false, schedule(0));

        assert_eq!(state(&gm, &first).retry.unwrap().consecutive_failures, 2);
        assert_eq!(state(&gm, &second).retry.unwrap().consecutive_failures, 1);
        assert!(state(&gm, &first).checked_at.is_none());
    }

    #[test]
    fn a_manual_failure_leaves_the_schedule_alone() {
        let (_guard, _dir, gm) = temp_manager();
        let dead = unreachable_rule_set("geoip-dead");

        gm.update_rule_sets(std::slice::from_ref(&dead), false, None);

        assert_eq!(record(&gm, &dead), sourced(&dead));
    }

    #[test]
    fn schedule_writes_started_before_a_reset_are_ignored() {
        let (_guard, _dir, gm) = temp_manager();
        let dead = unreachable_rule_set("geoip-dead");
        let stale = gm.schedule_epoch();
        let current = gm
            .reset_update_schedules(std::slice::from_ref(&dead), true)
            .unwrap();
        let reset_next = state(&gm, &dead).next_update;
        assert!(reset_next.is_some());

        gm.update_rule_sets(std::slice::from_ref(&dead), false, schedule(stale));
        assert!(state(&gm, &dead).retry.is_none());
        assert_eq!(state(&gm, &dead).next_update, reset_next);

        gm.update_rule_sets(std::slice::from_ref(&dead), false, schedule(current));
        assert!(state(&gm, &dead).retry.is_some());
    }

    #[test]
    fn schedule_reset_clears_retries_and_follows_the_update_setting() {
        let (_guard, _dir, gm) = temp_manager();
        let dead = unreachable_rule_set("geoip-dead");
        gm.update_rule_sets(std::slice::from_ref(&dead), false, schedule(0));

        gm.reset_update_schedules(std::slice::from_ref(&dead), true)
            .unwrap();
        let reset = state(&gm, &dead);
        assert!(reset.retry.is_none());
        assert_eq!(
            reset.next_update,
            Some(crate::config::profile::next_update_window_date(Local::now()))
        );

        gm.reset_update_schedules(std::slice::from_ref(&dead), false)
            .unwrap();
        assert!(state(&gm, &dead).next_update.is_none());
    }

    #[test]
    fn schedule_seeding_fills_only_missing_dates_and_ignores_a_stale_epoch() {
        let (_guard, _dir, gm) = temp_manager();
        let seeded = unreachable_rule_set("geoip-seeded");
        let fresh = unreachable_rule_set("geoip-fresh");
        let stale = gm.schedule_epoch();
        let current = gm
            .reset_update_schedules(std::slice::from_ref(&seeded), false)
            .unwrap();
        gm.update_metadata(|meta| {
            meta.rule_sets.entry(seeded.key()).or_default().next_update =
                NaiveDate::from_ymd_opt(2030, 1, 1);
            true
        })
        .unwrap();

        gm.ensure_update_schedules(&[seeded.clone(), fresh.clone()], true, stale)
            .unwrap();
        assert!(state(&gm, &fresh).next_update.is_none());

        gm.ensure_update_schedules(&[seeded.clone(), fresh.clone()], true, current)
            .unwrap();
        assert_eq!(
            state(&gm, &seeded).next_update,
            NaiveDate::from_ymd_opt(2030, 1, 1)
        );
        assert!(state(&gm, &fresh).next_update.is_some());
    }

    #[test]
    fn a_changed_url_turns_the_file_and_its_record_stale() {
        let (_guard, _dir, gm) = temp_manager();
        let old = stub_rule_set(
            "geoip-moved",
            RuleSetKind::Geoip,
            "https://old.example/geoip.srs".to_string(),
        );
        let moved = unreachable_rule_set("geoip-moved");
        fs::write(gm.rule_set_path(&old), crate::test_helpers::RULE_SET_BYTES).unwrap();
        gm.update_metadata(|meta| {
            let record = meta.rule_sets.entry(old.key()).or_default();
            record.url = Some(old.url.to_string());
            record.etag = Some("old".into());
            record.checked_at = Some(Local::now());
            true
        })
        .unwrap();
        assert!(state(&gm, &old).present);

        assert_eq!(state(&gm, &moved), RuleSetState::default());
        assert!(!state(&gm, &moved).present);

        gm.update_rule_sets(std::slice::from_ref(&moved), true, None);

        assert!(!gm.rule_set_path(&moved).exists());
        assert_eq!(record(&gm, &moved), sourced(&moved));
    }

    #[test]
    fn a_changed_url_downloads_even_when_only_missing_files_are_wanted() {
        let (_guard, _dir, gm) = temp_manager();
        let base = spawn_stub_http(1, |_, _| {
            (200, Some("new".to_string()), b"SRS\x01NEW".to_vec())
        });
        let moved = stub_rule_set("geoip-moved", RuleSetKind::Geoip, format!("{base}/geoip"));
        fs::write(
            gm.rule_set_path(&moved),
            crate::test_helpers::RULE_SET_BYTES,
        )
        .unwrap();
        gm.update_metadata(|meta| {
            meta.rule_sets.entry(moved.key()).or_default().url =
                Some("https://old.example/geoip.srs".into());
            true
        })
        .unwrap();

        let reports = gm.update_rule_sets(std::slice::from_ref(&moved), true, None);

        assert_eq!(outcomes(&reports), [&RuleSetOutcome::Updated]);
        assert_eq!(fs::read(gm.rule_set_path(&moved)).unwrap(), b"SRS\x01NEW");
        assert_eq!(record(&gm, &moved).url.as_deref(), Some(&*moved.url));
        assert_eq!(record(&gm, &moved).etag.as_deref(), Some("new"));
        assert!(state(&gm, &moved).present);
    }

    #[test]
    fn a_record_without_a_url_belongs_to_the_current_source() {
        let (_guard, _dir, gm) = temp_manager();
        let current = unreachable_rule_set("geoip-current");
        let checked_at = Local::now();
        fs::write(
            gm.rule_set_path(&current),
            crate::test_helpers::RULE_SET_BYTES,
        )
        .unwrap();
        gm.update_metadata(|meta| {
            meta.rule_sets.entry(current.key()).or_default().checked_at = Some(checked_at);
            true
        })
        .unwrap();

        let shown = state(&gm, &current);
        assert!(shown.present);
        assert_eq!(shown.checked_at, Some(checked_at));

        gm.ensure_update_schedules(std::slice::from_ref(&current), true, gm.schedule_epoch())
            .unwrap();
        assert!(state(&gm, &current).next_update.is_some());
        assert!(gm.rule_set_path(&current).exists());
    }

    #[test]
    fn schedule_writes_leave_a_file_of_another_source_alone() {
        let (_guard, _dir, gm) = temp_manager();
        let mirrored = stub_rule_set(
            "geoip-mirrored",
            RuleSetKind::Geoip,
            "https://mirror.example/geoip.srs".to_string(),
        );
        let builtin = unreachable_rule_set("geoip-mirrored");
        fs::write(
            gm.rule_set_path(&mirrored),
            crate::test_helpers::RULE_SET_BYTES,
        )
        .unwrap();
        gm.update_metadata(|meta| {
            meta.rule_sets.entry(mirrored.key()).or_default().url = Some(mirrored.url.to_string());
            true
        })
        .unwrap();

        let epoch = gm
            .reset_update_schedules(std::slice::from_ref(&builtin), true)
            .unwrap();
        gm.ensure_update_schedules(std::slice::from_ref(&builtin), true, epoch)
            .unwrap();

        assert!(gm.rule_set_path(&mirrored).exists());
        assert_eq!(record(&gm, &mirrored), sourced(&mirrored));
    }

    #[test]
    fn states_report_presence_only_for_a_file_with_the_rule_set_header() {
        let (_guard, _dir, gm) = temp_manager();
        let routing = crate::config::routing::RoutingConfig::default();
        let geosite = routing.rule_set("geosite-category-ru").unwrap();
        let geoip = routing.rule_set("geoip-ru").unwrap();
        fs::write(
            gm.rule_set_path(&geoip),
            crate::test_helpers::RULE_SET_BYTES,
        )
        .unwrap();
        fs::write(gm.rule_set_path(&geosite), b"SRS").unwrap();

        assert!(state(&gm, &geoip).present);
        assert!(!state(&gm, &geosite).present);
    }

    #[test]
    fn metadata_round_trips_by_rule_set() {
        let (_guard, _dir, gm) = temp_manager();
        let mut meta = GeoMetadata {
            schedule_epoch: 4,
            ..GeoMetadata::default()
        };
        meta.rule_sets.insert(
            "geoip-ru.srs".into(),
            RuleSetRecord {
                url: Some("https://example.com/geoip-ru.srs".into()),
                etag: Some("e".into()),
                checked_at: Some(Local::now()),
                retry: None,
                next_update: NaiveDate::from_ymd_opt(2030, 1, 1),
            },
        );
        gm.save_metadata(&meta).unwrap();

        let loaded = gm.load_metadata().unwrap();
        assert_eq!(loaded.schedule_epoch, 4);
        assert_eq!(loaded.rule_sets, meta.rule_sets);
    }

    #[test]
    fn metadata_moves_region_records_to_the_rule_sets_of_each_region() {
        let (_guard, _dir, gm) = temp_manager();
        let legacy = r#"{
            "etags": { "geoip-ru.srs": "modern", "geosite-cn.srs": "cn-site" },
            "geoip_ru_etag": "legacy",
            "geosite_ru_etag": "legacy-ru-site",
            "updated_at": { "ru": "2026-10-01T08:00:00+03:00" },
            "checked_at": { "ru": "2026-10-02T08:00:00+03:00" },
            "retry_states": {
                "cn": { "consecutive_failures": 2, "retry_at": "2026-10-02T09:00:00+03:00", "attempt_date": "2026-10-02" }
            },
            "region_next_update": { "ru": "2026-10-09" },
            "schedule_epoch": 3
        }"#;
        fs::write(&gm.metadata_path, legacy).unwrap();

        let loaded = gm.load_metadata().unwrap();

        let ru_ip = &loaded.rule_sets["geoip-ru.srs"];
        assert_eq!(ru_ip.etag.as_deref(), Some("modern"));
        assert_eq!(
            ru_ip.checked_at,
            DateTime::parse_from_rfc3339("2026-10-02T08:00:00+03:00")
                .ok()
                .map(|time| time.with_timezone(&Local))
        );
        assert_eq!(ru_ip.next_update, NaiveDate::from_ymd_opt(2026, 10, 9));
        assert_eq!(
            loaded.rule_sets["geosite-category-ru.srs"].etag.as_deref(),
            Some("legacy-ru-site")
        );
        let cn_site = &loaded.rule_sets["geosite-cn.srs"];
        assert_eq!(cn_site.etag.as_deref(), Some("cn-site"));
        assert_eq!(cn_site.retry.unwrap().consecutive_failures, 2);
        assert_eq!(loaded.rule_sets["geoip-cn.srs"].retry, cn_site.retry);
        assert!(!loaded.rule_sets.contains_key("geoip-ir.srs"));
        assert_eq!(loaded.schedule_epoch, 3);
    }

    #[test]
    fn load_metadata_missing_returns_default() {
        let (_guard, _dir, gm) = temp_manager();
        let _ = fs::remove_file(&gm.metadata_path);
        let meta = gm.load_metadata().unwrap();
        assert!(meta.rule_sets.is_empty());
        assert_eq!(meta.schedule_epoch, 0);
    }

    #[test]
    fn write_atomic_creates_file() {
        let (_guard, _dir, gm) = temp_manager();
        let dest = gm.geo_dir.join("test_atomic.txt");
        gm.write_atomic(&dest, b"hello world").unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "hello world");
    }

    #[test]
    fn write_atomic_preserves_srs_extension() {
        let (_guard, _dir, gm) = temp_manager();
        let dest = gm.geo_dir.join("geoip-ru.srs");
        gm.write_atomic(&dest, b"data").unwrap();
        assert!(dest.exists());
        assert!(!gm.geo_dir.join("geoip-ru.srs.tmp").exists());
    }

    #[test]
    fn load_metadata_parse_error_is_propagated() {
        let (_guard, _dir, gm) = temp_manager();
        fs::write(&gm.metadata_path, b"not valid json {{").unwrap();
        let err = gm.load_metadata().unwrap_err().to_string();
        assert!(err.contains("Failed to parse"));
    }
}
