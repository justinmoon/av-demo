use futures::channel::mpsc::UnboundedSender;

use super::core::Operation;

pub(super) fn now_timestamp() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0) as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

pub(super) fn schedule(tx: &UnboundedSender<Operation>, op: Operation) {
    if let Err(err) = tx.unbounded_send(op) {
        log::error!("operation queue closed: {err}");
    }
}

pub(super) fn short_key(key: &str) -> String {
    if key.len() <= 12 {
        key.to_string()
    } else {
        format!("{}…{}", &key[..6], &key[key.len() - 4..])
    }
}

pub(super) fn relay_relays_url(url: &str) -> String {
    url.parse::<url::Url>()
        .map(|parsed| {
            format!("wss://{}", parsed.host_str().unwrap_or("localhost"))
        })
        .unwrap_or_else(|_| "wss://localhost".to_string())
}
