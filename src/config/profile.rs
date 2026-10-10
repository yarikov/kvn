mod diagnostic;
mod dns;
mod entry;
mod json_schema;
mod migrate;
mod protocol;
mod protocol_config;
mod protocol_options;
mod routing;
mod schedule;
mod schema;
mod settings;
mod share_link;
mod subscription;
mod tls;

pub use diagnostic::ConfigDiagnostic;
pub(crate) use diagnostic::into_result;
#[cfg(test)]
pub use dns::CustomDnsPreset;
pub use dns::{ActiveDns, DnsConfig, DnsPreset, DnsRule, DnsServer, DnsStrategy, FakeIpServer};
pub use entry::Profile;
pub use json_schema::{config_json_schema, schema_diagnostics};
pub use protocol::Protocol;
pub use protocol_config::{
    AnytlsConfig, HttpConfig, Hysteria2Config, NaiveConfig, ProtocolConfig, ShadowsocksConfig,
    ShadowtlsConfig, SocksConfig, SshConfig, TrojanConfig, TuicConfig, VlessConfig, VmessConfig,
};
pub use protocol_options::{
    Hysteria2Obfs, Hysteria2ObfsType, ShadowsocksCipher, ShadowtlsVersion, SocksVersion,
    TuicCongestion, TuicUdpRelayMode, VmessSecurity,
};
pub use routing::{GeoRegion, GeoRouting, RoutedService, RoutingMode, ServiceRoute};
pub use schedule::{
    GeoAutoUpdate, SubscriptionAutoUpdate, next_update_window_date, retry_delay_minutes,
};
pub use schema::{CURRENT_SCHEMA_VERSION, Config};
pub use settings::{
    ConnectivityProbeConfig, IconSet, OMARCHY_THEME_SENTINEL, Settings, normalized_log_level,
    parse_connectivity_probe_url,
};
pub(crate) use share_link::{FINALMASK_PARAM, decode_b64_lenient, read_hysteria2_finalmask};
pub use share_link::{
    SUPPORTED_SHARE_SCHEMES, encode_share_link, is_http_proxy_link, parse_share_link,
};
pub use subscription::Subscription;
#[cfg(test)]
pub use subscription::SubscriptionRetryState;
pub use tls::{
    EchSettings, Flow, RealitySettings, Security, TlsCommon, TransportConfig, TransportType,
};
