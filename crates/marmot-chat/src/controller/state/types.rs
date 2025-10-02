use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use crate::controller::events::{ChatEvent, MemberInfo, SessionParams};
use crate::controller::services::{
    HandshakeMessage, IdentityHandle, KeyPackageExport, MoqService, NostrService,
};

pub type EventCallback = Rc<dyn Fn(ChatEvent)>;

pub struct ControllerConfig {
    pub identity: IdentityHandle,
    pub session: SessionParams,
    pub nostr: Rc<dyn NostrService>,
    pub moq: Rc<dyn MoqService>,
    pub callback: EventCallback,
}

pub struct ControllerState {
    pub identity: IdentityHandle,
    pub session: SessionParams,
    pub nostr: Rc<dyn NostrService>,
    pub moq: Rc<dyn MoqService>,
    pub callback: EventCallback,
    pub handshake: HandshakeState,
    pub commits: u32,
    pub ready: bool,
    pub outgoing_queue: VecDeque<Vec<u8>>,
    pub key_package_cache: Option<KeyPackageExport>,
    pub welcome_json: Option<String>,
    pub members: BTreeMap<String, MemberRecord>,
    pub admin_pubkeys: BTreeSet<String>,
    pub peer_pubkeys: BTreeSet<String>,
    pub pending_invites: BTreeMap<String, PendingInvite>,
    pub subscribed_peers: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub info: MemberInfo,
    pub joined: bool,
}

#[derive(Debug, Clone)]
pub struct PendingInvite {
    pub is_admin: bool,
}

#[derive(Debug, Clone)]
pub enum Operation {
    Start,
    Emit(ChatEvent),
    OutgoingHandshake(HandshakeMessage),
    IncomingHandshake(HandshakeMessage),
    ConnectMoq,
    IncomingFrame(Vec<u8>),
    PublishWrapper(Vec<u8>),
    Ready,
    Shutdown,
    SendText(String),
    RotateEpoch,
    InviteMember { pubkey: String, is_admin: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum HandshakeState {
    WaitingForKeyPackage,
    WaitingForWelcome,
    Established,
}
