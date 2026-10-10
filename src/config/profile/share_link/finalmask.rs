use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::config::profile::{Hysteria2Config, Hysteria2Obfs, Hysteria2ObfsType};

pub(crate) const FINALMASK_PARAM: &str = "fm";

const GECKO_PACKET_SIZES: std::ops::RangeInclusive<u32> = 1..=2048;
const MIN_HOP_INTERVAL_SECS: u32 = 5;
const DEFAULT_HOP_INTERVAL_SECS: u32 = 30;
const HOP_MODE: &str = "intervalRemote";

const CLIENT_QUIC_TUNING_KEYS: [&str; 15] = [
    "congestion",
    "debug",
    "bbrProfile",
    "brutalDisableLossCompensation",
    "initStreamReceiveWindow",
    "maxStreamReceiveWindow",
    "initConnectionReceiveWindow",
    "maxConnectionReceiveWindow",
    "maxIdleTimeout",
    "keepAlivePeriod",
    "disablePathMTUDiscovery",
    "disableChromeParrot",
    "disableGSO",
    "maxIncomingStreams",
    "disableStatelessReset",
];

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct HopInterval {
    pub secs: Option<u32>,
    pub max_secs: Option<u32>,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Hysteria2FinalMask {
    pub obfs: Option<Hysteria2Obfs>,
    pub ports: Option<String>,
    pub hop_interval: HopInterval,
    pub up_mbps: Option<u32>,
    pub down_mbps: Option<u32>,
    pub client_tuning: Map<String, Value>,
}

impl Hysteria2FinalMask {
    pub(crate) fn carries_link_settings(&self) -> bool {
        self.obfs.is_some()
            || self.ports.is_some()
            || self.hop_interval != HopInterval::default()
            || self.up_mbps.is_some()
            || self.down_mbps.is_some()
    }

    pub(crate) fn client_tuning_param(&self) -> Option<Value> {
        (!self.client_tuning.is_empty())
            .then(|| Value::String(json!({ "quicParams": self.client_tuning }).to_string()))
    }
}

pub(crate) fn read_hysteria2_finalmask(fm: &str) -> Result<Hysteria2FinalMask> {
    let value: Value = serde_json::from_str(fm).context("fm is not valid JSON")?;
    let root = value.as_object().context("fm is not a JSON object")?;
    let mut mask = Hysteria2FinalMask::default();
    for (key, value) in root {
        match key.as_str() {
            "tcp" => {
                if let Some(kind) = first_mask_type(value)? {
                    bail!("fm TCP mask \"{kind}\" is not supported by sing-box");
                }
            }
            "udp" => read_udp_masks(value, &mut mask)?,
            "quicParams" => read_quic_params(value, &mut mask)?,
            other => bail!("fm key \"{other}\" is not supported by sing-box"),
        }
    }
    Ok(mask)
}

fn first_mask_type(value: &Value) -> Result<Option<String>> {
    let masks = value.as_array().context("fm masks are not a list")?;
    Ok(masks.first().map(|mask| {
        mask.get("type")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string()
    }))
}

fn read_udp_masks(value: &Value, mask: &mut Hysteria2FinalMask) -> Result<()> {
    let masks = value.as_array().context("fm UDP masks are not a list")?;
    let mut hop_seen = false;
    for entry in masks {
        let entry = entry.as_object().context("fm UDP mask is not an object")?;
        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .context("fm UDP mask has no type")?;
        if let Some(key) = entry
            .keys()
            .find(|k| !matches!(k.as_str(), "type" | "settings"))
        {
            bail!("fm UDP mask \"{kind}\" field \"{key}\" is not supported by sing-box");
        }
        let settings = entry.get("settings").and_then(Value::as_object);
        match kind {
            "salamander" if mask.obfs.is_none() => {
                mask.obfs = Some(read_salamander(settings)?);
            }
            "udphop" if !hop_seen => {
                hop_seen = true;
                read_udp_hop(settings, mask)?;
            }
            "salamander" | "udphop" => {
                bail!("fm holds more than one \"{kind}\" mask, which sing-box cannot chain")
            }
            other => bail!("fm UDP mask \"{other}\" is not supported by sing-box"),
        }
    }
    Ok(())
}

fn read_salamander(settings: Option<&Map<String, Value>>) -> Result<Hysteria2Obfs> {
    let settings = settings.context("fm salamander mask has no settings")?;
    reject_unknown_keys(settings, &["password", "packetSize"], "salamander")?;
    let password = settings
        .get("password")
        .and_then(Value::as_str)
        .filter(|password| !password.is_empty())
        .context("fm salamander mask has no password")?
        .to_string();
    let (min, max) = settings
        .get("packetSize")
        .map(int_range)
        .transpose()
        .context("fm salamander packetSize is not a number or range")?
        .unwrap_or((0, 0));
    if max == 0 {
        return Ok(Hysteria2Obfs {
            kind: Hysteria2ObfsType::Salamander,
            password,
            ..Default::default()
        });
    }
    if !GECKO_PACKET_SIZES.contains(&min) || !GECKO_PACKET_SIZES.contains(&max) {
        bail!("fm gecko packetSize {min}-{max} is outside 1-2048");
    }
    Ok(Hysteria2Obfs {
        kind: Hysteria2ObfsType::Gecko,
        password,
        min_packet_size: u16::try_from(min).ok(),
        max_packet_size: u16::try_from(max).ok(),
    })
}

fn read_udp_hop(
    settings: Option<&Map<String, Value>>,
    mask: &mut Hysteria2FinalMask,
) -> Result<()> {
    let settings = settings.context("fm udphop mask has no settings")?;
    reject_unknown_keys(
        settings,
        &["mode", "interval", "remotePorts", "remoteIPs"],
        "udphop",
    )?;
    if settings
        .get("remoteIPs")
        .is_some_and(|ips| ips.as_array().is_none_or(|ips| !ips.is_empty()))
    {
        bail!("fm udphop remoteIPs is not supported by sing-box");
    }
    read_hop_mode(settings.get("mode"))?;
    let ports = settings
        .get("remotePorts")
        .context("fm udphop mask has no remotePorts")?;
    set_hopping(mask, ports, settings.get("interval"))
}

fn set_hopping(
    mask: &mut Hysteria2FinalMask,
    ports: &Value,
    interval: Option<&Value>,
) -> Result<()> {
    if mask.ports.is_some() {
        bail!("fm sets port hopping twice, in a udphop mask and in quicParams.udpHop");
    }
    mask.ports = Some(port_list(ports)?);
    mask.hop_interval = hop_interval(interval)?;
    Ok(())
}

fn read_hop_mode(mode: Option<&Value>) -> Result<()> {
    let mode = mode
        .and_then(Value::as_str)
        .context("fm udphop mask has no mode")?;
    let mut hops_remote = false;
    for part in mode.split(',').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "intervalremote" => hops_remote = true,
            "intervallocal" => {}
            _ => bail!("fm udphop mode \"{mode}\" is not supported by sing-box"),
        }
    }
    if !hops_remote {
        bail!("fm udphop mode \"{mode}\" is not supported by sing-box");
    }
    Ok(())
}

fn read_quic_params(value: &Value, mask: &mut Hysteria2FinalMask) -> Result<()> {
    let params = value
        .as_object()
        .context("fm quicParams is not an object")?;
    for (key, value) in params {
        match key.as_str() {
            "udpHop" => read_legacy_udp_hop(value, mask)?,
            "brutalUp" | "brutalDown" => match mbps(value) {
                Some(rate) if key == "brutalUp" => mask.up_mbps = Some(rate),
                Some(rate) => mask.down_mbps = Some(rate),
                None => {
                    mask.client_tuning.insert(key.clone(), value.clone());
                }
            },
            key if CLIENT_QUIC_TUNING_KEYS.contains(&key) => {
                mask.client_tuning.insert(key.to_string(), value.clone());
            }
            other => bail!("fm quicParams field \"{other}\" is not supported by sing-box"),
        }
    }
    Ok(())
}

fn read_legacy_udp_hop(value: &Value, mask: &mut Hysteria2FinalMask) -> Result<()> {
    let hop = value
        .as_object()
        .context("fm quicParams.udpHop is not an object")?;
    reject_unknown_keys(hop, &["ports", "interval"], "quicParams.udpHop")?;
    if hop.is_empty() {
        return Ok(());
    }
    let ports = hop
        .get("ports")
        .context("fm quicParams.udpHop has no ports")?;
    set_hopping(mask, ports, hop.get("interval"))
}

fn reject_unknown_keys(settings: &Map<String, Value>, known: &[&str], mask: &str) -> Result<()> {
    match settings.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => bail!("fm {mask} field \"{key}\" is not supported by sing-box"),
        None => Ok(()),
    }
}

fn int_range(value: &Value) -> Result<(u32, u32)> {
    let text = match value {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.trim().to_string(),
        _ => bail!("not a number or range"),
    };
    if text.is_empty() {
        return Ok((0, 0));
    }
    let (from, to) = text.split_once('-').unwrap_or((&text, &text));
    let from: u32 = from.trim().parse()?;
    let to: u32 = to.trim().parse()?;
    Ok((from.min(to), from.max(to)))
}

fn hop_interval(value: Option<&Value>) -> Result<HopInterval> {
    let (from, to) = value
        .map(int_range)
        .transpose()
        .context("fm hop interval is not a number or range")?
        .unwrap_or((0, 0));
    if (from, to) == (0, 0) || (from, to) == (DEFAULT_HOP_INTERVAL_SECS, DEFAULT_HOP_INTERVAL_SECS)
    {
        return Ok(HopInterval::default());
    }
    if from < MIN_HOP_INTERVAL_SECS {
        bail!("fm hop interval {from}-{to} is under {MIN_HOP_INTERVAL_SECS} seconds");
    }
    Ok(HopInterval {
        secs: Some(from),
        max_secs: (to > from).then_some(to),
    })
}

fn port_list(value: &Value) -> Result<String> {
    let text = match value {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.trim().to_string(),
        _ => bail!("fm hop ports are not a port list"),
    };
    let valid_port = |port: &str| port.trim().parse::<u16>().is_ok_and(|port| port > 0);
    let valid = !text.is_empty()
        && text.split(',').all(|part| {
            let (from, to) = part.split_once('-').unwrap_or((part, part));
            valid_port(from) && valid_port(to)
        });
    if !valid {
        bail!("fm hop ports \"{text}\" are not a port list");
    }
    Ok(text.replace(' ', ""))
}

fn mbps(value: &Value) -> Option<u32> {
    let text = value.as_str()?.trim().to_ascii_lowercase();
    let unit_start = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(unit_start);
    let bits_per_unit: f64 = match unit.trim() {
        "" | "b" | "bps" => 1.0,
        "k" | "kb" | "kbps" => 1024.0,
        "m" | "mb" | "mbps" => 1024.0 * 1024.0,
        "g" | "gb" | "gbps" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tbps" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bits_per_second = number.parse::<f64>().ok()? * bits_per_unit;
    let rate = (bits_per_second / 1_000_000.0).round();
    (rate >= 1.0 && rate <= f64::from(u32::MAX)).then_some(rate as u32)
}

pub(crate) fn export_hysteria2_finalmask(
    cfg: &Hysteria2Config,
    ports: Option<&str>,
    stored: Option<&str>,
) -> Option<String> {
    if let Some(stored) = stored.filter(|fm| read_hysteria2_finalmask(fm).is_err()) {
        return Some(stored.to_string());
    }
    let mut root = Map::new();
    if let (Some(secs), Some(ports)) = (cfg.hop_interval_secs, ports) {
        let interval = match cfg.hop_interval_max_secs {
            Some(max) if max > secs => format!("{secs}-{max}"),
            _ => secs.to_string(),
        };
        root.insert(
            "udp".into(),
            json!([{
                "type": "udphop",
                "settings": { "mode": HOP_MODE, "remotePorts": ports, "interval": interval },
            }]),
        );
    }
    if let Some(Value::Object(stored)) = stored.and_then(|fm| serde_json::from_str(fm).ok()) {
        for (key, value) in stored {
            root.entry(key).or_insert(value);
        }
    }
    (!root.is_empty()).then(|| Value::Object(root).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(fm: Value) -> Result<Hysteria2FinalMask> {
        read_hysteria2_finalmask(&fm.to_string())
    }

    fn salamander(settings: Value) -> Value {
        json!({ "udp": [{ "type": "salamander", "settings": settings }] })
    }

    fn obfs(fm: Value) -> Hysteria2Obfs {
        read(fm).unwrap().obfs.unwrap()
    }

    #[test]
    fn salamander_without_packet_size_stays_salamander() {
        for packet_size in [None, Some(json!(0)), Some(json!("0")), Some(json!("0-0"))] {
            let mut settings = json!({ "password": "pw" });
            if let Some(size) = &packet_size {
                settings["packetSize"] = size.clone();
            }
            assert_eq!(
                obfs(salamander(settings)),
                Hysteria2Obfs {
                    kind: Hysteria2ObfsType::Salamander,
                    password: "pw".into(),
                    ..Default::default()
                },
                "{packet_size:?}"
            );
        }
    }

    #[test]
    fn salamander_with_packet_size_is_gecko() {
        for (packet_size, range) in [
            (json!(1200), (1200, 1200)),
            (json!("1200"), (1200, 1200)),
            (json!("512-1200"), (512, 1200)),
            (json!("1200-512"), (512, 1200)),
        ] {
            let obfs = obfs(salamander(
                json!({ "password": "pw", "packetSize": packet_size }),
            ));
            assert_eq!(obfs.kind, Hysteria2ObfsType::Gecko);
            assert_eq!(
                (obfs.min_packet_size, obfs.max_packet_size),
                (Some(range.0), Some(range.1))
            );
        }
    }

    #[test]
    fn gecko_packet_size_outside_xray_bounds_is_refused() {
        for packet_size in [json!("0-1200"), json!(4096), json!("big")] {
            let fm = salamander(json!({ "password": "pw", "packetSize": packet_size }));
            assert!(read(fm).is_err(), "{packet_size}");
        }
    }

    #[test]
    fn udphop_mask_gives_ports_and_interval() {
        for (mode, interval, expected) in [
            ("intervalRemote", None, HopInterval::default()),
            (
                "intervalLocal,intervalRemote",
                Some(json!("10")),
                HopInterval {
                    secs: Some(10),
                    max_secs: None,
                },
            ),
            (
                "intervalRemote",
                Some(json!("20-10")),
                HopInterval {
                    secs: Some(10),
                    max_secs: Some(20),
                },
            ),
            ("intervalRemote", Some(json!(30)), HopInterval::default()),
        ] {
            let mut settings = json!({ "mode": mode, "remotePorts": "20000-30000,40000" });
            if let Some(interval) = &interval {
                settings["interval"] = interval.clone();
            }
            let mask =
                read(json!({ "udp": [{ "type": "udphop", "settings": settings }] })).unwrap();
            assert_eq!(mask.ports.as_deref(), Some("20000-30000,40000"), "{mode}");
            assert_eq!(mask.hop_interval, expected, "{mode} {interval:?}");
        }
    }

    #[test]
    fn udphop_outside_the_supported_subset_is_refused() {
        for settings in [
            json!({ "mode": "intervalRemote", "remotePorts": "20000", "remoteIPs": ["10.0.0.1"] }),
            json!({ "mode": "perConnRemote", "remotePorts": "20000" }),
            json!({ "mode": "intervalLocal", "remotePorts": "20000" }),
            json!({ "mode": "", "remotePorts": "20000" }),
            json!({ "remotePorts": "20000" }),
            json!({ "mode": "intervalRemote", "remotePorts": "20000", "interval": "3" }),
            json!({ "mode": "intervalRemote", "remotePorts": "0" }),
            json!({ "mode": "intervalRemote" }),
        ] {
            let fm = json!({ "udp": [{ "type": "udphop", "settings": settings }] });
            assert!(read(fm).is_err(), "{settings}");
        }
    }

    #[test]
    fn mixed_mask_chain_is_refused_by_name() {
        let fm = json!({ "udp": [
            { "type": "salamander", "settings": { "password": "pw" } },
            { "type": "header-custom", "settings": {} },
        ] });
        let error = read(fm).unwrap_err().to_string();
        assert!(error.contains("\"header-custom\""), "{error}");
    }

    #[test]
    fn salamander_and_udphop_combine_in_any_order() {
        let hop = json!({ "type": "udphop", "settings": { "mode": "intervalRemote", "remotePorts": "20000-30000" } });
        let obfs = json!({ "type": "salamander", "settings": { "password": "pw" } });
        for udp in [json!([hop.clone(), obfs.clone()]), json!([obfs, hop])] {
            let mask = read(json!({ "udp": udp })).unwrap();
            assert!(mask.obfs.is_some() && mask.ports.is_some());
        }
    }

    #[test]
    fn tcp_masks_and_unknown_keys_are_refused() {
        for fm in [
            json!({ "tcp": [{ "type": "fragment" }] }),
            json!({ "other": {} }),
            json!({ "quicParams": { "unknown": 1 } }),
        ] {
            assert!(read(fm.clone()).is_err(), "{fm}");
        }
        assert_eq!(
            read(json!({ "tcp": [], "udp": [] })).unwrap(),
            Hysteria2FinalMask::default()
        );
    }

    #[test]
    fn quic_params_give_hopping_and_brutal_and_keep_client_tuning() {
        let mask = read(json!({ "quicParams": {
            "udpHop": { "ports": 20000, "interval": "15" },
            "brutalUp": "100000000",
            "brutalDown": "100 mbps",
            "congestion": "bbr",
        } }))
        .unwrap();
        assert_eq!(mask.ports.as_deref(), Some("20000"));
        assert_eq!(mask.hop_interval.secs, Some(15));
        assert_eq!((mask.up_mbps, mask.down_mbps), (Some(100), Some(105)));
        assert_eq!(
            mask.client_tuning_param(),
            Some(Value::String(
                r#"{"quicParams":{"congestion":"bbr"}}"#.into()
            ))
        );
    }

    #[test]
    fn brutal_rates_follow_xray_units_and_keep_what_they_cannot_read() {
        for (rate, mbps) in [
            (json!("100000000"), Some(100)),
            (json!("1.5 g"), Some(1611)),
            (json!("500 kbps"), Some(1)),
            (json!(50), None),
            (json!("100 bps"), None),
            (json!("fast"), None),
        ] {
            let mask = read(json!({ "quicParams": { "brutalUp": rate.clone() } })).unwrap();
            assert_eq!(mask.up_mbps, mbps, "{rate}");
            assert_eq!(
                mask.client_tuning.contains_key("brutalUp"),
                mbps.is_none(),
                "{rate}"
            );
        }
    }

    #[test]
    fn empty_legacy_udp_hop_sets_nothing() {
        assert_eq!(
            read(json!({ "quicParams": { "udpHop": {} } })).unwrap(),
            Hysteria2FinalMask::default()
        );
        assert!(read(json!({ "quicParams": { "udpHop": { "interval": "10" } } })).is_err());
    }

    #[test]
    fn hopping_set_twice_is_refused() {
        let fm = json!({
            "udp": [{ "type": "udphop", "settings": { "mode": "intervalRemote", "remotePorts": "20000" } }],
            "quicParams": { "udpHop": { "ports": "30000" } },
        });
        assert!(read(fm).is_err());
    }

    #[test]
    fn export_writes_a_non_default_interval_and_keeps_client_tuning() {
        let cfg = Hysteria2Config {
            hop_interval_secs: Some(15),
            hop_interval_max_secs: Some(25),
            ..Default::default()
        };
        let fm = export_hysteria2_finalmask(
            &cfg,
            Some("20000-30000"),
            Some(r#"{"quicParams":{"congestion":"bbr"}}"#),
        )
        .unwrap();
        let mask = read_hysteria2_finalmask(&fm).unwrap();
        assert_eq!(mask.ports.as_deref(), Some("20000-30000"));
        assert_eq!(
            mask.hop_interval,
            HopInterval {
                secs: Some(15),
                max_secs: Some(25)
            }
        );
        assert_eq!(mask.client_tuning.get("congestion"), Some(&json!("bbr")));
        assert_eq!(
            export_hysteria2_finalmask(&Hysteria2Config::default(), Some("20000"), None),
            None
        );
    }
}
