use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};

use js_sys::{Function, Object, Reflect, Uint8Array};
use serde_wasm_bindgen as swb;
use serde_json::{json, Value as JsonValue};

use serde::{Deserialize, Serialize};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use web_sys::{BinaryType, ErrorEvent, MessageEvent, WebSocket};

use crate::controller::events::{ChatEvent, Role, SessionParams};
use crate::controller::services::{
    stub, HandshakeConnectParams, HandshakeListener, HandshakeMessage, HandshakeMessageBody,
    HandshakeMessageType, IdentityService, MoqListener, MoqService, NostrService,
};
use crate::controller::{ChatController, ControllerConfig};
use nostr::event::Tag;
use nostr::prelude::*;
use nostr::{JsonUtil, TagKind};
use mdk_core::{groups::NostrGroupConfigData, messages::MessageProcessingResult, MDK};
use mdk_memory_storage::MdkMemoryStorage;
use mdk_storage_traits::{groups::types::Group, GroupId};
use openmls::prelude::{KeyPackageBundle, OpenMlsProvider};
use openmls_traits::storage::StorageProvider;

const HANDSHAKE_KIND: u16 = 44501;
const MOQ_BRIDGE_KEY: &str = "__MARMOT_MOQ__";

#[cfg(feature = "panic-hook")]
use console_error_panic_hook::set_once;

#[derive(Serialize)]
struct JsErrorPayload {
    error: String,
}

#[wasm_bindgen(start)]
pub fn wasm_start() {
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
}

fn js_error<E: ToString>(err: E) -> JsValue {
    swb::to_value(&JsErrorPayload {
        error: err.to_string(),
    })
    .unwrap_or_else(|_| JsValue::from_str(&err.to_string()))
}

fn build_services(
    params: &SessionParams,
) -> Result<(Rc<dyn NostrService>, Rc<dyn MoqService>), JsValue> {
    if params.stub.is_some() {
        Ok(stub::make_stub_services(params))
    } else {
        Ok((Rc::new(JsNostrService::new()), Rc::new(JsMoqService::new())))
    }
}

// =====================================================
// Nostr service (handshake over websocket)
// =====================================================

struct JsNostrService {
    state: Rc<JsNostrState>,
}

struct JsNostrState {
    socket: RefCell<Option<WebSocket>>,
    listener: RefCell<Option<Box<dyn HandshakeListener>>>,
    keys: RefCell<Option<Keys>>,
    session: RefCell<Option<String>>,
    url: RefCell<Option<String>>,
    role: RefCell<Role>,
    on_message: RefCell<Option<Closure<dyn FnMut(MessageEvent)>>>,
    on_open: RefCell<Option<Closure<dyn FnMut(JsValue)>>>,
    on_error: RefCell<Option<Closure<dyn FnMut(ErrorEvent)>>>,
    pending: RefCell<VecDeque<HandshakeMessage>>,
}

impl JsNostrService {
    fn new() -> Self {
        Self {
            state: Rc::new(JsNostrState {
                socket: RefCell::new(None),
                listener: RefCell::new(None),
                keys: RefCell::new(None),
                session: RefCell::new(None),
                url: RefCell::new(None),
                role: RefCell::new(Role::Alice),
                on_message: RefCell::new(None),
                on_open: RefCell::new(None),
                on_error: RefCell::new(None),
                pending: RefCell::new(VecDeque::new()),
            }),
        }
    }
}

impl NostrService for JsNostrService {
    fn connect(&self, params: HandshakeConnectParams, listener: Box<dyn HandshakeListener>) {
        JsNostrState::connect_rc(self.state.clone(), params, listener);
    }

    fn send(&self, payload: HandshakeMessage) {
        JsNostrState::send_rc(&self.state, payload);
    }

    fn shutdown(&self) {
        JsNostrState::shutdown_rc(&self.state);
    }
}

impl JsNostrState {
    fn connect_rc(state: Rc<JsNostrState>, params: HandshakeConnectParams, listener: Box<dyn HandshakeListener>) {
        *state.listener.borrow_mut() = Some(listener);
        *state.role.borrow_mut() = params.role;
        *state.session.borrow_mut() = Some(params.session.clone());
        *state.url.borrow_mut() = Some(params.url.clone());
        state.pending.borrow_mut().clear();

        match SecretKey::from_hex(&params.secret_hex).map(Keys::new) {
            Ok(keys) => {
                *state.keys.borrow_mut() = Some(keys);
            }
            Err(err) => {
                log::error!("invalid handshake secret: {err}");
                return;
            }
        }

        match WebSocket::new(&params.url) {
            Ok(socket) => {
                let _ = socket.set_binary_type(BinaryType::Arraybuffer);
                JsNostrState::install_handlers(state.clone(), &socket);
                *state.socket.borrow_mut() = Some(socket);
            }
            Err(err) => {
                log::error!("failed to open handshake websocket: {:?}", err);
            }
        }
    }

    fn send_rc(state: &Rc<JsNostrState>, payload: HandshakeMessage) {
        if !JsNostrState::is_socket_open(state) {
            state.pending.borrow_mut().push_back(payload);
            return;
        }
        if let Err(err) = JsNostrState::send_now(state, &payload) {
            log::error!("failed to send handshake event: {:?}", err);
            state.pending.borrow_mut().push_back(payload);
            return;
        }
        JsNostrState::flush_pending(state);
    }

    fn is_socket_open(state: &Rc<JsNostrState>) -> bool {
        match state.socket.borrow().as_ref() {
            Some(socket) => socket.ready_state() == WebSocket::OPEN,
            None => false,
        }
    }

    fn send_now(state: &Rc<JsNostrState>, payload: &HandshakeMessage) -> Result<(), JsValue> {
        let keys = state
            .keys
            .borrow()
            .as_ref()
            .cloned()
            .ok_or_else(|| js_error("handshake keys missing"))?;
        let session = state
            .session
            .borrow()
            .as_ref()
            .cloned()
            .ok_or_else(|| js_error("handshake session missing"))?;
        let socket_ref = state.socket.borrow();
        let socket = socket_ref
            .as_ref()
            .ok_or_else(|| js_error("handshake socket missing"))?;
        let role = *state.role.borrow();

        let content = handshake_payload(&session, role, payload);
        let content_json = serde_json::to_string(&content)
            .map_err(|err| js_error(format!("handshake payload serialize error: {err}")))?;
        let tags = vec![
            Tag::custom(TagKind::custom("t"), [session.clone()]),
            Tag::custom(TagKind::custom("type"), [payload.message_type.as_str().to_string()]),
            Tag::custom(TagKind::custom("role"), [role.as_str().to_string()]),
        ];

        let builder = EventBuilder::new(Kind::from(HANDSHAKE_KIND), content_json).tags(tags);
        let event = builder
            .sign_with_keys(&keys)
            .map_err(|err| js_error(format!("failed to sign handshake event: {err}")))?;

        let outbound = format!("[\"EVENT\",{}]", event.as_json());
        socket.send_with_str(&outbound)
    }

    fn flush_pending(state: &Rc<JsNostrState>) {
        loop {
            if !JsNostrState::is_socket_open(state) {
                break;
            }
            let message = {
                let mut queue = state.pending.borrow_mut();
                queue.pop_front()
            };
            let Some(message) = message else {
                break;
            };
            if let Err(err) = JsNostrState::send_now(state, &message) {
                log::error!("failed to flush handshake event: {:?}", err);
                state.pending.borrow_mut().push_front(message);
                break;
            }
        }
    }

    fn shutdown_rc(state: &Rc<JsNostrState>) {
        if let Some(socket) = state.socket.borrow_mut().take() {
            let _ = socket.close();
        }
        state.listener.borrow_mut().take();
        state.keys.borrow_mut().take();
        state.on_message.borrow_mut().take();
        state.on_open.borrow_mut().take();
        state.on_error.borrow_mut().take();
        state.pending.borrow_mut().clear();
    }

    fn install_handlers(state: Rc<JsNostrState>, socket: &WebSocket) {
        let state_for_message = state.clone();
        let message_closure = Closure::<dyn FnMut(MessageEvent)>::wrap(Box::new(move |event: MessageEvent| {
            JsNostrState::handle_message(&state_for_message, event);
        }));
        socket.set_onmessage(Some(message_closure.as_ref().unchecked_ref()));
        *state.on_message.borrow_mut() = Some(message_closure);

        let state_for_open = state.clone();
        let open_closure = Closure::<dyn FnMut(JsValue)>::wrap(Box::new(move |_| {
            JsNostrState::send_subscription(&state_for_open);
        }));
        socket.set_onopen(Some(open_closure.as_ref().unchecked_ref()));
        *state.on_open.borrow_mut() = Some(open_closure);

        let error_closure = Closure::<dyn FnMut(ErrorEvent)>::wrap(Box::new(move |event: ErrorEvent| {
            log::error!("handshake websocket error: {}", event.message());
        }));
        socket.set_onerror(Some(error_closure.as_ref().unchecked_ref()));
        *state.on_error.borrow_mut() = Some(error_closure);
    }

    fn send_subscription(state: &Rc<JsNostrState>) {
        let socket_borrow = state.socket.borrow();
        let socket = match socket_borrow.as_ref() {
            Some(socket) => socket,
            None => return,
        };
        let session_borrow = state.session.borrow();
        let session = match session_borrow.as_ref() {
            Some(session) => session,
            None => return,
        };
        let payload = json!({
            "kinds": [HANDSHAKE_KIND],
            "#t": [session],
            "limit": 50,
        });
        let subscription = format!("[\"REQ\",\"marmot-{session}\",{}]", payload);
        if let Err(err) = socket.send_with_str(&subscription) {
            log::error!("failed to send handshake subscription: {:?}", err);
        } else {
            JsNostrState::flush_pending(state);
        }
    }

    fn handle_message(state: &Rc<JsNostrState>, event: MessageEvent) {
        let data = match event.data().as_string() {
            Some(text) => text,
            None => return,
        };
        let parsed: JsonValue = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(err) => {
                log::warn!("failed to parse handshake message: {err}");
                return;
            }
        };
        let array = match parsed.as_array() {
            Some(array) if array.len() >= 3 => array,
            _ => return,
        };
        if array.get(0).and_then(|v| v.as_str()) != Some("EVENT") {
            return;
        }
        let event_value = array[2].clone();
        let event_json = match serde_json::to_string(&event_value) {
            Ok(json) => json,
            Err(_) => return,
        };
        let nostr_event = match Event::from_json(&event_json) {
            Ok(event) => event,
            Err(err) => {
                log::warn!("failed to decode nostr event: {err}");
                return;
            }
        };
        if nostr_event.kind != Kind::from(HANDSHAKE_KIND) {
            return;
        }
        let content = &nostr_event.content;
        let payload: JsonValue = match serde_json::from_str(content) {
            Ok(value) => value,
            Err(err) => {
                log::warn!("invalid handshake payload: {err}");
                return;
            }
        };
        let session = match state.session.borrow().as_ref() {
            Some(session) => session.clone(),
            None => return,
        };
        if payload.get("session").and_then(|v| v.as_str()) != Some(session.as_str()) {
            return;
        }
        let from_role = match payload
            .get("from")
            .and_then(|v| v.as_str())
            .and_then(Role::from_str)
        {
            Some(role) => role,
            None => return,
        };
        if from_role == *state.role.borrow() {
            return;
        }
        let message = match handshake_from_payload(&payload) {
            Some(message) => message,
            None => return,
        };
        if let Some(listener) = state.listener.borrow().as_ref() {
            listener.on_message(message);
        }
    }
}

fn handshake_payload(session: &str, role: Role, message: &HandshakeMessage) -> JsonValue {
    let mut base = json!({
        "type": message.message_type.as_str(),
        "session": session,
        "from": role.as_str(),
        "created_at": js_sys::Date::now() as u64 / 1000,
    });
    match &message.data {
        HandshakeMessageBody::None => {}
        HandshakeMessageBody::KeyPackage { event, bundle, pubkey } => {
            if let Some(obj) = base.as_object_mut() {
                obj.insert("event".to_string(), json!(event));
                if let Some(bundle) = bundle {
                    obj.insert("bundle".to_string(), json!(bundle));
                }
                if let Some(pubkey) = pubkey {
                    obj.insert("pubkey".to_string(), json!(pubkey));
                }
            }
        }
        HandshakeMessageBody::Welcome { welcome, group_id_hex } => {
            if let Some(obj) = base.as_object_mut() {
                obj.insert("welcome".to_string(), json!(welcome));
                if let Some(group) = group_id_hex {
                    obj.insert("groupIdHex".to_string(), json!(group));
                }
            }
        }
    }
    base
}

fn handshake_from_payload(payload: &JsonValue) -> Option<HandshakeMessage> {
    let ty = payload.get("type")?.as_str()?;
    let message_type = HandshakeMessageType::from_str(ty)?;
    let data = match message_type {
        HandshakeMessageType::RequestKeyPackage | HandshakeMessageType::RequestWelcome => {
            HandshakeMessageBody::None
        }
        HandshakeMessageType::KeyPackage => {
            let event = payload.get("event")?.as_str()?.to_string();
            let bundle = payload
                .get("bundle")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let pubkey = payload
                .get("pubkey")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            HandshakeMessageBody::KeyPackage { event, bundle, pubkey }
        }
        HandshakeMessageType::Welcome => {
            let welcome = payload.get("welcome")?.as_str()?.to_string();
            let group_id_hex = payload
                .get("groupIdHex")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            HandshakeMessageBody::Welcome {
                welcome,
                group_id_hex,
            }
        }
    };
    Some(HandshakeMessage { message_type, data })
}

// =====================================================
// MoQ service (bridge to JS implementation)
// =====================================================

struct JsMoqService {
    handle: Rc<RefCell<Option<JsValue>>>,
    listener: Rc<RefCell<Option<Box<dyn MoqListener>>>>,
    pending: Rc<RefCell<Vec<Vec<u8>>>>,
    ready: Rc<RefCell<bool>>,
    on_ready: Rc<RefCell<Option<Closure<dyn FnMut()>>>>,
    on_frame: Rc<RefCell<Option<Closure<dyn FnMut(Uint8Array)>>>>,
    on_error: Rc<RefCell<Option<Closure<dyn FnMut(JsValue)>>>>,
    on_closed: Rc<RefCell<Option<Closure<dyn FnMut()>>>>,
}

impl JsMoqService {
    fn new() -> Self {
        Self {
            handle: Rc::new(RefCell::new(None)),
            listener: Rc::new(RefCell::new(None)),
            pending: Rc::new(RefCell::new(Vec::new())),
            ready: Rc::new(RefCell::new(false)),
            on_ready: Rc::new(RefCell::new(None)),
            on_frame: Rc::new(RefCell::new(None)),
            on_error: Rc::new(RefCell::new(None)),
            on_closed: Rc::new(RefCell::new(None)),
        }
    }

    fn flush_pending(&self) {
        if !*self.ready.borrow() {
            return;
        }
        let handle = match self.handle.borrow().as_ref() {
            Some(handle) => handle.clone(),
            None => return,
        };
        let publish = match get_bridge_method(&handle, "publish") {
            Ok(fun) => fun,
            Err(err) => {
                log::error!("missing moq publish: {:?}", err);
                return;
            }
        };
        while let Some(bytes) = self.pending.borrow_mut().pop() {
            let buffer = Uint8Array::from(bytes.as_slice());
            if let Err(err) = publish.call1(&handle, &buffer.into()) {
                log::error!("moq publish failed: {:?}", err);
            }
        }
    }
}

impl MoqService for JsMoqService {
    fn connect(&self, url: &str, session: &str, role: Role, peer_role: Role, listener: Box<dyn MoqListener>) {
        *self.listener.borrow_mut() = Some(listener);
        let bridge = match get_moq_bridge() {
            Ok(obj) => obj,
            Err(err) => {
                log::error!("moq bridge missing: {:?}", err);
                return;
            }
        };
        let connect = match get_bridge_method(&bridge, "connect") {
            Ok(fun) => fun,
            Err(err) => {
                log::error!("moq connect missing: {:?}", err);
                return;
            }
        };

        let params = json!({
            "relay": url,
            "session": session,
            "role": role.as_str(),
            "peerRole": peer_role.as_str(),
        });
        let params_js = swb::to_value(&params).unwrap_or(JsValue::NULL);

        let ready_flag = self.ready.clone();
        let handle_cell = self.handle.clone();
        let listener_cell = self.listener.clone();
        let listener_for_ready = self.listener.clone();
        let on_ready_closure = Closure::wrap(Box::new(move || {
            *ready_flag.borrow_mut() = true;
            if let Some(listener) = listener_for_ready.borrow().as_ref() {
                listener.on_ready();
            }
        }) as Box<dyn FnMut()>);

        let listener_for_frame = listener_cell.clone();
        let on_frame_closure = Closure::wrap(Box::new(move |buffer: Uint8Array| {
            let mut data = vec![0u8; buffer.length() as usize];
            buffer.copy_to(&mut data[..]);
            if let Some(listener) = listener_for_frame.borrow().as_ref() {
                listener.on_frame(data);
            }
        }) as Box<dyn FnMut(Uint8Array)>);

        let listener_for_error = listener_cell.clone();
        let on_error_closure = Closure::wrap(Box::new(move |value: JsValue| {
            let message = value.as_string().unwrap_or_else(|| String::from("unknown error"));
            if let Some(listener) = listener_for_error.borrow().as_ref() {
                listener.on_error(message);
            }
        }) as Box<dyn FnMut(JsValue)>);

        let listener_for_closed = listener_cell.clone();
        let on_closed_closure = Closure::wrap(Box::new(move || {
            if let Some(listener) = listener_for_closed.borrow().as_ref() {
                listener.on_closed();
            }
        }) as Box<dyn FnMut()>);

        let callbacks_obj = Object::new();
        let _ = Reflect::set(&callbacks_obj, &JsValue::from_str("onReady"), on_ready_closure.as_ref());
        let _ = Reflect::set(&callbacks_obj, &JsValue::from_str("onFrame"), on_frame_closure.as_ref());
        let _ = Reflect::set(&callbacks_obj, &JsValue::from_str("onError"), on_error_closure.as_ref());
        let _ = Reflect::set(&callbacks_obj, &JsValue::from_str("onClosed"), on_closed_closure.as_ref());

        *self.on_ready.borrow_mut() = Some(on_ready_closure);
        *self.on_frame.borrow_mut() = Some(on_frame_closure);
        *self.on_error.borrow_mut() = Some(on_error_closure);
        *self.on_closed.borrow_mut() = Some(on_closed_closure);

        let promise = match connect.call2(&bridge, &params_js, &callbacks_obj.into()) {
            Ok(value) => value,
            Err(err) => {
                log::error!("moq connect call failed: {:?}", err);
                return;
            }
        };

        let promise: js_sys::Promise = match promise.dyn_into() {
            Ok(p) => p,
            Err(err) => {
                log::error!("moq connect did not return a Promise: {:?}", err);
                return;
            }
        };

        let handle_cell_clone = handle_cell.clone();
        let service_clone = self.clone();
        spawn_local(async move {
            match JsFuture::from(promise).await {
                Ok(handle) => {
                    *handle_cell_clone.borrow_mut() = Some(handle.clone());
                    *service_clone.ready.borrow_mut() = true;
                    service_clone.flush_pending();
                }
                Err(err) => {
                    log::error!("moq connect promise rejected: {:?}", err);
                }
            }
        });
    }

    fn publish_wrapper(&self, bytes: &[u8]) {
        if !*self.ready.borrow() {
            self.pending.borrow_mut().push(bytes.to_vec());
            return;
        }
        let handle = match self.handle.borrow().as_ref() {
            Some(handle) => handle.clone(),
            None => {
                self.pending.borrow_mut().push(bytes.to_vec());
                return;
            }
        };
        let publish = match get_bridge_method(&handle, "publish") {
            Ok(fun) => fun,
            Err(err) => {
                log::error!("moq publish missing: {:?}", err);
                return;
            }
        };
        let buffer = Uint8Array::from(bytes);
        if let Err(err) = publish.call1(&handle, &buffer.into()) {
            log::error!("moq publish error: {:?}", err);
        }
    }

    fn shutdown(&self) {
        if let Some(handle) = self.handle.borrow_mut().take() {
            if let Ok(close) = get_bridge_method(&handle, "close") {
                let _ = close.call0(&handle);
            }
        }
        *self.ready.borrow_mut() = false;
        self.pending.borrow_mut().clear();
        self.listener.borrow_mut().take();
        self.on_ready.borrow_mut().take();
        self.on_frame.borrow_mut().take();
        self.on_error.borrow_mut().take();
        self.on_closed.borrow_mut().take();
    }
}

impl Clone for JsMoqService {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            listener: self.listener.clone(),
            pending: self.pending.clone(),
            ready: self.ready.clone(),
            on_ready: self.on_ready.clone(),
            on_frame: self.on_frame.clone(),
            on_error: self.on_error.clone(),
            on_closed: self.on_closed.clone(),
        }
    }
}

// =====================================================
// Utility helpers
// =====================================================

fn get_moq_bridge() -> Result<JsValue, JsValue> {
    let global = js_sys::global();
    Reflect::get(&global, &JsValue::from_str(MOQ_BRIDGE_KEY))
}

fn get_bridge_method(target: &JsValue, name: &str) -> Result<Function, JsValue> {
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

struct LegacyIdentity {
    keys: Keys,
    mdk: MDK<MdkMemoryStorage>,
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

fn with_identity<F, R>(id: u32, f: F) -> Result<R, JsValue>
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

fn decode_hex(bytes_hex: &str) -> Result<Vec<u8>, JsValue> {
    hex::decode(bytes_hex).map_err(|e| js_error(format!("invalid hex: {e}")))
}

fn parse_public_keys(keys: &[String]) -> Result<Vec<PublicKey>, JsValue> {
    keys.iter()
        .map(|k| PublicKey::from_hex(k).map_err(|e| js_error(format!("invalid pubkey: {e}"))))
        .collect()
}

#[wasm_bindgen]
pub fn create_identity(secret_hex: String) -> Result<u32, JsValue> {
    let secret = SecretKey::from_hex(&secret_hex).map_err(|e| js_error(format!("invalid secret: {e}")))?;
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

        swb::to_value(&export).map_err(|e| js_error(format!(
            "failed to serialize exported key package bundle: {e}"
        )))
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
        let input: CreateMessageInput =
            swb::from_value(payload).map_err(|e| js_error(format!("invalid message payload: {e}")))?;
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

#[wasm_bindgen]
pub fn ingest_wrapper(identity_id: u32, wrapper: Uint8Array) -> Result<JsValue, JsValue> {
    with_identity(identity_id, |identity| {
        let mut bytes = vec![0u8; wrapper.length() as usize];
        wrapper.copy_to(&mut bytes[..]);
        let json = String::from_utf8(bytes).map_err(|e| js_error(format!("invalid utf8: {e}")))?;
        let event = Event::from_json(&json).map_err(|e| js_error(format!("invalid wrapper: {e}")))?;
        let result = identity
            .mdk
            .process_message(&event)
            .map_err(|e| js_error(format!("failed to process wrapper: {e}")))?;

        let processed = match result {
            MessageProcessingResult::ApplicationMessage(msg) => ProcessedWrapper {
                kind: "application".into(),
                message: Some(DecryptedMessage {
                    content: msg.content,
                    author: msg.pubkey.to_hex(),
                    created_at: msg.created_at.as_u64(),
                    event: JsonValue::Null,
                }),
                proposal: None,
                commit: None,
            },
            MessageProcessingResult::Commit => ProcessedWrapper {
                kind: "commit".into(),
                message: None,
                proposal: None,
                commit: Some(CommitEnvelope {
                    event: event.as_json(),
                }),
            },
            MessageProcessingResult::Proposal(_) => ProcessedWrapper {
                kind: "proposal".into(),
                message: None,
                proposal: None,
                commit: None,
            },
            MessageProcessingResult::ExternalJoinProposal => ProcessedWrapper {
                kind: "external_join".into(),
                message: None,
                proposal: None,
                commit: None,
            },
            MessageProcessingResult::Unprocessable => ProcessedWrapper {
                kind: "unprocessable".into(),
                message: None,
                proposal: None,
                commit: None,
            },
        };
        swb::to_value(&processed).map_err(|e| js_error(format!("failed to serialize processed wrapper: {e}")))
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
