#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use marmot_chat::controller::events::{ChatEvent, Role, SessionParams, StubConfig, StubWrapper};
use marmot_chat::scenario::{DeterministicScenario, WrapperKind};
use marmot_chat::WasmChatController;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

use gloo_timers::future::TimeoutFuture;
use serde_wasm_bindgen as swb;

#[wasm_bindgen_test]
async fn bob_bootstrap_flow() {
    let mut scenario = DeterministicScenario::new().expect("deterministic scenario");
    let config = scenario.config.clone();
    let backlog_wrappers = scenario
        .conversation
        .initial_backlog()
        .expect("alice backlog wrappers");

    let stub_backlog: Vec<StubWrapper> = backlog_wrappers
        .iter()
        .map(|wrapper| StubWrapper {
            bytes: wrapper.bytes.clone(),
            label: wrapper.kind.label().to_string(),
        })
        .collect();

    let stub = StubConfig {
        backlog: stub_backlog,
        welcome: Some(config.welcome_json.clone()),
        key_package_bundle: Some(config.joiner_key_package.bundle.clone()),
        key_package_event: Some(config.joiner_key_package.event_json.clone()),
        group_id_hex: Some(config.group_id_hex.clone()),
        pause_after_frames: None,
    };

    let session = SessionParams {
        role: Role::Joiner,
        relay_url: "stub://relay".to_string(),
        nostr_url: "stub://nostr".to_string(),
        session_id: "phase4".to_string(),
        secret_hex: config.joiner_secret_hex.clone(),
        invitee_pubkey: Some(config.creator_pubkey.clone()),
        group_id_hex: Some(config.group_id_hex.clone()),
        stub: Some(stub),
    };

    let events = Rc::new(RefCell::new(Vec::<ChatEvent>::new()));
    let events_ref = events.clone();
    let callback = Closure::wrap(Box::new(move |value: JsValue| {
        if let Ok(event) = swb::from_value::<ChatEvent>(value) {
            events_ref.borrow_mut().push(event);
        }
    }) as Box<dyn FnMut(JsValue)>);

    let session_js = swb::to_value(&session).expect("serialize session");
    let handle = WasmChatController::start(session_js, callback.as_ref().clone())
        .map_err(|err| err.as_string().unwrap_or_default())
        .expect("controller start");
    callback.forget();

    TimeoutFuture::new(50).await;

    handle.shutdown();

    let recorded = events.borrow();
    let messages: Vec<_> = recorded
        .iter()
        .filter_map(|event| match event {
            ChatEvent::Message { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect();

    let expected: Vec<_> = backlog_wrappers
        .iter()
        .filter_map(|wrapper| match &wrapper.kind {
            WrapperKind::Application { content, .. } => Some(content.clone()),
            WrapperKind::Commit => None,
        })
        .collect();

    assert_eq!(messages, expected, "decrypted backlog mismatch");

    let commit_events = recorded
        .iter()
        .filter(|event| matches!(event, ChatEvent::Commit { .. }))
        .count();
    let expected_commits = backlog_wrappers
        .iter()
        .filter(|wrapper| matches!(wrapper.kind, WrapperKind::Commit))
        .count();
    assert_eq!(commit_events, expected_commits, "commit count mismatch");
}
