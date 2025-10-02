#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use gloo_timers::future::TimeoutFuture;
use marmot_chat::controller::events::{ChatEvent, SessionParams, SessionRole};
use marmot_chat::controller::services::IdentityService;
use marmot_chat::WasmChatController;
use serde_wasm_bindgen as swb;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

#[path = "support/mod.rs"]
mod support;

use support::scenario::{CREATOR_SECRET, INVITEE_SECRET};

const THIRD_SECRET: &str = "9c4e9aba1e3ff5deaa1bcb2a7dce1f2f4a5c6d7e8f9a0b1c2d3e4f5061728394";

#[wasm_bindgen_test]
async fn bob_bootstrap_flow() {
    let session_id = unique_session_id("bob-bootstrap");
    let transport_id = format!("transport-{session_id}");
    let relay_url = format!("local://{session_id}/relay");
    let nostr_url = format!("local://{session_id}/nostr");

    let alice_pub = IdentityService::create(CREATOR_SECRET)
        .expect("alice identity")
        .public_key_hex();
    let bob_pub = IdentityService::create(INVITEE_SECRET)
        .expect("bob identity")
        .public_key_hex();

    let alice_session = SessionParams {
        bootstrap_role: SessionRole::Initial,
        relay_url: relay_url.clone(),
        nostr_url: nostr_url.clone(),
        session_id: session_id.clone(),
        secret_hex: CREATOR_SECRET.to_string(),
        peer_pubkeys: vec![bob_pub.clone()],
        group_id_hex: None,
        admin_pubkeys: vec![alice_pub.clone()],
        local_transport_id: Some(transport_id.clone()),
    };

    let bob_session = SessionParams {
        bootstrap_role: SessionRole::Invitee,
        relay_url: relay_url.clone(),
        nostr_url: nostr_url.clone(),
        session_id: session_id.clone(),
        secret_hex: INVITEE_SECRET.to_string(),
        peer_pubkeys: vec![alice_pub.clone()],
        group_id_hex: None,
        admin_pubkeys: vec![alice_pub.clone()],
        local_transport_id: Some(transport_id.clone()),
    };

    let (alice, alice_events) = start_controller(alice_session);
    let (bob, bob_events) = start_controller(bob_session);

    assert!(
        wait_until(60, || is_ready(&alice_events) && is_ready(&bob_events)).await,
        "bob bootstrap readiness: alice={:?} bob={:?}",
        alice_events.borrow(),
        bob_events.borrow()
    );

    alice.send_message("alice → bob #1".to_string());
    assert!(
        wait_until(60, || {
            has_message(&bob_events, &alice_pub, "alice → bob #1")
        })
        .await,
        "bob did not receive alice message; events={:?}",
        bob_events.borrow()
    );

    bob.send_message("bob → alice ack".to_string());
    assert!(
        wait_until(60, || {
            has_message(&alice_events, &bob_pub, "bob → alice ack")
        })
        .await,
        "alice did not receive bob ack; events={:?}",
        alice_events.borrow()
    );

    alice.rotate_epoch();
    wait_for_commit("alice commit", &alice_events).await;
    wait_for_commit("bob commit", &bob_events).await;

    alice.shutdown();
    bob.shutdown();
}

#[wasm_bindgen_test]
async fn three_member_invite_flow() {
    let session_id = unique_session_id("three-member");
    let transport_id = format!("transport-{session_id}");
    let relay_url = format!("local://{session_id}/relay");
    let nostr_url = format!("local://{session_id}/nostr");

    let alice_pub = IdentityService::create(CREATOR_SECRET)
        .expect("alice identity")
        .public_key_hex();
    let bob_pub = IdentityService::create(INVITEE_SECRET)
        .expect("bob identity")
        .public_key_hex();
    let carol_pub = IdentityService::create(THIRD_SECRET)
        .expect("carol identity")
        .public_key_hex();

    let alice_session = SessionParams {
        bootstrap_role: SessionRole::Initial,
        relay_url: relay_url.clone(),
        nostr_url: nostr_url.clone(),
        session_id: session_id.clone(),
        secret_hex: CREATOR_SECRET.to_string(),
        peer_pubkeys: vec![bob_pub.clone(), carol_pub.clone()],
        group_id_hex: None,
        admin_pubkeys: vec![alice_pub.clone()],
        local_transport_id: Some(transport_id.clone()),
    };

    let bob_session = SessionParams {
        bootstrap_role: SessionRole::Invitee,
        relay_url: relay_url.clone(),
        nostr_url: nostr_url.clone(),
        session_id: session_id.clone(),
        secret_hex: INVITEE_SECRET.to_string(),
        peer_pubkeys: vec![alice_pub.clone()],
        group_id_hex: None,
        admin_pubkeys: vec![alice_pub.clone()],
        local_transport_id: Some(transport_id.clone()),
    };

    let carol_session = SessionParams {
        bootstrap_role: SessionRole::Invitee,
        relay_url: relay_url.clone(),
        nostr_url: nostr_url.clone(),
        session_id: session_id.clone(),
        secret_hex: THIRD_SECRET.to_string(),
        peer_pubkeys: vec![alice_pub.clone(), bob_pub.clone()],
        group_id_hex: None,
        admin_pubkeys: vec![alice_pub.clone()],
        local_transport_id: Some(transport_id.clone()),
    };

    let (alice, alice_events) = start_controller(alice_session);
    let (bob, bob_events) = start_controller(bob_session);

    assert!(
        wait_until(60, || is_ready(&alice_events) && is_ready(&bob_events)).await,
        "alice/bob not ready; alice={:?} bob={:?}",
        alice_events.borrow(),
        bob_events.borrow()
    );

    let (carol, carol_events) = start_controller(carol_session);
    assert!(
        wait_until(20, || has_handshake_waiting(&carol_events)).await,
        "carol did not reach handshake wait; events={:?}",
        carol_events.borrow()
    );

    alice.invite_member(carol_pub.clone(), false);

    assert!(
        wait_until(120, || is_ready(&carol_events)).await,
        "carol not ready; events={:?}",
        carol_events.borrow()
    );
    assert!(
        wait_until(120, || roster_contains(&alice_events, &carol_pub)).await,
        "alice roster missing carol; events={:?}",
        alice_events.borrow()
    );
    assert!(
        wait_until(120, || roster_contains(&bob_events, &carol_pub)).await,
        "bob roster missing carol; events={:?}",
        bob_events.borrow()
    );

    carol.send_message("carol → everyone".to_string());

    assert!(
        wait_until(120, || {
            has_message(&alice_events, &carol_pub, "carol → everyone")
        })
        .await,
        "alice missing carol message; events={:?}",
        alice_events.borrow()
    );
    assert!(
        wait_until(120, || {
            has_message(&bob_events, &carol_pub, "carol → everyone")
        })
        .await,
        "bob missing carol message; events={:?}",
        bob_events.borrow()
    );

    alice.rotate_epoch();
    wait_for_commit("alice commit after carol", &alice_events).await;
    wait_for_commit("bob commit after carol", &bob_events).await;

    alice.shutdown();
    bob.shutdown();
    carol.shutdown();
}

fn start_controller(session: SessionParams) -> (WasmChatController, Rc<RefCell<Vec<ChatEvent>>>) {
    let events = Rc::new(RefCell::new(Vec::<ChatEvent>::new()));
    let events_ref = events.clone();
    let callback = wasm_bindgen::closure::Closure::wrap(Box::new(move |value: JsValue| {
        if let Ok(event) = swb::from_value::<ChatEvent>(value) {
            events_ref.borrow_mut().push(event);
        }
    }) as Box<dyn FnMut(JsValue)>);

    let session_js = swb::to_value(&session).expect("serialize session params");
    let controller =
        WasmChatController::start(session_js, callback.as_ref().clone()).expect("start controller");
    callback.forget();
    (controller, events)
}

fn is_ready(events: &Rc<RefCell<Vec<ChatEvent>>>) -> bool {
    events
        .borrow()
        .iter()
        .any(|event| matches!(event, ChatEvent::Ready { ready: true }))
}

fn has_handshake_waiting(events: &Rc<RefCell<Vec<ChatEvent>>>) -> bool {
    events.borrow().iter().any(|event| match event {
        ChatEvent::Handshake { phase } => matches!(
            phase,
            marmot_chat::controller::events::HandshakePhase::WaitingForWelcome
        ),
        _ => false,
    })
}

fn has_message(events: &Rc<RefCell<Vec<ChatEvent>>>, author: &str, content: &str) -> bool {
    events.borrow().iter().any(|event| match event {
        ChatEvent::Message {
            author: evt_author,
            content: evt_content,
            ..
        } => evt_author == author && evt_content == content,
        _ => false,
    })
}

fn has_commit(events: &Rc<RefCell<Vec<ChatEvent>>>) -> bool {
    events
        .borrow()
        .iter()
        .any(|event| matches!(event, ChatEvent::Commit { .. }))
}

fn roster_contains(events: &Rc<RefCell<Vec<ChatEvent>>>, target: &str) -> bool {
    events.borrow().iter().any(|event| match event {
        ChatEvent::Roster { members } => members.iter().any(|member| member.pubkey == target),
        ChatEvent::MemberJoined { member } => member.pubkey == target,
        ChatEvent::MemberUpdated { member } => member.pubkey == target,
        _ => false,
    })
}

async fn wait_for_commit(label: &str, events: &Rc<RefCell<Vec<ChatEvent>>>) {
    for _ in 0..120 {
        if has_commit(events) {
            return;
        }
        TimeoutFuture::new(20).await;
    }
    panic!("{label} not observed; events={:?}", events.borrow());
}

async fn wait_until<F>(attempts: usize, mut condition: F) -> bool
where
    F: FnMut() -> bool,
{
    for _ in 0..attempts {
        if condition() {
            return true;
        }
        TimeoutFuture::new(20).await;
    }
    condition()
}

fn unique_session_id(label: &str) -> String {
    let random = (js_sys::Math::random() * 1_000_000.0) as u64;
    format!("{label}-{random}-{}", js_sys::Date::now() as u64)
}
