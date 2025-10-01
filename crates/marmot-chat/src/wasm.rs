use std::cell::RefCell;
use std::collections::HashMap;

use wasm_bindgen::prelude::*;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_wasm_bindgen as swb;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

use js_sys::Uint8Array;

use nostr::prelude::*;
use nostr::util::JsonUtil;

use mdk_core::{groups::NostrGroupConfigData, groups::UpdateGroupResult};
use mdk_core::{messages::MessageProcessingResult, MDK};
use mdk_memory_storage::MdkMemoryStorage;
use mdk_storage_traits::{groups::types::Group, GroupId};
use openmls::prelude::{KeyPackageBundle, OpenMlsProvider};
use openmls_traits::storage::StorageProvider;

#[cfg(feature = "panic-hook")]
use console_error_panic_hook::set_once;

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

#[derive(Default)]
#[allow(dead_code)]
struct Registry {
    next_id: u32,
    identities: HashMap<u32, Identity>,
}
struct Identity {
    keys: Keys,
    mdk: MDK<MdkMemoryStorage>,
}

#[derive(Debug, Serialize)]
struct JsErrorPayload {
    error: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct GroupCreateResult {
    group_id_hex: String,
    nostr_group_id: String,
    welcome: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct KeyPackageResult {
    event: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct AcceptWelcomeResult {
    group_id_hex: String,
    nostr_group_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct ExportedKeyPackageBundle {
    event: String,
    bundle: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct SelfUpdateResult {
    evolution_event: String,
    welcome: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct ProcessedWrapper {
    kind: String,
    message: Option<DecryptedMessage>,
    proposal: Option<ProposalEnvelope>,
    commit: Option<CommitEnvelope>,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct DecryptedMessage {
    content: String,
    author: String,
    created_at: u64,
    event: JsonValue,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct ProposalEnvelope {
    event: String,
    welcome: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct CommitEnvelope {
    event: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct GroupInfo {
    group_id_hex: String,
    nostr_group_id: String,
    member_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct GroupConfigInput {
    name: String,
    description: String,
    relays: Vec<String>,
    admins: Vec<String>,
    #[serde(default)]
    image_hash: Option<String>,
    #[serde(default)]
    image_key: Option<String>,
    #[serde(default)]
    image_nonce: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
struct CreateMessageInput {
    group_id_hex: String,
    rumor: JsonValue,
}

fn js_error<E: std::fmt::Display>(err: E) -> JsValue {
    swb::to_value(&JsErrorPayload {
        error: err.to_string(),
    })
    .unwrap_or_else(|_| JsValue::from_str(&err.to_string()))
}
fn with_identity<F, R>(id: u32, f: F) -> Result<R, JsValue>
where
    F: FnOnce(&mut Identity) -> Result<R, JsValue>,
{
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let identity = registry
            .identities
            .get_mut(&id)
            .ok_or_else(|| js_error(format!("unknown identity id {id}")))?;
        f(identity)
    })
}

fn decode_hex(bytes_hex: &str) -> Result<Vec<u8>, JsValue> {
    hex::decode(bytes_hex).map_err(|e| js_error(format!("invalid hex: {e}")))
}

fn parse_public_keys(keys: &[String]) -> Result<Vec<PublicKey>, JsValue> {
    keys.iter()
        .map(|k| PublicKey::from_hex(k).map_err(|e| js_error(format!("invalid pubkey: {e}"))))
        .collect()
}

#[wasm_bindgen(start)]
pub fn wasm_start() {
    #[cfg(feature = "panic-hook")]
    set_once();
}

#[wasm_bindgen]
pub fn create_identity(secret_hex: String) -> Result<u32, JsValue> {
    let secret =
        SecretKey::from_hex(&secret_hex).map_err(|e| js_error(format!("invalid secret: {e}")))?;
    let keys = Keys::new(secret);
    let identity = Identity {
        keys,
        mdk: MDK::new(MdkMemoryStorage::default()),
    };

    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let id = registry.next_id;
        registry.next_id += 1;
        registry.identities.insert(id, identity);
        Ok(id)
    })
}

#[wasm_bindgen]
pub fn public_key(identity_id: u32) -> Result<String, JsValue> {
    with_identity(identity_id, |identity| {
        Ok(identity.keys.public_key().to_hex())
    })
}

#[wasm_bindgen]
pub fn create_key_package(identity_id: u32, relays: JsValue) -> Result<JsValue, JsValue> {
    with_identity(identity_id, |identity| {
        let relay_urls: Vec<String> = swb::from_value(relays)
            .map_err(|e| js_error(format!("invalid relays payload: {e}")))?;

        let parsed_relays: Result<Vec<RelayUrl>, _> = relay_urls
            .iter()
            .map(|url| RelayUrl::parse(url).map_err(|e| js_error(format!("invalid relay: {e}"))))
            .collect();

        let parsed_relays = parsed_relays?;

        let (encoded, tags) = identity
            .mdk
            .create_key_package_for_event(&identity.keys.public_key(), parsed_relays)
            .map_err(|e| js_error(format!("failed to create key package: {e}")))?;

        let event = EventBuilder::new(Kind::MlsKeyPackage, encoded)
            .tags(tags)
            .build(identity.keys.public_key())
            .sign_with_keys(&identity.keys)
            .map_err(|e| js_error(format!("failed to sign key package: {e}")))?;

        let result = KeyPackageResult {
            event: event.as_json(),
        };
        swb::to_value(&result)
            .map_err(|e| js_error(format!("failed to serialize key package: {e}")))
    })
}

#[wasm_bindgen]
pub fn export_key_package_bundle(identity_id: u32, event_json: String) -> Result<JsValue, JsValue> {
    with_identity(identity_id, |identity| {
        let event = Event::from_json(&event_json)
            .map_err(|e| js_error(format!("invalid key package event: {e}")))?;
        let key_package = identity
            .mdk
            .parse_key_package(&event)
            .map_err(|e| js_error(format!("failed to parse key package: {e}")))?;
        let hash_ref = key_package
            .hash_ref(identity.mdk.provider.crypto())
            .map_err(|e| js_error(format!("failed to derive key package reference: {e}")))?;
        let bundle = identity
            .mdk
            .provider
            .storage()
            .key_package::<_, KeyPackageBundle>(&hash_ref)
            .map_err(|e| js_error(format!("failed to load key package bundle: {:?}", e)))?
            .ok_or_else(|| js_error("key package bundle not found"))?;

        let bundle_bytes = serde_json::to_vec(&bundle)
            .map_err(|e| js_error(format!("failed to serialize key package bundle: {e}")))?;
        let encoded = BASE64.encode(bundle_bytes);

        let export = ExportedKeyPackageBundle {
            event: event_json,
            bundle: encoded,
        };

        swb::to_value(&export).map_err(|e| {
            js_error(format!(
                "failed to serialize exported key package bundle: {e}"
            ))
        })
    })
}

#[wasm_bindgen]
pub fn import_key_package_bundle(identity_id: u32, bundle_b64: String) -> Result<(), JsValue> {
    with_identity(identity_id, |identity| {
        let bundle_bytes = BASE64
            .decode(bundle_b64)
            .map_err(|e| js_error(format!("invalid key package bundle encoding: {e}")))?;
        let bundle: KeyPackageBundle = serde_json::from_slice(&bundle_bytes)
            .map_err(|e| js_error(format!("failed to parse key package bundle: {e}")))?;
        let hash_ref = bundle
            .key_package()
            .hash_ref(identity.mdk.provider.crypto())
            .map_err(|e| js_error(format!("failed to derive key package reference: {e}")))?;

        identity
            .mdk
            .provider
            .storage()
            .write_key_package::<_, KeyPackageBundle>(&hash_ref, &bundle)
            .map_err(|e| js_error(format!("failed to store key package bundle: {:?}", e)))?;

        Ok(())
    })
}

#[wasm_bindgen]
pub fn create_group(
    identity_id: u32,
    config: JsValue,
    member_events: JsValue,
) -> Result<JsValue, JsValue> {
    with_identity(identity_id, |identity| {
        let config: GroupConfigInput =
            swb::from_value(config).map_err(|e| js_error(format!("invalid group config: {e}")))?;
        let member_events: Vec<String> = swb::from_value(member_events)
            .map_err(|e| js_error(format!("invalid member events: {e}")))?;

        let relays: Result<Vec<RelayUrl>, _> = config
            .relays
            .iter()
            .map(|url| RelayUrl::parse(url).map_err(|e| js_error(format!("invalid relay: {e}"))))
            .collect();
        let relays = relays?;

        let admins = parse_public_keys(&config.admins)?;

        let mut image_hash_bytes = None;
        if let Some(ref hash_hex) = config.image_hash {
            image_hash_bytes = Some(
                hex::decode(hash_hex).map_err(|e| js_error(format!("invalid image hash: {e}")))?,
            );
        }
        let mut image_key_bytes = None;
        if let Some(ref key_hex) = config.image_key {
            image_key_bytes = Some(
                hex::decode(key_hex).map_err(|e| js_error(format!("invalid image key: {e}")))?,
            );
        }
        let mut image_nonce_bytes = None;
        if let Some(ref nonce_hex) = config.image_nonce {
            image_nonce_bytes = Some(
                hex::decode(nonce_hex)
                    .map_err(|e| js_error(format!("invalid image nonce: {e}")))?,
            );
        }

        let to_array32 =
            |bytes: Option<Vec<u8>>, name: &str| -> Result<Option<[u8; 32]>, JsValue> {
                if let Some(bytes) = bytes {
                    let arr: [u8; 32] = bytes
                        .try_into()
                        .map_err(|_| js_error(format!("{name} must be 32 bytes")))?;
                    Ok(Some(arr))
                } else {
                    Ok(None)
                }
            };
        let to_array12 =
            |bytes: Option<Vec<u8>>, name: &str| -> Result<Option<[u8; 12]>, JsValue> {
                if let Some(bytes) = bytes {
                    let arr: [u8; 12] = bytes
                        .try_into()
                        .map_err(|_| js_error(format!("{name} must be 12 bytes")))?;
                    Ok(Some(arr))
                } else {
                    Ok(None)
                }
            };

        let image_hash = to_array32(image_hash_bytes, "image hash")?;
        let image_key = to_array32(image_key_bytes, "image key")?;
        let image_nonce = to_array12(image_nonce_bytes, "image nonce")?;

        let cfg = NostrGroupConfigData::new(
            config.name,
            config.description,
            image_hash,
            image_key,
            image_nonce,
            relays,
            admins,
        );

        let members: Result<Vec<Event>, JsValue> = member_events
            .iter()
            .map(|raw| {
                Event::from_json(raw).map_err(|e| js_error(format!("invalid member event: {e}")))
            })
            .collect();
        let members = members?;

        let group_result = identity
            .mdk
            .create_group(&identity.keys.public_key(), members, cfg)
            .map_err(|e| js_error(format!("failed to create group: {e}")))?;

        let welcome_json = group_result
            .welcome_rumors
            .iter()
            .map(|unsigned| unsigned.as_json())
            .collect();

        let resp = GroupCreateResult {
            group_id_hex: hex::encode(group_result.group.mls_group_id.as_slice()),
            nostr_group_id: hex::encode(group_result.group.nostr_group_id),
            welcome: welcome_json,
        };
        swb::to_value(&resp).map_err(|e| js_error(format!("failed to serialize group result: {e}")))
    })
}

#[wasm_bindgen]
pub fn accept_welcome(identity_id: u32, welcome_json: String) -> Result<JsValue, JsValue> {
    with_identity(identity_id, |identity| {
        let welcome_unsigned = UnsignedEvent::from_json(welcome_json.as_bytes())
            .map_err(|e| js_error(format!("invalid welcome event: {e}")))?;

        identity
            .mdk
            .process_welcome(&EventId::all_zeros(), &welcome_unsigned)
            .map_err(|e| js_error(format!("failed to process welcome: {e}")))?;

        let mut accepted_group: Option<Group> = None;

        if let Ok(mut welcomes) = identity.mdk.get_pending_welcomes() {
            for welcome in welcomes.iter() {
                if let Err(err) = identity.mdk.accept_welcome(welcome) {
                    return Err(js_error(format!("failed to accept welcome: {err}")));
                }
            }
            if let Some(latest) = welcomes.pop() {
                if let Ok(groups) = identity.mdk.get_groups() {
                    accepted_group = groups
                        .into_iter()
                        .find(|g| g.mls_group_id == latest.mls_group_id);
                }
            }
        }

        let group =
            accepted_group.ok_or_else(|| js_error("accepted welcome but group not found"))?;

        let resp = AcceptWelcomeResult {
            group_id_hex: hex::encode(group.mls_group_id.as_slice()),
            nostr_group_id: hex::encode(group.nostr_group_id),
        };
        swb::to_value(&resp)
            .map_err(|e| js_error(format!("failed to serialize accept result: {e}")))
    })
}

#[wasm_bindgen]
pub fn list_groups(identity_id: u32) -> Result<JsValue, JsValue> {
    with_identity(identity_id, |identity| {
        let groups = identity
            .mdk
            .get_groups()
            .map_err(|e| js_error(format!("failed to fetch groups: {e}")))?;

        let infos: Vec<GroupInfo> = groups
            .iter()
            .map(|g| GroupInfo {
                group_id_hex: hex::encode(g.mls_group_id.as_slice()),
                nostr_group_id: hex::encode(g.nostr_group_id),
                member_count: identity
                    .mdk
                    .get_members(&g.mls_group_id)
                    .map(|m| m.len())
                    .unwrap_or(0),
            })
            .collect();

        swb::to_value(&infos).map_err(|e| js_error(format!("failed to serialize groups: {e}")))
    })
}

#[wasm_bindgen]
pub fn create_message(identity_id: u32, payload: JsValue) -> Result<Uint8Array, JsValue> {
    with_identity(identity_id, |identity| {
        let payload: CreateMessageInput = swb::from_value(payload)
            .map_err(|e| js_error(format!("invalid message payload: {e}")))?;

        let group_id_bytes = decode_hex(&payload.group_id_hex)?;
        let group_id = GroupId::from_slice(&group_id_bytes);

        let rumor_json = serde_json::to_string(&payload.rumor)
            .map_err(|e| js_error(format!("failed to serialize rumor: {e}")))?;
        let mut rumor = UnsignedEvent::from_json(rumor_json.as_bytes())
            .map_err(|e| js_error(format!("invalid rumor json: {e}")))?;
        rumor.ensure_id();

        let event = identity
            .mdk
            .create_message(&group_id, rumor)
            .map_err(|e| js_error(format!("failed to create message: {e}")))?;

        let json = event.as_json();
        Ok(Uint8Array::from(json.as_bytes()))
    })
}

#[wasm_bindgen]
pub fn ingest_wrapper(identity_id: u32, wrapper: Uint8Array) -> Result<JsValue, JsValue> {
    with_identity(identity_id, |identity| {
        let json = String::from_utf8(wrapper.to_vec())
            .map_err(|e| js_error(format!("wrapper is not valid UTF-8: {e}")))?;
        let event = nostr::Event::from_json(&json)
            .map_err(|e| js_error(format!("invalid wrapper event: {e}")))?;

        let result = identity
            .mdk
            .process_message(&event)
            .map_err(|e| js_error(format!("failed to process wrapper: {e}")))?;

        let response = match result {
            MessageProcessingResult::ApplicationMessage(message) => {
                let event_json = message.event.as_json();
                ProcessedWrapper {
                    kind: "application".to_string(),
                    message: Some(DecryptedMessage {
                        content: message.content,
                        author: message.pubkey.to_hex(),
                        created_at: message.created_at.as_u64(),
                        event: serde_json::from_str(&event_json).unwrap_or(JsonValue::Null),
                    }),
                    proposal: None,
                    commit: None,
                }
            }
            MessageProcessingResult::Proposal(UpdateGroupResult {
                evolution_event,
                welcome_rumors,
            }) => {
                let welcomes =
                    welcome_rumors.map(|rumors| rumors.iter().map(|r| r.as_json()).collect());
                ProcessedWrapper {
                    kind: "proposal".to_string(),
                    message: None,
                    proposal: Some(ProposalEnvelope {
                        event: evolution_event.as_json(),
                        welcome: welcomes,
                    }),
                    commit: None,
                }
            }
            MessageProcessingResult::Commit => ProcessedWrapper {
                kind: "commit".to_string(),
                message: None,
                proposal: None,
                commit: Some(CommitEnvelope {
                    event: event.as_json(),
                }),
            },
            MessageProcessingResult::ExternalJoinProposal => ProcessedWrapper {
                kind: "external".to_string(),
                message: None,
                proposal: None,
                commit: None,
            },
            MessageProcessingResult::Unprocessable => ProcessedWrapper {
                kind: "unprocessable".to_string(),
                message: None,
                proposal: None,
                commit: None,
            },
        };

        swb::to_value(&response)
            .map_err(|e| js_error(format!("failed to serialize processing result: {e}")))
    })
}

#[wasm_bindgen]
pub fn self_update(identity_id: u32, group_id_hex: String) -> Result<JsValue, JsValue> {
    with_identity(identity_id, |identity| {
        let group_id_bytes = decode_hex(&group_id_hex)?;
        let group_id = GroupId::from_slice(&group_id_bytes);

        let update = identity
            .mdk
            .self_update(&group_id)
            .map_err(|e| js_error(format!("failed to self-update: {e}")))?;

        let welcome = update
            .welcome_rumors
            .map(|rumors| rumors.iter().map(|r| r.as_json()).collect());
        let resp = SelfUpdateResult {
            evolution_event: update.evolution_event.as_json(),
            welcome,
        };
        swb::to_value(&resp).map_err(|e| js_error(format!("failed to serialize self update: {e}")))
    })
}

#[wasm_bindgen]
pub fn merge_pending_commit(identity_id: u32, group_id_hex: String) -> Result<(), JsValue> {
    with_identity(identity_id, |identity| {
        let group_id_bytes = decode_hex(&group_id_hex)?;
        let group_id = GroupId::from_slice(&group_id_bytes);
        identity
            .mdk
            .merge_pending_commit(&group_id)
            .map_err(|e| js_error(format!("failed to merge pending commit: {e}")))?;
        Ok(())
    })
}

#[wasm_bindgen]
pub fn init(user_secret_hex: String) -> Result<u32, JsValue> {
    create_identity(user_secret_hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    const ALICE_SECRET: &str = "4d36e7068b0eeef39b4e2ff1f908db8b27c12075b1219777084ffcf86490b6ae";
    const BOB_SECRET: &str = "6e8a52c9ac36ca5293b156d8af4d7f6aeb52208419bd99c75472fc6f4321a5fd";

    #[derive(Deserialize)]
    struct WrappedProcessed {
        kind: String,
        message: Option<DecryptedMessage>,
    }

    #[wasm_bindgen_test]
    fn two_client_round_trip() {
        let alice = create_identity(ALICE_SECRET.into()).expect("failed to create alice");
        let bob = create_identity(BOB_SECRET.into()).expect("failed to create bob");

        let relays = swb::to_value(&vec!["wss://relay.example.com".to_string()]).unwrap();
        let key_pkg_val = create_key_package(bob, relays).expect("bob key package");
        let key_pkg: KeyPackageResult = swb::from_value(key_pkg_val).unwrap();

        let alice_pub = public_key(alice).unwrap();
        let bob_pub = public_key(bob).unwrap();

        let config = GroupConfigInput {
            name: "Demo Chat".into(),
            description: "Test conversation".into(),
            relays: vec!["wss://relay.example.com".into()],
            admins: vec![alice_pub.clone(), bob_pub.clone()],
            image_hash: None,
            image_key: None,
            image_nonce: None,
        };
        let members = vec![key_pkg.event];
        let group_val = create_group(
            alice,
            swb::to_value(&config).unwrap(),
            swb::to_value(&members).unwrap(),
        )
        .expect("create group");
        let group_resp: GroupCreateResult = swb::from_value(group_val).unwrap();

        assert_eq!(group_resp.welcome.len(), 1);
        let accept_val =
            accept_welcome(bob, group_resp.welcome[0].clone()).expect("bob accept welcome");
        let accept_resp: AcceptWelcomeResult = swb::from_value(accept_val).unwrap();
        assert_eq!(accept_resp.group_id_hex, group_resp.group_id_hex);

        let rumor = EventBuilder::new(Kind::TextNote, "hello from alice")
            .build(PublicKey::from_hex(&alice_pub).unwrap());
        let rumor_json: JsonValue = serde_json::from_str(&rumor.as_json()).unwrap();
        let msg_payload = CreateMessageInput {
            group_id_hex: group_resp.group_id_hex.clone(),
            rumor: rumor_json,
        };
        let wrapper = create_message(alice, swb::to_value(&msg_payload).unwrap()).unwrap();
        let bob_result_val = ingest_wrapper(bob, wrapper).expect("bob ingest");
        let bob_result: WrappedProcessed = swb::from_value(bob_result_val).unwrap();
        assert_eq!(bob_result.kind, "application");
        let message = bob_result.message.expect("bob decrypted message");
        assert_eq!(message.content, "hello from alice");
        assert_eq!(message.author, alice_pub);

        let update_val = self_update(alice, group_resp.group_id_hex.clone()).expect("self update");
        let update_resp: SelfUpdateResult = swb::from_value(update_val).unwrap();
        let commit_json = update_resp.evolution_event.clone();
        let alice_commit = Uint8Array::from(commit_json.as_bytes());
        let _ = ingest_wrapper(alice, alice_commit).expect("alice ingest commit");
        let commit_processed = ingest_wrapper(bob, Uint8Array::from(commit_json.as_bytes()))
            .expect("bob ingest commit");
        let commit_resp: WrappedProcessed = swb::from_value(commit_processed).unwrap();
        assert_eq!(commit_resp.kind, "commit");

        merge_pending_commit(alice, group_resp.group_id_hex.clone())
            .expect("alice merge pending commit");
        merge_pending_commit(bob, group_resp.group_id_hex.clone())
            .expect("bob merge pending commit");

        let reply_rumor = EventBuilder::new(Kind::TextNote, "hi alice")
            .build(PublicKey::from_hex(&bob_pub).unwrap());
        let reply_json: JsonValue = serde_json::from_str(&reply_rumor.as_json()).unwrap();
        let reply_payload = CreateMessageInput {
            group_id_hex: group_resp.group_id_hex.clone(),
            rumor: reply_json,
        };
        let reply_wrapper = create_message(bob, swb::to_value(&reply_payload).unwrap()).unwrap();
        let alice_processed = ingest_wrapper(alice, reply_wrapper).expect("alice ingests");
        let alice_resp: WrappedProcessed = swb::from_value(alice_processed).unwrap();
        assert_eq!(alice_resp.kind, "application");
        let alice_msg = alice_resp.message.expect("alice got message");
        assert_eq!(alice_msg.content, "hi alice");
        assert_eq!(alice_msg.author, bob_pub);
    }
}
