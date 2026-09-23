use std::sync::mpsc::Sender;
use std::thread;

use uuid::Uuid;

use crate::app::model::Model;
use crate::app::msg::{IpcError, Msg};

pub(super) fn fetch(tx: &Sender<Msg>, model: &Model, id: Uuid) {
    if let Some(sub) = model.config.subscriptions.iter().find(|s| s.id == id) {
        let sub = sub.clone();
        let settings = model.config.settings.clone();
        if let Some(message) = crate::config::subscription::deprecated_http_warning(&sub, &settings)
        {
            crate::services::log_tailer::append_app_log("WARN", &message);
        }
        let tx = tx.clone();
        thread::spawn(move || {
            let result = crate::config::subscription::fetch_subscription(&sub, &settings)
                .map_err(IpcError::from);
            let _ = tx.send(Msg::SubscriptionFetched { id, result });
        });
    }
}
