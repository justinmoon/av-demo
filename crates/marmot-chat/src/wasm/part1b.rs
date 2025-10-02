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
    role: RefCell<SessionRole>,
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
                role: RefCell::new(SessionRole::Initial),
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
    fn connect_rc(
        state: Rc<JsNostrState>,
        params: HandshakeConnectParams,
        listener: Box<dyn HandshakeListener>,
    ) {
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
            Tag::custom(
                TagKind::custom("type"),
                [payload.message_type.as_str().to_string()],
            ),
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
        let message_closure =
            Closure::<dyn FnMut(MessageEvent)>::wrap(Box::new(move |event: MessageEvent| {
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

        let error_closure =
            Closure::<dyn FnMut(ErrorEvent)>::wrap(Box::new(move |event: ErrorEvent| {
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
            .and_then(SessionRole::from_str)
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

fn handshake_payload(session: &str, role: SessionRole, message: &HandshakeMessage) -> JsonValue {
    let mut base = json!({
        "type": message.message_type.as_str(),
        "session": session,
        "from": role.as_str(),
        "created_at": js_sys::Date::now() as u64 / 1000,
    });
    match &message.data {
        HandshakeMessageBody::None => {}
        HandshakeMessageBody::Request { pubkey, is_admin } => {
            if let Some(obj) = base.as_object_mut() {
                if let Some(pubkey) = pubkey {
                    obj.insert("pubkey".to_string(), json!(pubkey));
                }
                if let Some(is_admin) = is_admin {
                    obj.insert("isAdmin".to_string(), json!(is_admin));
                }
            }
        }
        HandshakeMessageBody::KeyPackage {
            event,
            bundle,
            pubkey,
        } => {
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
        HandshakeMessageBody::Welcome {
            welcome,
            group_id_hex,
            recipient,
        } => {
            if let Some(obj) = base.as_object_mut() {
                obj.insert("welcome".to_string(), json!(welcome));
                if let Some(group) = group_id_hex {
                    obj.insert("groupIdHex".to_string(), json!(group));
                }
                if let Some(recipient) = recipient {
                    obj.insert("recipient".to_string(), json!(recipient));
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
            let pubkey = payload
                .get("pubkey")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let is_admin = payload.get("isAdmin").and_then(|v| v.as_bool());
            HandshakeMessageBody::Request { pubkey, is_admin }
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
            HandshakeMessageBody::KeyPackage {
                event,
                bundle,
                pubkey,
            }
        }
        HandshakeMessageType::Welcome => {
            let welcome = payload.get("welcome")?.as_str()?.to_string();
            let group_id_hex = payload
                .get("groupIdHex")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let recipient = payload
                .get("recipient")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            HandshakeMessageBody::Welcome {
                welcome,
                group_id_hex,
                recipient,
            }
        }
    };
    Some(HandshakeMessage { message_type, data })
}

// =====================================================
