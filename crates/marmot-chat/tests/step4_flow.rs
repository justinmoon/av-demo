#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use marmot_chat::controller::events::{
    ChatEvent, SessionParams, SessionRole, StubConfig, StubWrapper,
};
use marmot_chat::controller::services::IdentityService;
use marmot_chat::messages::{WrapperFrame, WrapperKind};

#[path = "support/mod.rs"]
mod support;

use marmot_chat::WasmChatController;
use support::scenario::{DeterministicScenario, CREATOR_SECRET, INVITEE_SECRET};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

use anyhow::{anyhow, Result};
use gloo_timers::future::TimeoutFuture;
use serde_wasm_bindgen as swb;

const THIRD_SECRET: &str = "9c4e9aba1e3ff5deaa1bcb2a7dce1f2f4a5c6d7e8f9a0b1c2d3e4f5061728394";
const RELAY_ENDPOINT: &str = "wss://relay.example.com";
const RELAY_STUB_URL: &str = "stub://relay";
const NOSTR_STUB_URL: &str = "stub://nostr";

struct ThreeMemberFixture {
    session: SessionParams,
    expected_messages: Vec<String>,
    expected_commits: usize,
    member_pubkeys: Vec<String>,
    admin_pubkeys: Vec<String>,
    new_member_pubkey: String,
}

impl ThreeMemberFixture {
    fn build() -> Result<Self> {
        let relays = vec![RELAY_ENDPOINT.to_string()];

        let alice = IdentityService::create(CREATOR_SECRET)?;
        let bob = IdentityService::create(INVITEE_SECRET)?;
        let carol = IdentityService::create(THIRD_SECRET)?;

        let alice_pub = alice.public_key_hex();
        let bob_pub = bob.public_key_hex();
        let carol_pub = carol.public_key_hex();

        let bob_export = bob.create_key_package(&relays)?;
        let group_artifacts =
            alice.create_group(&bob_export.event_json, &bob_pub, &[alice_pub.clone()])?;

        bob.import_key_package_bundle(&bob_export.bundle)?;
        let group_id_hex = bob.accept_welcome(&group_artifacts.welcome)?;

        let alice_msg1 = alice.create_message("Alice → Bob: before Carol joins")?;
        let alice_msg2 = alice.create_message("Alice: prepping to add Carol")?;

        let carol_export = carol.create_key_package(&relays)?;
        let add_artifacts = alice.add_members(&[carol_export.event_json.clone()])?;

        let welcome = add_artifacts
            .welcomes
            .first()
            .ok_or_else(|| anyhow!("missing welcome for new member"))?;

        carol.import_key_package_bundle(&carol_export.bundle)?;
        let carol_group_hex = carol.accept_welcome(&welcome.welcome)?;
        if carol_group_hex != group_id_hex {
            return Err(anyhow!("group id mismatch after Carol join"));
        }

        let carol_msg = carol.create_message("Carol: happy to join!")?;
        let alice_post = alice.create_message("Alice: welcome Carol")?;

        let frames: Vec<WrapperFrame> = vec![
            alice_msg1,
            alice_msg2,
            add_artifacts.commit.clone(),
            carol_msg,
            alice_post,
        ];

        let stub_backlog: Vec<StubWrapper> = frames.iter().map(stub_wrapper).collect();

        let expected_messages = frames
            .iter()
            .filter_map(|frame| match &frame.kind {
                WrapperKind::Application { content, .. } => Some(content.clone()),
                WrapperKind::Commit => None,
            })
            .collect::<Vec<_>>();

        let expected_commits = frames
            .iter()
            .filter(|frame| matches!(frame.kind, WrapperKind::Commit))
            .count();

        let stub = StubConfig {
            backlog: stub_backlog,
            welcome: Some(group_artifacts.welcome.clone()),
            key_package_bundle: Some(bob_export.bundle.clone()),
            key_package_event: Some(bob_export.event_json.clone()),
            group_id_hex: Some(group_artifacts.group_id_hex.clone()),
            pause_after_frames: None,
        };

        let session = SessionParams {
            bootstrap_role: SessionRole::Invitee,
            relay_url: RELAY_STUB_URL.to_string(),
            nostr_url: NOSTR_STUB_URL.to_string(),
            session_id: "three-member".to_string(),
            secret_hex: INVITEE_SECRET.to_string(),
            peer_pubkeys: vec![alice_pub.clone(), carol_pub.clone()],
            group_id_hex: Some(group_artifacts.group_id_hex.clone()),
            admin_pubkeys: vec![alice_pub.clone()],
            stub: Some(stub),
        };

        Ok(Self {
            session,
            expected_messages,
            expected_commits,
            member_pubkeys: vec![alice_pub.clone(), bob_pub.clone(), carol_pub.clone()],
            admin_pubkeys: vec![alice_pub],
            new_member_pubkey: carol_pub,
        })
    }
}

fn stub_wrapper(wrapper: &WrapperFrame) -> StubWrapper {
    StubWrapper {
        bytes: wrapper.bytes.clone(),
        label: wrapper.kind.label().to_string(),
    }
}

#[wasm_bindgen_test]
async fn bob_bootstrap_flow() {
    let mut scenario = DeterministicScenario::new().expect("deterministic scenario");
    let config = scenario.config.clone();
    let backlog_wrappers = scenario
        .conversation
        .initial_backlog()
        .expect("initial backlog wrappers");

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
        key_package_bundle: Some(config.invitee_key_package.bundle.clone()),
        key_package_event: Some(config.invitee_key_package.event_json.clone()),
        group_id_hex: Some(config.group_id_hex.clone()),
        pause_after_frames: None,
    };

    let session = SessionParams {
        bootstrap_role: SessionRole::Invitee,
        relay_url: "stub://relay".to_string(),
        nostr_url: "stub://nostr".to_string(),
        session_id: "phase4".to_string(),
        secret_hex: config.invitee_secret_hex.clone(),
        peer_pubkeys: vec![config.creator_pubkey.clone()],
        group_id_hex: Some(config.group_id_hex.clone()),
        admin_pubkeys: Vec::new(),
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

#[wasm_bindgen_test]
async fn three_member_backlog_flow() {
    let fixture = ThreeMemberFixture::build().expect("three member fixture");

    let events = Rc::new(RefCell::new(Vec::<ChatEvent>::new()));
    let events_ref = events.clone();
    let callback = Closure::wrap(Box::new(move |value: JsValue| {
        if let Ok(event) = swb::from_value::<ChatEvent>(value) {
            events_ref.borrow_mut().push(event);
        }
    }) as Box<dyn FnMut(JsValue)>);

    let session_js = swb::to_value(&fixture.session).expect("serialize session");
    let handle = WasmChatController::start(session_js, callback.as_ref().clone())
        .map_err(|err| err.as_string().unwrap_or_default())
        .expect("controller start");
    callback.forget();

    TimeoutFuture::new(80).await;

    handle.shutdown();

    let recorded = events.borrow();

    let messages: Vec<_> = recorded
        .iter()
        .filter_map(|event| match event {
            ChatEvent::Message { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        messages, fixture.expected_messages,
        "decrypted message backlog mismatch"
    );

    let commit_events = recorded
        .iter()
        .filter(|event| matches!(event, ChatEvent::Commit { .. }))
        .count();

    assert_eq!(
        commit_events, fixture.expected_commits,
        "commit count mismatch"
    );

    let roster_members = recorded
        .iter()
        .rev()
        .find_map(|event| match event {
            ChatEvent::Roster { members } => Some(members.clone()),
            _ => None,
        })
        .expect("roster event not emitted");

    let roster_pubkeys: HashSet<String> = roster_members
        .iter()
        .map(|member| member.pubkey.clone())
        .collect();
    let expected_pubkeys: HashSet<String> = fixture.member_pubkeys.iter().cloned().collect();
    assert_eq!(roster_pubkeys, expected_pubkeys, "roster members mismatch");

    for admin in &fixture.admin_pubkeys {
        let entry = roster_members
            .iter()
            .find(|member| &member.pubkey == admin)
            .expect("admin missing from roster");
        assert!(entry.is_admin, "expected admin {} flagged", admin);
    }

    let new_member_entry = roster_members
        .iter()
        .find(|member| member.pubkey == fixture.new_member_pubkey)
        .expect("new member missing from roster");
    assert!(!new_member_entry.is_admin, "new member should not be admin");

    let joined_pubkeys: Vec<String> = recorded
        .iter()
        .filter_map(|event| match event {
            ChatEvent::MemberJoined { member } => Some(member.pubkey.clone()),
            _ => None,
        })
        .collect();
    assert!(
        joined_pubkeys.contains(&fixture.new_member_pubkey),
        "missing member joined event for new participant"
    );
}
