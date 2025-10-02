use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use log::warn;
use nostr::{Keys, SecretKey};

use super::events::SessionRole;
use super::services::{
    HandshakeConnectParams, HandshakeListener, HandshakeMessage, HandshakeMessageBody, MoqListener,
    MoqService, NostrService,
};

thread_local! {
    static REGISTRY: RefCell<HashMap<String, Rc<LocalNetwork>>> = RefCell::new(HashMap::new());
}

pub fn connect_services(id: &str) -> (Rc<dyn NostrService>, Rc<dyn MoqService>) {
    let network = REGISTRY.with(|registry| {
        let mut map = registry.borrow_mut();
        map.entry(id.to_string())
            .or_insert_with(|| Rc::new(LocalNetwork::new()))
            .clone()
    });
    let endpoint = Rc::new(LocalEndpoint::new(network.clone()));
    (
        Rc::new(LocalNostrService::new(endpoint.clone())),
        Rc::new(LocalMoqService::new(endpoint)),
    )
}

struct LocalEndpoint {
    network: Rc<LocalNetwork>,
    session: RefCell<Option<String>>,
    role: RefCell<Option<SessionRole>>,
    pubkey: RefCell<Option<String>>,
}

impl LocalEndpoint {
    fn new(network: Rc<LocalNetwork>) -> Self {
        Self {
            network,
            session: RefCell::new(None),
            role: RefCell::new(None),
            pubkey: RefCell::new(None),
        }
    }

    fn set_session(&self, session: String) {
        *self.session.borrow_mut() = Some(session);
    }

    fn session(&self) -> Option<String> {
        self.session.borrow().clone()
    }

    fn set_role(&self, role: SessionRole) {
        *self.role.borrow_mut() = Some(role);
    }

    fn role(&self) -> Option<SessionRole> {
        *self.role.borrow()
    }

    fn set_pubkey(&self, pubkey: String) {
        *self.pubkey.borrow_mut() = Some(pubkey);
    }

    fn pubkey(&self) -> Option<String> {
        self.pubkey.borrow().clone()
    }
}

struct LocalNetwork {
    handshake_listeners: RefCell<HashMap<String, Vec<HandshakeEntry>>>,
    moq_subscribers: RefCell<HashMap<String, Vec<MoqEntry>>>,
    pending_handshakes: RefCell<HashMap<String, Vec<PendingHandshake>>>,
}

struct HandshakeEntry {
    role: SessionRole,
    pubkey: String,
    listener: Weak<RefCell<Option<Box<dyn HandshakeListener>>>>,
}

#[derive(Clone)]
struct PendingHandshake {
    target_role: SessionRole,
    target_pubkey: Option<String>,
    message: HandshakeMessage,
}

struct MoqEntry {
    role: SessionRole,
    pubkey: Option<String>,
    listener: Weak<RefCell<Option<Box<dyn MoqListener>>>>,
}

impl LocalNetwork {
    fn new() -> Self {
        Self {
            handshake_listeners: RefCell::new(HashMap::new()),
            moq_subscribers: RefCell::new(HashMap::new()),
            pending_handshakes: RefCell::new(HashMap::new()),
        }
    }

    fn register_handshake(
        &self,
        session: String,
        role: SessionRole,
        pubkey: String,
        listener: Rc<RefCell<Option<Box<dyn HandshakeListener>>>>,
    ) {
        let mut map = self.handshake_listeners.borrow_mut();
        let session_key = session.clone();
        let pubkey_key = pubkey.clone();
        map.entry(session)
            .or_insert_with(Vec::new)
            .push(HandshakeEntry {
                role,
                pubkey,
                listener: Rc::downgrade(&listener),
            });
        self.flush_pending(session_key, role, pubkey_key, listener);
    }

    fn dispatch_handshake(&self, session: &str, from_role: SessionRole, message: HandshakeMessage) {
        let target_pubkey = match &message.data {
            HandshakeMessageBody::Request { pubkey, .. } => pubkey.clone(),
            HandshakeMessageBody::Welcome { recipient, .. } => recipient.clone(),
            _ => None,
        };
        let target_role = from_role.peer();
        let mut delivered = false;
        if let Some(entries) = self.handshake_listeners.borrow_mut().get_mut(session) {
            entries.retain(|entry| {
                if let Some(listener) = entry.listener.upgrade() {
                    if entry.role == target_role {
                        let matches_target = match &target_pubkey {
                            Some(target) => &entry.pubkey == target,
                            None => true,
                        };
                        if matches_target {
                            if let Some(handler) = listener.borrow().as_ref() {
                                handler.on_message(message.clone());
                                delivered = true;
                            }
                        }
                    }
                    true
                } else {
                    false
                }
            });
        }
        if !delivered {
            self.pending_handshakes
                .borrow_mut()
                .entry(session.to_string())
                .or_insert_with(Vec::new)
                .push(PendingHandshake {
                    target_role,
                    target_pubkey: target_pubkey.clone(),
                    message,
                });
        }
    }

    fn register_moq_listener(
        &self,
        session: String,
        role: SessionRole,
        pubkey: Option<String>,
        listener: Rc<RefCell<Option<Box<dyn MoqListener>>>>,
    ) {
        let mut map = self.moq_subscribers.borrow_mut();
        map.entry(session).or_insert_with(Vec::new).push(MoqEntry {
            role,
            pubkey,
            listener: Rc::downgrade(&listener),
        });
    }

    fn broadcast(&self, session: &str, sender_pubkey: Option<String>, bytes: Vec<u8>) {
        if let Some(entries) = self.moq_subscribers.borrow_mut().get_mut(session) {
            entries.retain(|entry| {
                if let Some(listener) = entry.listener.upgrade() {
                    let different_member = match (&sender_pubkey, &entry.pubkey) {
                        (Some(sender), Some(target)) => sender != target,
                        _ => true,
                    };

                    if different_member {
                        if let Some(handler) = listener.borrow().as_ref() {
                            handler.on_frame(bytes.clone());
                        }
                    }
                    true
                } else {
                    false
                }
            });
        }
    }

    fn notify_ready(&self, session: &str, role: SessionRole) {
        if let Some(entries) = self.moq_subscribers.borrow_mut().get_mut(session) {
            entries.retain(|entry| {
                if let Some(listener) = entry.listener.upgrade() {
                    if entry.role == role {
                        if let Some(handler) = listener.borrow().as_ref() {
                            handler.on_ready();
                        }
                    }
                    true
                } else {
                    false
                }
            });
        }
    }
}

impl LocalNetwork {
    fn flush_pending(
        &self,
        session: String,
        role: SessionRole,
        pubkey: String,
        listener: Rc<RefCell<Option<Box<dyn HandshakeListener>>>>,
    ) {
        let mut pending_map = self.pending_handshakes.borrow_mut();
        if let Some(queue) = pending_map.get_mut(&session) {
            let mut index = 0;
            while index < queue.len() {
                let deliver_role = queue[index].target_role == role;
                let deliver_pubkey = match queue[index].target_pubkey.as_ref() {
                    Some(target) => target == &pubkey,
                    None => true,
                };

                if deliver_role && deliver_pubkey {
                    if let Some(handler) = listener.borrow().as_ref() {
                        handler.on_message(queue[index].message.clone());
                    }
                    queue.remove(index);
                } else {
                    index += 1;
                }
            }
            if queue.is_empty() {
                pending_map.remove(&session);
            }
        }
    }
}

struct LocalNostrService {
    endpoint: Rc<LocalEndpoint>,
    listener: Rc<RefCell<Option<Box<dyn HandshakeListener>>>>,
}

impl LocalNostrService {
    fn new(endpoint: Rc<LocalEndpoint>) -> Self {
        Self {
            endpoint,
            listener: Rc::new(RefCell::new(None)),
        }
    }
}

impl NostrService for LocalNostrService {
    fn connect(&self, params: HandshakeConnectParams, listener: Box<dyn HandshakeListener>) {
        self.endpoint.set_session(params.session.clone());
        self.endpoint.set_role(params.role);
        *self.listener.borrow_mut() = Some(listener);
        let derived = SecretKey::from_hex(&params.secret_hex)
            .map(Keys::new)
            .map(|keys| keys.public_key().to_hex());
        let pubkey = match derived {
            Ok(hex) => {
                self.endpoint.set_pubkey(hex.clone());
                hex
            }
            Err(err) => {
                warn!("local transport: failed to derive pubkey: {err:#}");
                format!("{}-{}", params.role.as_str(), params.session)
            }
        };
        self.endpoint.network.register_handshake(
            params.session,
            params.role,
            pubkey,
            self.listener.clone(),
        );
    }

    fn send(&self, payload: HandshakeMessage) {
        if let (Some(session), Some(role)) = (self.endpoint.session(), self.endpoint.role()) {
            self.endpoint
                .network
                .dispatch_handshake(&session, role, payload);
        }
    }

    fn shutdown(&self) {
        self.listener.borrow_mut().take();
    }
}

struct LocalMoqService {
    endpoint: Rc<LocalEndpoint>,
    listener: Rc<RefCell<Option<Box<dyn MoqListener>>>>,
    peer_role: RefCell<Option<SessionRole>>,
}

impl LocalMoqService {
    fn new(endpoint: Rc<LocalEndpoint>) -> Self {
        Self {
            endpoint,
            listener: Rc::new(RefCell::new(None)),
            peer_role: RefCell::new(None),
        }
    }
}

impl MoqService for LocalMoqService {
    fn connect(
        &self,
        _url: &str,
        session: &str,
        role: SessionRole,
        peer_role: SessionRole,
        listener: Box<dyn MoqListener>,
    ) {
        self.endpoint.set_session(session.to_string());
        self.endpoint.set_role(role);
        *self.peer_role.borrow_mut() = Some(peer_role);
        *self.listener.borrow_mut() = Some(listener);
        self.endpoint.network.register_moq_listener(
            session.to_string(),
            peer_role,
            self.endpoint.pubkey(),
            self.listener.clone(),
        );
        self.endpoint.network.notify_ready(session, peer_role);
    }

    fn publish_wrapper(&self, bytes: &[u8]) {
        if let Some(session) = self.endpoint.session() {
            let sender_pubkey = self.endpoint.pubkey();
            self.endpoint
                .network
                .broadcast(&session, sender_pubkey, bytes.to_vec());
        }
    }

    fn shutdown(&self) {
        self.listener.borrow_mut().take();
    }
}
