use crate::app::msg::Msg;
use std::future::poll_fn;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::mpsc::Sender;
use zbus::export::futures_core::Stream;
use zbus::{Connection, MatchRule, MessageStream};

pub fn listen_blocking(tx: Sender<Msg>) {
    zbus::block_on(listen(tx));
}

async fn listen(tx: Sender<Msg>) {
    let Ok(conn) = Connection::system().await else {
        tracing::warn!("Failed to connect to system D-Bus");
        return;
    };

    let rule = match MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")
        .interface("org.freedesktop.login1.Manager")
        .member("PrepareForSleep")
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Failed to build D-Bus match rule: {}", e);
            return;
        }
    };

    let mut stream = match MessageStream::for_match_rule(rule, &conn, Some(NonZeroUsize::MIN)).await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Failed to create D-Bus message stream: {}", e);
            return;
        }
    };

    while let Some(msg) = poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await {
        if let Ok(msg) = msg
            && let Ok(going_to_sleep) = msg.body().deserialize::<bool>()
            && !going_to_sleep
        {
            let _ = tx.send(Msg::SystemResumed);
        }
    }
}
