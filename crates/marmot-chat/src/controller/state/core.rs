use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use anyhow::Result;
use futures::channel::mpsc::UnboundedSender;
use log::{info, warn};

use crate::controller::events::{
    ChatEvent, HandshakePhase, MemberInfo, SessionParams, SessionRole,
};
use crate::controller::services::{
    HandshakeMessage, IdentityHandle, KeyPackageExport, MoqService,
    NostrService,
};

use super::utils::{now_timestamp, schedule};

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
        let mut members = BTreeMap::new();
        let self_pub = config.identity.public_key_hex();
        let is_admin = admin_pubkeys.contains(&self_pub);
        members.insert(
            self_pub.clone(),
            MemberRecord {
                info: MemberInfo {
                    pubkey: self_pub,
                    is_admin,
                },
                joined: false,
            },
        );

        let peer_pubkeys: BTreeSet<String> = config.session.peer_pubkeys.iter().cloned().collect();
        for peer in &peer_pubkeys {
            if peer == &config.identity.public_key_hex() {
                continue;
            }
            let peer_is_admin = admin_pubkeys.contains(peer);
            members.entry(peer.clone()).or_insert_with(|| MemberRecord {
                info: MemberInfo {
                    pubkey: peer.clone(),
                    is_admin: peer_is_admin,
                },
                joined: false,
            });
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
            key_package_cache: None,
            welcome_json: None,
            members,
            admin_pubkeys,
            peer_pubkeys,
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

    fn emit_roster(&self) {
        let members: Vec<MemberInfo> = self
            .members
            .values()
            .filter(|record| record.joined)
            .map(|record| record.info.clone())
            .collect();
        if !members.is_empty() {
            (self.callback)(ChatEvent::Roster { members });
        }
    }

    pub(super) fn ensure_member(&mut self, pubkey: &str) -> &mut MemberRecord {
        let is_admin = self.admin_pubkeys.contains(pubkey);
        self.peer_pubkeys.insert(pubkey.to_string());
        self.members
            .entry(pubkey.to_string())
            .or_insert_with(|| MemberRecord {
                info: MemberInfo {
                    pubkey: pubkey.to_string(),
                    is_admin,
                },
                joined: false,
            })
    }

    pub(super) fn mark_member_joined(&mut self, pubkey: &str) {
        let newly_joined = {
            let entry = self.ensure_member(pubkey);
            if entry.joined {
                false
            } else {
                entry.joined = true;
                true
            }
        };
        if newly_joined {
            if let Some(record) = self.members.get(pubkey) {
                (self.callback)(ChatEvent::MemberJoined {
                    member: record.info.clone(),
                });
            }
            self.emit_roster();
        }
    }

    pub(super) fn update_member_admin(&mut self, pubkey: &str, is_admin: bool) {
        if is_admin {
            self.admin_pubkeys.insert(pubkey.to_string());
        } else {
            self.admin_pubkeys.remove(pubkey);
        }

        let mut updated_member: Option<MemberInfo> = None;
        if let Some(entry) = self.members.get_mut(pubkey) {
            if entry.info.is_admin != is_admin {
                entry.info.is_admin = is_admin;
                updated_member = Some(entry.info.clone());
            }
        } else {
            let entry = self.ensure_member(pubkey);
            entry.info.is_admin = is_admin;
            updated_member = Some(entry.info.clone());
        }

        if let Some(member) = updated_member {
            (self.callback)(ChatEvent::MemberUpdated {
                member: member.clone(),
            });
            if self
                .members
                .get(pubkey)
                .map(|record| record.joined)
                .unwrap_or(false)
            {
                self.emit_roster();
            }
        }
    }

    pub fn enqueue_outgoing(&mut self, bytes: Vec<u8>) {
        self.outgoing_queue.push_back(bytes);
    }

    pub fn take_next_outgoing(&mut self) -> Option<Vec<u8>> {
        self.outgoing_queue.pop_front()
    }

    pub fn handle_incoming_frame(&mut self, bytes: Vec<u8>) -> Result<Vec<ChatEvent>> {
        match self.identity.ingest_wrapper(&bytes)? {
            crate::controller::services::WrapperOutcome::Application {
                author,
                content,
                created_at,
            } => {
                self.mark_member_joined(&author);
                let local = author == self.identity.public_key_hex();
                Ok(vec![ChatEvent::Message {
                    author,
                    content,
                    created_at,
                    local,
                }])
            }
            crate::controller::services::WrapperOutcome::Commit => {
                self.identity.merge_pending_commit()?;
                self.commits += 1;
                self.sync_members_from_identity()?;
                Ok(vec![ChatEvent::Commit {
                    total: self.commits,
                }])
            }
            crate::controller::services::WrapperOutcome::None => Ok(Vec::new()),
        }
    }

    pub(super) fn sync_members_from_identity(&mut self) -> Result<()> {
        let members = match self.identity.list_members() {
            Ok(list) => list,
            Err(err) => {
                warn!("sync_members_from_identity failed: {err:#}");
                return Ok(());
            }
        };
        let mut updated = false;
        let own_pubkey = self.identity.public_key_hex();
        for pubkey in members {
            let entry = self.ensure_member(&pubkey);
            if !entry.joined {
                entry.joined = true;
                updated = true;
            }

            // Subscribe to peer's MoQ track if not already subscribed
            if pubkey != own_pubkey && !self.subscribed_peers.contains(&pubkey) {
                info!("sync_members: subscribing to peer {}", &pubkey[..8]);
                self.moq.subscribe_to_peer(&pubkey);
                self.subscribed_peers.insert(pubkey.clone());
            }
        }
        if updated {
            self.emit_roster();
        }
        Ok(())
    }

    pub fn handle_outgoing_message(&mut self, content: &str) -> Result<(Vec<u8>, ChatEvent)> {
        let wrapper = self.identity.create_message(content)?;
        let bytes = wrapper.bytes.clone();
        let event = ChatEvent::Message {
            author: self.identity.public_key_hex(),
            content: content.to_string(),
            created_at: now_timestamp(),
            local: true,
        };
        Ok((bytes, event))
    }

    pub fn handle_self_update(&mut self) -> Result<(Vec<u8>, Vec<ChatEvent>)> {
        let frame = self.identity.self_update()?;
        self.commits += 1;
        Ok((
            frame.bytes,
            vec![ChatEvent::Commit {
                total: self.commits,
            }],
        ))
    }

    pub fn handshake_phase(&self) -> HandshakePhase {
        match self.handshake {
            HandshakeState::WaitingForKeyPackage => HandshakePhase::WaitingForKeyPackage,
            HandshakeState::WaitingForWelcome => HandshakePhase::WaitingForWelcome,
            HandshakeState::Established => HandshakePhase::Connected,
        }
    }

    pub fn mark_ready(&mut self, ready: bool) -> ChatEvent {
        self.ready = ready;
        ChatEvent::Ready { ready }
    }

    pub fn on_ready(&mut self, tx: &UnboundedSender<Operation>) {
        let event = self.mark_ready(true);
        schedule(tx, Operation::Emit(event));
        while let Some(bytes) = self.take_next_outgoing() {
            self.moq.publish_wrapper(&bytes);
        }
    }

    pub fn publish_or_queue(&mut self, bytes: Vec<u8>) {
        if self.ready {
            self.moq.publish_wrapper(&bytes);
        } else {
            self.enqueue_outgoing(bytes);
        }
    }
}
