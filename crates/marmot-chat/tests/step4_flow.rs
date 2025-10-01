#![cfg(target_arch = "wasm32")]

use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

use js_sys::Uint8Array;
use marmot_chat::scenario::{Phase4Scenario, WrapperKind};
use marmot_chat::{
    accept_welcome, create_identity, import_key_package_bundle, ingest_wrapper,
    merge_pending_commit,
};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_wasm_bindgen as swb;

#[derive(Deserialize)]
struct AcceptWelcomeOutput {
    group_id_hex: String,
}

#[derive(Deserialize)]
struct ProcessedWrapper {
    kind: String,
    message: Option<DecryptedMessage>,
}

#[derive(Deserialize)]
struct DecryptedMessage {
    author: String,
    content: String,
}

#[wasm_bindgen_test]
fn bob_bootstrap_flow() {
    let mut scenario = Phase4Scenario::new().expect("phase 4 fixture");
    let config = &scenario.config;

    let wrappers = scenario
        .conversation
        .initial_backlog()
        .expect("alice backlog wrappers");

    let identity = unwrap_identity(
        create_identity(config.bob_secret_hex.clone()),
        "create_identity",
    );

    expect_js_ok(
        import_key_package_bundle(identity, config.bob_key_package.bundle.clone()),
        "import_key_package_bundle",
    );

    let accept: AcceptWelcomeOutput = unwrap_js_value(
        accept_welcome(identity, config.welcome_json.clone()),
        "accept_welcome",
    );
    assert_eq!(accept.group_id_hex, config.group_id_hex);

    let expected_messages: Vec<String> = wrappers
        .iter()
        .filter_map(|wrapper| match &wrapper.kind {
            WrapperKind::Application { content, .. } => Some(content.clone()),
            WrapperKind::Commit => None,
        })
        .collect();

    let mut observed_messages = Vec::new();
    let mut processed_commit = false;

    for (index, wrapper) in wrappers.iter().enumerate() {
        let processed: ProcessedWrapper = unwrap_js_value(
            ingest_wrapper(identity, Uint8Array::from(wrapper.bytes.as_slice())),
            &format!("ingest_wrapper[{index}]"),
        );

        match &wrapper.kind {
            WrapperKind::Application { content, .. } => {
                assert_eq!(processed.kind, "application", "wrapper[{index}] kind");
                let message = processed
                    .message
                    .unwrap_or_else(|| panic!("wrapper[{index}] missing decrypted message"));
                assert_eq!(
                    message.author, config.alice_pubkey,
                    "wrapper[{index}] author"
                );
                assert_eq!(message.content, *content, "wrapper[{index}] content");
                observed_messages.push(message.content);
            }
            WrapperKind::Commit => {
                assert_eq!(processed.kind, "commit", "wrapper[{index}] kind");
                processed_commit = true;
                expect_js_ok(
                    merge_pending_commit(identity, config.group_id_hex.clone()),
                    &format!("merge_pending_commit[{index}]"),
                );
            }
        }
    }

    assert!(processed_commit, "commit wrapper was never processed");
    assert_eq!(
        observed_messages, expected_messages,
        "message ordering mismatch"
    );
}

fn unwrap_identity(result: Result<u32, JsValue>, ctx: &str) -> u32 {
    result.unwrap_or_else(|err| panic!("{ctx} failed: {}", decode_js_error(err)))
}

fn unwrap_js_value<T>(result: Result<JsValue, JsValue>, ctx: &str) -> T
where
    T: for<'de> Deserialize<'de>,
{
    let value = result.unwrap_or_else(|err| panic!("{ctx} failed: {}", decode_js_error(err)));
    swb::from_value(value).unwrap_or_else(|err| panic!("{ctx} decode failed: {err}"))
}

fn expect_js_ok(result: Result<(), JsValue>, ctx: &str) {
    result.unwrap_or_else(|err| panic!("{ctx} failed: {}", decode_js_error(err)));
}

fn decode_js_error(err: JsValue) -> String {
    if let Ok(value) = swb::from_value::<JsonValue>(err.clone()) {
        if let Some(msg) = value.get("error").and_then(|v| v.as_str()) {
            return msg.to_string();
        }
        return value.to_string();
    }
    if let Some(text) = err.as_string() {
        return text;
    }
    if let Ok(text) = js_sys::JSON::stringify(&err) {
        if let Some(s) = text.as_string() {
            return s;
        }
    }
    format!("{err:?}")
}
