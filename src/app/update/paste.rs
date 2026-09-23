use crate::app::effect::Effect;
use crate::app::model::Model;
use crate::config::profile::{Subscription, SubscriptionAutoUpdate};
use uuid::Uuid;

use crate::app::update::status::{
    DownloadKind, download_allowed, push_download_blocked, push_status,
};

pub(in crate::app::update) fn handle_clipboard_text(model: &mut Model, text: &str) -> Vec<Effect> {
    let trimmed = text.trim();
    if trimmed.is_empty() || !trimmed.contains("://") {
        let message = if trimmed.is_empty() {
            "Clipboard is empty"
        } else {
            "Not a supported VPN link or subscription URL"
        };
        let mut effects = Vec::new();
        push_status(
            &mut effects,
            model,
            crate::app::model::AppStatus::Error(message.into()),
        );
        return effects;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return add_and_fetch_subscription(model, trimmed);
    }

    match crate::config::profile::parse_share_link(trimmed) {
        Ok(profile) => {
            if model.has_duplicate(&profile) {
                let mut effects = Vec::new();
                push_status(
                    &mut effects,
                    model,
                    crate::app::model::AppStatus::Error("Profile already exists".into()),
                );
                return effects;
            }
            let name = profile.name.clone();
            model.add_profile(profile);
            let mut effects = vec![Effect::SaveConfig];
            push_status(
                &mut effects,
                model,
                crate::app::model::AppStatus::Info(format!("Pasted profile: {}", name)),
            );
            effects
        }
        Err(e) => {
            let mut effects = Vec::new();
            push_status(
                &mut effects,
                model,
                crate::app::model::AppStatus::Error(format!("Invalid URI: {}", e)),
            );
            effects
        }
    }
}

fn add_and_fetch_subscription(model: &mut Model, url: &str) -> Vec<Effect> {
    if let Err(error) = crate::config::subscription::validate_subscription_url(
        url,
        model.config.settings.allow_insecure_http_subscriptions,
    ) {
        let mut effects = Vec::new();
        push_status(
            &mut effects,
            model,
            crate::app::model::AppStatus::Error(error.to_string()),
        );
        return effects;
    }

    let name = derive_subscription_name(url);
    let id = Uuid::new_v4();
    let sub = Subscription {
        id,
        name: name.clone(),
        url: url.to_string(),
        auto_update: SubscriptionAutoUpdate::default(),
        last_updated: None,
        next_auto_update: None,
        retry_state: None,
        send_hwid: false,
        hwid: None,
    };
    model.config.subscriptions.push(sub);
    model.selected = crate::app::model::row_for_subscription_header(
        &model.config,
        model.config.subscriptions.len().saturating_sub(1),
    );
    let mut effects = vec![Effect::SaveConfig];
    if !download_allowed(model) {
        push_download_blocked(&mut effects, model, DownloadKind::Subscription);
        return effects;
    }
    model.subscription_fetching = true;
    model.subscription_updates.insert(id);
    effects.push(Effect::UpdateSubscription { id });
    push_status(
        &mut effects,
        model,
        crate::app::model::AppStatus::Info(format!(
            "Added subscription '{}' and fetching profiles…",
            name
        )),
    );
    effects
}

fn derive_subscription_name(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| "Subscription".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::ConnectionState;
    use crate::test_helpers::*;

    #[test]
    fn paste_duplicate_profile_shows_error() {
        let mut model = model_with_profiles(vec![]);
        let uri = "vless://671c62c7-6768-4b98-ac6b-572c9c707be0@203.0.113.42:443#Test";

        // First paste succeeds
        let effects = handle_clipboard_text(&mut model, uri);
        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(
            effects,
            vec![Effect::SaveConfig, app_log_info("Pasted profile: Test")]
        );
        assert!(model.status_text().contains("Pasted profile"));

        // Second paste with same UUID fails
        let effects = handle_clipboard_text(&mut model, uri);
        assert_eq!(model.config.profiles.len(), 1);
        assert_eq!(effects, vec![app_log_error("Profile already exists")]);
        assert!(model.status_is_error());
        assert!(model.status_text().contains("already exists"));
    }

    #[test]
    fn paste_subscription_url_creates_subscription_and_fetches() {
        let mut model = model_with_profiles(vec![]);
        let url = "https://192.0.2.10:2096/sub/test-token";

        let effects = handle_clipboard_text(&mut model, url);

        assert_eq!(model.config.subscriptions.len(), 1);
        assert_eq!(model.config.subscriptions[0].url, url);
        assert!(matches!(
            model.selected_row(),
            Some(crate::app::model::SourceRow::SubscriptionHeader(0))
        ));
        assert!(model.subscription_fetching);
        assert!(
            model
                .subscription_updates
                .contains(&model.config.subscriptions[0].id)
        );
        assert_eq!(
            effects,
            vec![
                Effect::SaveConfig,
                Effect::UpdateSubscription {
                    id: model.config.subscriptions[0].id
                },
                app_log_info("Added subscription '192.0.2.10' and fetching profiles…")
            ]
        );
    }

    #[test]
    fn paste_http_subscription_is_allowed_with_compatibility_flag() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.allow_insecure_http_subscriptions = true;

        let effects = handle_clipboard_text(&mut model, "http://example.com/secret-token");

        assert_eq!(model.config.subscriptions.len(), 1);
        assert!(model.subscription_fetching);
        assert!(effects.contains(&Effect::SaveConfig));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::UpdateSubscription { id }
                if *id == model.config.subscriptions[0].id
        )));
    }

    #[test]
    fn paste_http_subscription_is_rejected_for_new_users() {
        let mut model = model_with_profiles(vec![]);

        let effects = handle_clipboard_text(&mut model, "http://example.com/secret-token");

        assert!(model.config.subscriptions.is_empty());
        assert!(!model.subscription_fetching);
        assert_eq!(
            effects,
            vec![app_log_error(
                "Insecure HTTP subscriptions are blocked; use HTTPS"
            )]
        );
    }

    #[test]
    fn paste_plain_text_shows_error_without_side_effects() {
        let mut model = model_with_profiles(vec![]);
        let effects = handle_clipboard_text(&mut model, "m");
        assert!(!effects.contains(&Effect::SaveConfig));
        assert_eq!(model.overlay, crate::app::model::Overlay::None);
        assert!(model.config.profiles.is_empty());
        assert!(model.status_is_error());
        assert_eq!(
            model.status_text(),
            "Not a supported VPN link or subscription URL"
        );

        handle_clipboard_text(&mut model, "  \n");
        assert_eq!(model.status_text(), "Clipboard is empty");
    }

    #[test]
    fn paste_vless_adds_standalone_profile() {
        let mut model = model_with_profiles(vec![]);
        let uri = "vless://671c62c7-6768-4b98-ac6b-572c9c707be0@203.0.113.42:443#Test";

        let effects = handle_clipboard_text(&mut model, uri);

        assert_eq!(model.config.profiles.len(), 1);
        assert!(matches!(
            model.selected_row(),
            Some(crate::app::model::SourceRow::StandaloneProfile(0))
        ));
        assert_eq!(
            effects,
            vec![Effect::SaveConfig, app_log_info("Pasted profile: Test")]
        );
    }

    #[test]
    fn pasted_subscription_is_saved_but_not_fetched_behind_kill_switch() {
        let mut model = model_with_profiles(vec![]);
        model.config.settings.kill_switch = true;
        let effects = handle_clipboard_text(&mut model, "https://example.com/sub");

        assert_eq!(model.config.subscriptions.len(), 1);
        assert!(!model.subscription_fetching);
        assert!(model.subscription_updates.is_empty());
        assert!(effects.contains(&Effect::SaveConfig));
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::UpdateSubscription { .. }))
        );
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::AppendAppLog { message, .. }
                if message.contains("Subscription update is blocked")
        )));
    }

    #[test]
    fn pasted_subscription_can_fetch_directly_or_through_vpn() {
        let mut direct = model_with_profiles(vec![]);
        let effects = handle_clipboard_text(&mut direct, "https://example.com/direct");
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::UpdateSubscription { .. }))
        );

        let mut tunneled = model_with_profiles(vec![]);
        tunneled.config.settings.kill_switch = true;
        tunneled.connection = ConnectionState::Connected;
        let effects = handle_clipboard_text(&mut tunneled, "https://example.com/tunneled");
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::UpdateSubscription { .. }))
        );
    }

    // ---- derive_subscription_name ----

    #[test]
    fn derive_subscription_name_uses_host_when_present() {
        assert_eq!(
            derive_subscription_name("https://example.com/sub"),
            "example.com"
        );
        assert_eq!(
            derive_subscription_name("https://sub.example.com:8443/path?q=1"),
            "sub.example.com"
        );
    }

    #[test]
    fn derive_subscription_name_ip_url() {
        assert_eq!(derive_subscription_name("http://1.2.3.4/sub"), "1.2.3.4");
    }

    #[test]
    fn derive_subscription_name_invalid_url_fallback() {
        assert_eq!(derive_subscription_name("not a url"), "Subscription");
    }

    #[test]
    fn derive_subscription_name_url_without_host_fallback() {
        // `file:` URLs parse but have no host.
        assert_eq!(derive_subscription_name("file:///tmp/sub"), "Subscription");
    }
}
