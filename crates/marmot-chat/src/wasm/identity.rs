use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};

use js_sys::{Function, Object, Reflect, Uint8Array};
use serde_json::{json, Value as JsonValue};
use serde_wasm_bindgen as swb;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use web_sys::{BinaryType, ErrorEvent, MessageEvent, WebSocket};

use crate::controller::events::{ChatEvent, SessionParams, SessionRole};
use crate::controller::services::{
    HandshakeConnectParams, HandshakeListener, HandshakeMessage, HandshakeMessageBody,
    HandshakeMessageType, IdentityService, MoqListener, MoqService, NostrService,
};
use crate::controller::{ChatController, ControllerConfig};

use super::moq_bridge::JsMoqService;
use super::nostr_client::JsNostrService;
use mdk_core::{groups::NostrGroupConfigData, messages::MessageProcessingResult, MDK};
use mdk_memory_storage::MdkMemoryStorage;
use mdk_storage_traits::{groups::types::Group, GroupId};
use nostr::event::Tag;
use nostr::prelude::*;
use nostr::{JsonUtil, TagKind};
use openmls::prelude::{KeyPackageBundle, OpenMlsProvider};
use openmls_traits::storage::StorageProvider;

pub(super) const HANDSHAKE_KIND: u16 = 44501;
pub(super) const MOQ_BRIDGE_KEY: &str = "__MARMOT_MOQ__";

#[cfg(feature = "panic-hook")]
use console_error_panic_hook::set_once;

#[derive(Serialize)]
struct JsErrorPayload {
    error: String,
}

#[wasm_bindgen(start)]
pub fn wasm_start() {
    let _ = console_log::init_with_level(log::Level::Debug);
    #[cfg(feature = "panic-hook")]
    set_once();
}

#[wasm_bindgen]
pub struct WasmChatController {
    controller: ChatController,
    _callback: Rc<Function>,
}

#[wasm_bindgen]
impl WasmChatController {
    #[wasm_bindgen(js_name = start)]
    pub fn start(session: JsValue, callback: JsValue) -> Result<WasmChatController, JsValue> {
        let params: SessionParams = swb::from_value(session)
            .map_err(|err| js_error(format!("invalid session params: {err}")))?;
        let callback_fn: Function = callback
            .dyn_into()
            .map_err(|_| js_error("callback must be a function"))?;
        let callback_rc = Rc::new(callback_fn);

        let identity = IdentityService::create(&params.secret_hex).map_err(js_error)?;

        let (nostr, moq) = build_services(&params)?;

        let callback_emit = callback_rc.clone();
        let event_callback = Rc::new(move |event: ChatEvent| {
            if let Ok(value) = swb::to_value(&event) {
                let _ = callback_emit.call1(&JsValue::NULL, &value);
            }
        });

        let config = ControllerConfig {
            identity,
            session: params,
            nostr,
            moq,
            callback: event_callback,
        };

        let controller = ChatController::new(config);
        controller.start();

        Ok(Self {
            controller,
            _callback: callback_rc,
        })
    }

    pub fn send_message(&self, content: String) {
        self.controller.send_text(content);
    }

    pub fn rotate_epoch(&self) {
        self.controller.rotate_epoch();
    }

    pub fn shutdown(&self) {
        self.controller.shutdown();
    }

    #[wasm_bindgen(js_name = inviteMember)]
    pub fn invite_member(&self, pubkey: String, is_admin: bool) {
        self.controller.invite_member(pubkey, is_admin);
    }
}

pub(super) fn js_error<E: ToString>(err: E) -> JsValue {
    swb::to_value(&JsErrorPayload {
        error: err.to_string(),
    })
    .unwrap_or_else(|_| JsValue::from_str(&err.to_string()))
}

fn build_services(
    _params: &SessionParams,
) -> Result<(Rc<dyn NostrService>, Rc<dyn MoqService>), JsValue> {
    Ok((Rc::new(JsNostrService::new()), Rc::new(JsMoqService::new())))
}

