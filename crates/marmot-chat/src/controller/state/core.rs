use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::controller::events::{ChatEvent, HandshakePhase, SessionRole};

use super::types::{ControllerConfig, ControllerState, HandshakeState};

impl ControllerState {
    pub fn new(config: ControllerConfig) -> Self {
        let role = config.session.bootstrap_role;
        let handshake = match role {
            SessionRole::Initial => HandshakeState::WaitingForKeyPackage,
            SessionRole::Invitee => HandshakeState::WaitingForWelcome,
        };
        let mut admin_pubkeys: BTreeSet<String> =
            config.session.admin_pubkeys.iter().cloned().collect();
        if role == SessionRole::Initial {
            admin_pubkeys.insert(config.identity.public_key_hex());
        }
        Self {
            identity: config.identity,
            session: config.session,
            nostr: config.nostr,
            moq: config.moq,
            callback: config.callback,
            handshake,
            commits: 0,
            ready: false,
            outgoing_queue: VecDeque::new(),
            pending_incoming: VecDeque::new(),
            key_package_cache: None,
            welcome_json: None,
            admin_pubkeys,
            pending_invites: BTreeMap::new(),
            subscribed_peers: BTreeSet::new(),
        }
    }

    pub fn emit_status<S: Into<String>>(&self, status: S) {
        (self.callback)(ChatEvent::status(status));
    }

    pub fn emit_handshake_phase(&self, phase: HandshakePhase) {
        (self.callback)(ChatEvent::Handshake { phase });
    }

    pub fn handshake_phase(&self) -> HandshakePhase {
        match self.handshake {
            HandshakeState::WaitingForKeyPackage => HandshakePhase::WaitingForKeyPackage,
            HandshakeState::WaitingForWelcome => HandshakePhase::WaitingForWelcome,
            HandshakeState::Established => HandshakePhase::Connected,
        }
    }
}
