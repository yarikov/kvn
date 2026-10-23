use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::app::msg::GeoSchedule;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuleSetKind {
    Geoip,
    Geosite,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RuleSet {
    pub(crate) tag: &'static str,
    pub(crate) kind: RuleSetKind,
    pub(crate) url: &'static str,
}

impl RuleSet {
    pub(crate) fn filename(&self) -> String {
        format!("{}.srs", self.tag)
    }
}

const GEOIP_RU: RuleSet = RuleSet {
    tag: "geoip-ru",
    kind: RuleSetKind::Geoip,
    url: "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-ru.srs",
};

const GEOSITE_CATEGORY_RU: RuleSet = RuleSet {
    tag: "geosite-category-ru",
    kind: RuleSetKind::Geosite,
    url: "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-category-ru.srs",
};

const GEOIP_CN: RuleSet = RuleSet {
    tag: "geoip-cn",
    kind: RuleSetKind::Geoip,
    url: "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-cn.srs",
};

const GEOSITE_CN: RuleSet = RuleSet {
    tag: "geosite-cn",
    kind: RuleSetKind::Geosite,
    url: "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-cn.srs",
};

const GEOIP_IR: RuleSet = RuleSet {
    tag: "geoip-ir",
    kind: RuleSetKind::Geoip,
    url: "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-ir.srs",
};

const GEOSITE_CATEGORY_IR: RuleSet = RuleSet {
    tag: "geosite-category-ir",
    kind: RuleSetKind::Geosite,
    url: "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-category-ir.srs",
};

pub(crate) const RULE_SETS: &[&RuleSet] = &[
    &GEOIP_RU,
    &GEOSITE_CATEGORY_RU,
    &GEOIP_CN,
    &GEOSITE_CN,
    &GEOIP_IR,
    &GEOSITE_CATEGORY_IR,
];

pub(crate) fn region_rule_sets(region: GeoRegion) -> &'static [&'static RuleSet] {
    match region {
        GeoRegion::Global => &[],
        GeoRegion::Ru => &[&GEOSITE_CATEGORY_RU, &GEOIP_RU],
        GeoRegion::Cn => &[&GEOSITE_CN, &GEOIP_CN],
        GeoRegion::Ir => &[&GEOSITE_CATEGORY_IR, &GEOIP_IR],
    }
}

pub(crate) fn rule_set(tag: &str) -> Option<&'static RuleSet> {
    RULE_SETS
        .iter()
        .copied()
        .find(|rule_set| rule_set.tag == tag)
}

pub(crate) fn is_ip_rule_set_tag(tag: &str) -> bool {
    rule_set(tag).is_some_and(|rule_set| rule_set.kind == RuleSetKind::Geoip)
}

#[derive(Debug, Clone, Default, Serialize)]
struct GeoMetadata {
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    etags: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    updated_at: HashMap<GeoRegion, DateTime<Local>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    checked_at: HashMap<GeoRegion, DateTime<Local>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    region_retry_states: HashMap<GeoRegion, RetryState>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    region_next_update: HashMap<GeoRegion, NaiveDate>,
    #[serde(default, skip_serializing_if = "is_zero")]
    schedule_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionUpdate {
    Updated {
        parts: Vec<String>,
        checked_at: DateTime<Local>,
    },
    UpToDate {
        checked_at: Option<DateTime<Local>>,
    },
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Default, Deserialize)]
struct GeoMetadataRaw {
    #[serde(default)]
    etags: HashMap<String, String>,
    #[serde(default)]
    updated_at: HashMap<GeoRegion, DateTime<Local>>,
    #[serde(default)]
    checked_at: HashMap<GeoRegion, DateTime<Local>>,
    #[serde(default, alias = "retry_states")]
    region_retry_states: HashMap<GeoRegion, RetryState>,
    #[serde(default)]
    region_next_update: HashMap<GeoRegion, NaiveDate>,
    #[serde(default)]
    schedule_epoch: u64,
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
        let mut etags = raw.etags;
        let mut promote = |filename: &str, value: Option<String>| {
            if let Some(v) = value {
                etags.entry(filename.to_string()).or_insert(v);
            }
        };
        promote("geoip-ru.srs", raw.geoip_ru_etag);
        promote("geosite-category-ru.srs", raw.geosite_ru_etag);
        promote("geoip-cn.srs", raw.geoip_cn_etag);
        promote("geosite-cn.srs", raw.geosite_cn_etag);
        promote("geoip-ir.srs", raw.geoip_ir_etag);
        promote("geosite-category-ir.srs", raw.geosite_ir_etag);
        GeoMetadata {
            etags,
            updated_at: raw.updated_at,
            checked_at: raw.checked_at,
            region_retry_states: raw.region_retry_states,
            region_next_update: raw.region_next_update,
            schedule_epoch: raw.schedule_epoch,
        }
    }
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
        self.geo_dir.join(rule_set.filename())
    }

    pub(crate) fn has_rule_set(&self, rule_set: &RuleSet) -> bool {
        rule_set_usable(&self.rule_set_path(rule_set))
    }

    pub fn has_databases(&self, region: GeoRegion) -> bool {
        region_rule_sets(region)
            .iter()
            .all(|rule_set| self.has_rule_set(rule_set))
    }

    fn update_rule_sets_if_needed(&self, rule_sets: &[&RuleSet]) -> Result<bool> {
        let saved_etags = self.load_metadata().unwrap_or_default().etags;
        let mut downloaded_etags = Vec::new();
        let mut updated = false;
        let mut first_err: Option<anyhow::Error> = None;
        for rule_set in rule_sets {
            let filename = rule_set.filename();
            let needed = match self
                .rule_set_needs_download(rule_set, saved_etags.get(&filename).map(String::as_str))
            {
                Ok(n) => n,
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                    continue;
                }
            };
            if !needed {
                continue;
            }
            match self.download_file(rule_set.url, &self.rule_set_path(rule_set)) {
                Ok(etag) => {
                    if let Some(e) = etag {
                        downloaded_etags.push((filename, e));
                    }
                    updated = true;
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e.context(format!("Failed to download {filename}")));
                    }
                }
            }
        }
        if !downloaded_etags.is_empty() {
            self.update_metadata(|meta| {
                meta.etags.extend(downloaded_etags);
                true
            })?;
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(updated),
        }
    }

    pub fn last_updated(&self, region: GeoRegion) -> Option<String> {
        if matches!(region, GeoRegion::Global) {
            return None;
        }
        let meta = self.load_metadata().ok()?;
        meta.checked_at
            .get(&region)
            .map(|dt| dt.format("%d %b %H:%M").to_string())
    }

    pub fn last_checked_at(&self, region: GeoRegion) -> Option<DateTime<Local>> {
        if matches!(region, GeoRegion::Global) {
            return None;
        }
        self.load_metadata().ok()?.checked_at.get(&region).copied()
    }

    pub fn retry_state(&self, region: GeoRegion) -> Option<RetryState> {
        self.load_metadata()
            .ok()?
            .region_retry_states
            .get(&region)
            .copied()
    }

    pub fn schedule_epoch(&self) -> u64 {
        self.load_metadata().unwrap_or_default().schedule_epoch
    }

    pub fn record_update_failure(
        &self,
        region: GeoRegion,
        schedule_epoch: u64,
    ) -> Result<Option<RetryState>> {
        self.record_update_failure_at(region, Local::now(), schedule_epoch)
    }

    fn record_update_failure_at(
        &self,
        region: GeoRegion,
        now: DateTime<Local>,
        schedule_epoch: u64,
    ) -> Result<Option<RetryState>> {
        let mut recorded = None;
        self.update_schedule(schedule_epoch, |meta| {
            let state = RetryState::after_failure(meta.region_retry_states.get(&region), now);
            meta.region_retry_states.insert(region, state);
            recorded = Some(state);
            true
        })?;
        Ok(recorded)
    }

    pub fn clear_retry_state(&self, region: GeoRegion) -> Result<()> {
        self.update_metadata(|meta| meta.region_retry_states.remove(&region).is_some())
    }

    pub fn region_next_update(&self, region: GeoRegion) -> Option<NaiveDate> {
        self.load_metadata()
            .ok()?
            .region_next_update
            .get(&region)
            .copied()
    }

    pub fn reset_update_schedules(&self, region: GeoRegion, enabled: bool) -> Result<u64> {
        let mut schedule_epoch = 0;
        self.update_metadata(|meta| {
            meta.schedule_epoch = meta.schedule_epoch.wrapping_add(1);
            schedule_epoch = meta.schedule_epoch;
            meta.region_retry_states.remove(&region);
            if enabled && region != GeoRegion::Global {
                let next = crate::config::profile::next_update_window_date(Local::now());
                meta.region_next_update.insert(region, next);
            } else {
                meta.region_next_update.remove(&region);
            }
            true
        })?;
        Ok(schedule_epoch)
    }

    pub fn schedule(&self, region: GeoRegion, epoch: u64) -> GeoSchedule {
        let meta = self.load_metadata().unwrap_or_default();
        GeoSchedule {
            epoch,
            retry_state: meta.region_retry_states.get(&region).copied(),
            next_update: meta.region_next_update.get(&region).copied(),
        }
    }

    pub fn ensure_update_schedule(
        &self,
        region: GeoRegion,
        enabled: bool,
        schedule_epoch: u64,
    ) -> Result<()> {
        if !enabled || region == GeoRegion::Global {
            return Ok(());
        }
        let next = crate::config::profile::next_update_window_date(Local::now());
        self.update_schedule(schedule_epoch, |meta| {
            if meta.region_next_update.contains_key(&region) {
                return false;
            }
            meta.region_next_update.insert(region, next);
            true
        })?;
        Ok(())
    }

    pub fn record_region_schedule_success(
        &self,
        region: GeoRegion,
        interval_days: i64,
        schedule_epoch: u64,
    ) -> Result<()> {
        self.update_schedule(schedule_epoch, |meta| {
            meta.region_retry_states.remove(&region);
            meta.region_next_update.insert(
                region,
                Local::now().date_naive() + chrono::Duration::days(interval_days),
            );
            true
        })?;
        Ok(())
    }

    fn outdated_rule_sets(&self, rule_sets: &[&'static RuleSet]) -> Result<Vec<&'static RuleSet>> {
        let meta = self.load_metadata().unwrap_or_default();
        let mut outdated = Vec::new();
        for rule_set in rule_sets {
            let saved_etag = meta.etags.get(&rule_set.filename()).map(String::as_str);
            if self.rule_set_needs_download(rule_set, saved_etag)? {
                outdated.push(*rule_set);
            }
        }
        Ok(outdated)
    }

    #[cfg(test)]
    pub fn download_databases(&self, region: GeoRegion) -> Result<bool> {
        self.download_region_rule_sets(region, region_rule_sets(region))
    }

    fn download_region_rule_sets(&self, region: GeoRegion, rule_sets: &[&RuleSet]) -> Result<bool> {
        let updated = self.update_rule_sets_if_needed(rule_sets)?;
        if updated {
            self.update_metadata(|meta| {
                let now = Local::now();
                meta.updated_at.insert(region, now);
                meta.checked_at.insert(region, now);
                true
            })?;
        }
        Ok(updated)
    }

    pub fn update_if_needed(&self, region: GeoRegion) -> Result<RegionUpdate> {
        let rule_sets = region_rule_sets(region);
        if rule_sets.is_empty() {
            return Ok(RegionUpdate::UpToDate { checked_at: None });
        }
        self.update_region_rule_sets(region, rule_sets)
    }

    fn update_region_rule_sets(
        &self,
        region: GeoRegion,
        rule_sets: &[&'static RuleSet],
    ) -> Result<RegionUpdate> {
        let outdated = self.outdated_rule_sets(rule_sets)?;

        if outdated.is_empty() {
            let checked_at = Local::now();
            self.update_metadata(|meta| {
                meta.checked_at.insert(region, checked_at);
                true
            })?;
            return Ok(RegionUpdate::UpToDate {
                checked_at: Some(checked_at),
            });
        }

        let updated = self.download_region_rule_sets(region, rule_sets)?;
        if updated {
            Ok(RegionUpdate::Updated {
                parts: outdated
                    .iter()
                    .map(|rule_set| rule_set.tag.to_string())
                    .collect(),
                checked_at: self.last_checked_at(region).unwrap_or_else(Local::now),
            })
        } else {
            Ok(RegionUpdate::UpToDate {
                checked_at: Some(Local::now()),
            })
        }
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
        rule_set: &RuleSet,
        saved_etag: Option<&str>,
    ) -> Result<bool> {
        if !self.has_rule_set(rule_set) {
            return Ok(true);
        }
        self.check_single(rule_set.url, saved_etag)
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
    use chrono::Timelike;

    #[test]
    fn schedule_writes_started_before_a_reset_are_ignored() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let stale = gm.schedule_epoch();
        let current = gm.reset_update_schedules(GeoRegion::Ru, true).unwrap();
        let region_next = gm.region_next_update(GeoRegion::Ru);

        gm.record_region_schedule_success(GeoRegion::Ru, 7, stale)
            .unwrap();
        assert_eq!(
            gm.record_update_failure(GeoRegion::Ru, stale).unwrap(),
            None
        );

        assert_ne!(stale, current);
        assert_eq!(gm.region_next_update(GeoRegion::Ru), region_next);
        assert!(gm.retry_state(GeoRegion::Ru).is_none());
        assert!(
            gm.record_update_failure(GeoRegion::Ru, current)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn schedule_seeding_started_before_disabling_updates_is_ignored() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let stale = gm.schedule_epoch();
        gm.reset_update_schedules(GeoRegion::Ru, false).unwrap();

        gm.ensure_update_schedule(GeoRegion::Ru, true, stale)
            .unwrap();

        assert!(gm.region_next_update(GeoRegion::Ru).is_none());
    }

    fn after_update_window() -> DateTime<Local> {
        Local::now()
            .with_hour(9)
            .unwrap()
            .with_minute(0)
            .unwrap()
            .with_second(0)
            .unwrap()
    }

    const RULE_REGIONS: [GeoRegion; 3] = [GeoRegion::Ru, GeoRegion::Cn, GeoRegion::Ir];

    #[test]
    fn each_rule_region_uses_one_geosite_and_one_geoip_rule_set() {
        assert!(region_rule_sets(GeoRegion::Global).is_empty());
        for region in RULE_REGIONS {
            let kinds: Vec<RuleSetKind> = region_rule_sets(region)
                .iter()
                .map(|rule_set| rule_set.kind)
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
            assert!(entry.url.ends_with(&entry.filename()), "{}", entry.url);
            assert_eq!(rule_set(entry.tag), Some(*entry));
        }
    }

    #[test]
    fn request_timeout_is_capped_by_request_and_batch_deadlines() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let mut gm = GeoManager::new().unwrap();

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

    #[test]
    fn ip_rule_set_tags_are_the_geoip_rule_sets() {
        for tag in ["geoip-ru", "geoip-cn", "geoip-ir"] {
            assert!(is_ip_rule_set_tag(tag), "{tag}");
        }
        for tag in ["geosite-category-ru", "geosite-cn", "geoip-unknown"] {
            assert!(!is_ip_rule_set_tag(tag), "{tag}");
        }
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

    fn stub_rule_set(tag: &'static str, kind: RuleSetKind, url: String) -> &'static RuleSet {
        Box::leak(Box::new(RuleSet {
            tag,
            kind,
            url: Box::leak(url.into_boxed_str()),
        }))
    }

    fn stub_rule_sets(base: &str) -> Vec<&'static RuleSet> {
        vec![
            stub_rule_set("geoip-stub", RuleSetKind::Geoip, format!("{base}/geoip")),
            stub_rule_set(
                "geosite-stub",
                RuleSetKind::Geosite,
                format!("{base}/geosite"),
            ),
        ]
    }

    #[test]
    fn update_assets_partial_failure_persists_successful_etag() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let base = spawn_stub_http(4, |_, path| {
            if path == "/geoip" {
                (200, Some("etag-ok".to_string()), b"SRS\x01AAA".to_vec())
            } else {
                (500, None, Vec::new())
            }
        });
        let rule_sets = stub_rule_sets(&base);

        let err = gm
            .update_rule_sets_if_needed(&rule_sets)
            .unwrap_err()
            .to_string();
        assert!(err.contains("geosite-stub.srs"), "{err}");
        assert!(gm.geo_dir.join("geoip-stub.srs").exists());
        assert!(!gm.geo_dir.join("geosite-stub.srs").exists());
        let meta = gm.load_metadata().unwrap();
        assert_eq!(
            meta.etags.get("geoip-stub.srs").map(String::as_str),
            Some("etag-ok")
        );
        assert!(!meta.etags.contains_key("geosite-stub.srs"));
    }

    #[test]
    fn update_assets_downloads_both_when_missing_then_skips_via_etag() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let base = spawn_stub_http(8, |_, _| {
            (200, Some("same".to_string()), b"SRS\x01DATA".to_vec())
        });
        let rule_sets = stub_rule_sets(&base);

        assert!(gm.update_rule_sets_if_needed(&rule_sets).unwrap());
        assert_eq!(
            fs::read(gm.geo_dir.join("geoip-stub.srs")).unwrap(),
            b"SRS\x01DATA"
        );
        assert_eq!(
            fs::read(gm.geo_dir.join("geosite-stub.srs")).unwrap(),
            b"SRS\x01DATA"
        );
        assert!(!gm.update_rule_sets_if_needed(&rule_sets).unwrap());
    }

    fn assert_metadata_writable_during_requests(
        refresh: impl FnOnce(&GeoManager, &str) -> Result<bool> + Send,
    ) {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Mutex::new(release_rx);
        let base = spawn_stub_http(2, move |_, _| {
            let _ = entered_tx.send(());
            let _ = release_rx.lock().unwrap().recv();
            (200, Some("etag".to_string()), b"SRS\x01DATA".to_vec())
        });

        std::thread::scope(|scope| {
            let gm = &gm;
            let worker = scope.spawn(move || refresh(gm, &base));
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let (cleared_tx, cleared_rx) = std::sync::mpsc::channel();
            scope.spawn(move || cleared_tx.send(gm.clear_retry_state(GeoRegion::Ru).is_ok()));
            let cleared = cleared_rx.recv_timeout(Duration::from_secs(2));
            drop(release_tx);
            assert_eq!(
                cleared,
                Ok(true),
                "metadata write waited for a rule-set request"
            );
            assert!(worker.join().unwrap().unwrap());
        });
    }

    #[test]
    fn region_refresh_does_not_hold_metadata_lock_during_requests() {
        assert_metadata_writable_during_requests(|gm, base| {
            gm.update_region_rule_sets(GeoRegion::Ru, &stub_rule_sets(base))
                .map(|result| matches!(result, RegionUpdate::Updated { .. }))
        });
    }

    #[test]
    fn update_assets_check_error_does_not_starve_missing_sibling() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let base = spawn_stub_http(2, |_, _| {
            (200, Some("e1".to_string()), b"SRS\x01NEW".to_vec())
        });
        let rule_sets = [
            stub_rule_set(
                "geoip-stub",
                RuleSetKind::Geoip,
                "http://127.0.0.1:1/dead".to_string(),
            ),
            stub_rule_set(
                "geosite-stub",
                RuleSetKind::Geosite,
                format!("{base}/geosite"),
            ),
        ];
        fs::write(gm.geo_dir.join("geoip-stub.srs"), b"SRS\x01OLD").unwrap();

        let err = gm
            .update_rule_sets_if_needed(&rule_sets)
            .unwrap_err()
            .to_string();
        assert!(err.contains("HEAD request failed"), "{err}");
        assert_eq!(
            fs::read(gm.geo_dir.join("geosite-stub.srs")).unwrap(),
            b"SRS\x01NEW"
        );
        let meta = gm.load_metadata().unwrap();
        assert_eq!(
            meta.etags.get("geosite-stub.srs").map(String::as_str),
            Some("e1")
        );
    }

    const CAPTIVE_PORTAL_PAGE: &[u8] = b"<html>Sign in to the network</html>";

    fn with_corrupt_cache_and_unchanged_etags(
        refresh: impl FnOnce(&GeoManager, &[&'static RuleSet]) -> Result<bool>,
    ) {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let base = spawn_stub_http(4, |_, _| {
            (200, Some("same".to_string()), b"SRS\x01NEW".to_vec())
        });
        let rule_sets = stub_rule_sets(&base);
        for rule_set in &rule_sets {
            fs::write(gm.rule_set_path(rule_set), CAPTIVE_PORTAL_PAGE).unwrap();
        }
        gm.update_metadata(|meta| {
            for rule_set in &rule_sets {
                meta.etags.insert(rule_set.filename(), "same".to_string());
            }
            true
        })
        .unwrap();

        assert!(refresh(&gm, &rule_sets).unwrap());
        for rule_set in &rule_sets {
            assert_eq!(fs::read(gm.rule_set_path(rule_set)).unwrap(), b"SRS\x01NEW");
        }
    }

    #[test]
    fn region_refresh_replaces_a_corrupt_cache_despite_an_unchanged_etag() {
        with_corrupt_cache_and_unchanged_etags(|gm, rule_sets| {
            gm.update_region_rule_sets(GeoRegion::Ru, rule_sets)
                .map(|result| matches!(result, RegionUpdate::Updated { .. }))
        });
    }

    #[test]
    fn a_response_without_the_rule_set_header_is_not_written() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let base = spawn_stub_http(2, |_, _| {
            (
                200,
                Some("portal".to_string()),
                CAPTIVE_PORTAL_PAGE.to_vec(),
            )
        });
        let rule_sets = stub_rule_sets(&base);
        let geoip = gm.rule_set_path(rule_sets[0]);
        fs::write(&geoip, crate::test_helpers::RULE_SET_BYTES).unwrap();

        let err = gm
            .update_rule_sets_if_needed(&rule_sets[..1])
            .unwrap_err()
            .to_string();

        assert!(err.contains("geoip-stub.srs"), "{err}");
        assert_eq!(
            fs::read(&geoip).unwrap(),
            crate::test_helpers::RULE_SET_BYTES
        );
        assert!(gm.load_metadata().unwrap().etags.is_empty());
    }

    #[test]
    fn a_file_without_the_rule_set_header_does_not_count_as_downloaded() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let [geosite, geoip] = region_rule_sets(GeoRegion::Ru) else {
            unreachable!()
        };
        fs::write(gm.rule_set_path(geoip), crate::test_helpers::RULE_SET_BYTES).unwrap();
        fs::write(gm.rule_set_path(geosite), CAPTIVE_PORTAL_PAGE).unwrap();
        assert!(!gm.has_databases(GeoRegion::Ru));

        fs::write(gm.rule_set_path(geosite), b"SRS").unwrap();
        assert!(!gm.has_databases(GeoRegion::Ru));
    }

    #[test]
    fn metadata_roundtrip_via_map() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let now = Local::now();
        let mut etags = HashMap::new();
        let mut updated_at = HashMap::new();
        for region in RULE_REGIONS {
            for rule_set in region_rule_sets(region) {
                etags.insert(rule_set.filename(), format!("etag-{}", rule_set.tag));
            }
            updated_at.insert(region, now);
        }
        let meta = GeoMetadata {
            etags,
            updated_at,
            checked_at: HashMap::new(),
            region_retry_states: HashMap::new(),
            region_next_update: HashMap::new(),
            schedule_epoch: 0,
        };
        gm.save_metadata(&meta).unwrap();
        let loaded = gm.load_metadata().unwrap();
        for region in RULE_REGIONS {
            for rule_set in region_rule_sets(region) {
                assert_eq!(
                    loaded.etags.get(&rule_set.filename()),
                    Some(&format!("etag-{}", rule_set.tag))
                );
            }
            assert!(loaded.updated_at.contains_key(&region));
        }
        assert!(loaded.checked_at.is_empty());
    }

    #[test]
    fn checked_at_roundtrips_per_region() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let ru_checked = Local::now() - chrono::Duration::try_hours(2).unwrap();
        let cn_checked = Local::now();
        let mut meta = GeoMetadata::default();
        meta.checked_at.insert(GeoRegion::Ru, ru_checked);
        meta.checked_at.insert(GeoRegion::Cn, cn_checked);
        gm.save_metadata(&meta).unwrap();

        assert_eq!(gm.last_checked_at(GeoRegion::Ru), Some(ru_checked));
        assert_eq!(gm.last_checked_at(GeoRegion::Cn), Some(cn_checked));
        assert_eq!(gm.last_checked_at(GeoRegion::Ir), None);
        assert_eq!(gm.last_checked_at(GeoRegion::Global), None);
    }

    #[test]
    fn retry_backoff_persists_per_region_and_clears_independently() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let now = after_update_window();

        for (failures, delay) in [(1, 1), (2, 5), (3, 15), (4, 60)] {
            let state = gm
                .record_update_failure_at(GeoRegion::Ru, now, 0)
                .unwrap()
                .unwrap();
            assert_eq!(state.consecutive_failures, failures);
            assert_eq!(state.retry_at, now + chrono::Duration::minutes(delay));
        }
        let fifth = gm
            .record_update_failure_at(GeoRegion::Ru, now, 0)
            .unwrap()
            .unwrap();
        assert_eq!(fifth.consecutive_failures, 5);
        assert!(fifth.retry_at.date_naive() > now.date_naive());
        let cn = gm
            .record_update_failure_at(GeoRegion::Cn, now, 0)
            .unwrap()
            .unwrap();
        assert_eq!(cn.consecutive_failures, 1);
        assert_eq!(
            gm.retry_state(GeoRegion::Ru).unwrap().consecutive_failures,
            5
        );

        gm.clear_retry_state(GeoRegion::Ru).unwrap();
        assert!(gm.retry_state(GeoRegion::Ru).is_none());
        assert_eq!(gm.retry_state(GeoRegion::Cn), Some(cn));
    }

    #[test]
    fn metadata_migrates_legacy_per_country_etag_fields() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let legacy = r#"{
            "geoip_ru_etag": "legacy-ru-ip",
            "geosite_ru_etag": "legacy-ru-site",
            "geoip_cn_etag": "legacy-cn-ip",
            "geoip_ir_etag": "legacy-ir-ip"
        }"#;
        fs::write(&gm.metadata_path, legacy).unwrap();
        let loaded = gm.load_metadata().unwrap();
        assert_eq!(
            loaded.etags.get("geoip-ru.srs").map(String::as_str),
            Some("legacy-ru-ip")
        );
        assert_eq!(
            loaded
                .etags
                .get("geosite-category-ru.srs")
                .map(String::as_str),
            Some("legacy-ru-site")
        );
        assert_eq!(
            loaded.etags.get("geoip-cn.srs").map(String::as_str),
            Some("legacy-cn-ip")
        );
        assert_eq!(
            loaded.etags.get("geoip-ir.srs").map(String::as_str),
            Some("legacy-ir-ip")
        );
        assert!(!loaded.etags.contains_key("geosite-cn.srs"));
        assert!(!loaded.etags.contains_key("geosite-category-ir.srs"));
    }

    #[test]
    fn metadata_map_wins_over_legacy_field_when_both_present() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let mixed = r#"{
            "etags": { "geoip-ru.srs": "modern" },
            "geoip_ru_etag": "legacy"
        }"#;
        fs::write(&gm.metadata_path, mixed).unwrap();
        let loaded = gm.load_metadata().unwrap();
        assert_eq!(
            loaded.etags.get("geoip-ru.srs").map(String::as_str),
            Some("modern")
        );
    }

    #[test]
    fn load_metadata_missing_returns_default() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let _ = fs::remove_file(&gm.metadata_path);
        let meta = gm.load_metadata().unwrap();
        assert!(meta.etags.is_empty());
        assert!(meta.updated_at.is_empty());
        assert!(meta.checked_at.is_empty());
    }

    #[test]
    fn has_databases_reflects_file_presence_per_region() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        assert!(gm.has_databases(GeoRegion::Global));
        for region in RULE_REGIONS {
            let [geosite, geoip] = region_rule_sets(region) else {
                unreachable!()
            };
            let geoip = gm.rule_set_path(geoip);
            let geosite = gm.rule_set_path(geosite);
            let _ = fs::remove_file(&geoip);
            let _ = fs::remove_file(&geosite);

            assert!(!gm.has_databases(region), "missing files: {:?}", region);
            fs::write(&geoip, crate::test_helpers::RULE_SET_BYTES).unwrap();
            assert!(
                !gm.has_databases(region),
                "only geoip present: {:?}",
                region
            );
            fs::write(&geosite, crate::test_helpers::RULE_SET_BYTES).unwrap();
            assert!(gm.has_databases(region), "both present: {:?}", region);

            let _ = fs::remove_file(&geoip);
            let _ = fs::remove_file(&geosite);
        }
    }

    #[test]
    fn write_atomic_creates_file() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let dest = gm.geo_dir.join("test_atomic.txt");
        let _ = fs::remove_file(&dest);
        gm.write_atomic(&dest, b"hello world").unwrap();
        assert!(dest.exists());
        let contents = fs::read_to_string(&dest).unwrap();
        assert_eq!(contents, "hello world");
        let _ = fs::remove_file(&dest);
    }

    #[test]
    fn write_atomic_preserves_srs_extension() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let dest = gm.geo_dir.join("geoip-ru.srs");
        let _ = fs::remove_file(&dest);
        gm.write_atomic(&dest, b"data").unwrap();
        assert!(dest.exists());
        let temp = gm.geo_dir.join("geoip-ru.srs.tmp");
        assert!(!temp.exists());
        let _ = fs::remove_file(&dest);
    }

    #[test]
    fn last_updated_returns_none_for_global() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        assert!(gm.last_updated(GeoRegion::Global).is_none());
    }

    #[test]
    fn last_updated_returns_none_when_metadata_missing() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let _ = fs::remove_file(&gm.metadata_path);
        for region in RULE_REGIONS {
            assert!(gm.last_updated(region).is_none());
        }
    }

    #[test]
    fn last_updated_returns_last_check_formatted_after_save() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let mut checked_at = HashMap::new();
        let dt = Local::now();
        checked_at.insert(GeoRegion::Ru, dt);
        let meta = GeoMetadata {
            checked_at,
            ..GeoMetadata::default()
        };
        gm.save_metadata(&meta).unwrap();
        let formatted = gm.last_updated(GeoRegion::Ru).unwrap();
        assert_eq!(formatted.len(), 12);
        assert_eq!(formatted, dt.format("%d %b %H:%M").to_string());
        assert!(gm.last_updated(GeoRegion::Cn).is_none());
    }

    #[test]
    fn download_databases_global_is_noop() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        assert!(!gm.download_databases(GeoRegion::Global).unwrap());
        assert!(!gm.metadata_path.exists());
    }

    #[test]
    fn update_if_needed_global_is_up_to_date() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        let result = gm.update_if_needed(GeoRegion::Global).unwrap();
        assert_eq!(result, RegionUpdate::UpToDate { checked_at: None });
    }

    #[test]
    fn load_metadata_parse_error_is_propagated() {
        let _guard = crate::test_helpers::ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let gm = GeoManager::new().unwrap();
        fs::write(&gm.metadata_path, b"not valid json {{").unwrap();
        let err = gm.load_metadata().unwrap_err().to_string();
        assert!(err.contains("Failed to parse"));
    }
}
