// =====================================================
// Utility helpers
// =====================================================

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use js_sys::{Function, Object, Reflect, Uint8Array};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_wasm_bindgen as swb;

use nostr::prelude::*;
use nostr::JsonUtil;

use mdk_core::{groups::NostrGroupConfigData, messages::MessageProcessingResult, MDK};
use mdk_memory_storage::MdkMemoryStorage;
use mdk_storage_traits::{groups::types::Group, GroupId};
use openmls::prelude::{KeyPackageBundle, OpenMlsProvider};
use openmls_traits::storage::StorageProvider;

use crate::controller::events::{ChatEvent, SessionParams, SessionRole};
use crate::controller::services::{IdentityService, MoqListener, MoqService, NostrService};
use crate::controller::{ChatController, ControllerConfig};

use super::identity::{js_error, MOQ_BRIDGE_KEY};

pub(super) fn get_moq_bridge() -> Result<JsValue, JsValue> {
    let global = js_sys::global();
    Reflect::get(&global, &JsValue::from_str(MOQ_BRIDGE_KEY))
}

pub(super) fn get_bridge_method(target: &JsValue, name: &str) -> Result<Function, JsValue> {
    Reflect::get(target, &JsValue::from_str(name))?.dyn_into()
}
thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

#[derive(Default)]
struct Registry {
    next_id: u32,
    identities: HashMap<u32, LegacyIdentity>,
}

pub(super) struct LegacyIdentity {
    pub(super) keys: Keys,
    pub(super) mdk: MDK<MdkMemoryStorage>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GroupCreateResult {
    group_id_hex: String,
    nostr_group_id: String,
    welcome: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct KeyPackageResult {
    event: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AcceptWelcomeResult {
    group_id_hex: String,
    nostr_group_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExportedKeyPackageBundle {
    event: String,
    bundle: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SelfUpdateResult {
    evolution_event: String,
    welcome: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProcessedWrapper {
    kind: String,
    message: Option<DecryptedMessage>,
    proposal: Option<ProposalEnvelope>,
    commit: Option<CommitEnvelope>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DecryptedMessage {
    content: String,
    author: String,
    created_at: u64,
    event: JsonValue,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProposalEnvelope {
    event: String,
    welcome: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CommitEnvelope {
    event: String,
}

#[derive(Debug, Serialize, Deserialize)]
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
struct CreateMessageInput {
    group_id_hex: String,
    rumor: JsonValue,
}

pub(super) fn with_identity<F, R>(id: u32, f: F) -> Result<R, JsValue>
where
    F: FnOnce(&mut LegacyIdentity) -> Result<R, JsValue>,
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

pub(super) fn decode_hex(bytes_hex: &str) -> Result<Vec<u8>, JsValue> {
    hex::decode(bytes_hex).map_err(|e| js_error(format!("invalid hex: {e}")))
}

fn parse_public_keys(keys: &[String]) -> Result<Vec<PublicKey>, JsValue> {
    keys.iter()
        .map(|k| PublicKey::from_hex(k).map_err(|e| js_error(format!("invalid pubkey: {e}"))))
        .collect()
}

#[wasm_bindgen]
pub fn create_identity(secret_hex: String) -> Result<u32, JsValue> {
    let secret =
        SecretKey::from_hex(&secret_hex).map_err(|e| js_error(format!("invalid secret: {e}")))?;
    let keys = Keys::new(secret);
    let identity = LegacyIdentity {
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
pub fn create_message(identity_id: u32, payload: JsValue) -> Result<Uint8Array, JsValue> {
    with_identity(identity_id, |identity| {
        let input: CreateMessageInput = swb::from_value(payload)
            .map_err(|e| js_error(format!("invalid message payload: {e}")))?;
        let rumor_bytes = serde_json::to_vec(&input.rumor)
            .map_err(|e| js_error(format!("failed to serialize rumor: {e}")))?;
        let rumor = UnsignedEvent::from_json(&rumor_bytes)
            .map_err(|e| js_error(format!("failed to parse rumor: {e}")))?;
        let wrapper = identity
            .mdk
            .create_message(
                &GroupId::from_slice(&decode_hex(&input.group_id_hex)?),
                rumor.into(),
            )
            .map_err(|e| js_error(format!("failed to create message: {e}")))?;
        Ok(Uint8Array::from(wrapper.as_json().as_bytes()))
    })
}
