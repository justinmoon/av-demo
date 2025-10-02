use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use anyhow::{anyhow, Context, Result};
use futures::channel::mpsc::UnboundedSender;

use crate::controller::events::{
    ChatEvent, HandshakePhase, MemberInfo, SessionParams, SessionRole,
};
use crate::controller::services::{
    GroupArtifacts, HandshakeConnectParams, HandshakeListener, HandshakeMessage,
    HandshakeMessageBody, HandshakeMessageType, IdentityHandle, KeyPackageExport, MoqService,
    NostrService,
};
use nostr::{prelude::FromBech32, PublicKey};

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

    fn ensure_member(&mut self, pubkey: &str) -> &mut MemberRecord {
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

    fn mark_member_joined(&mut self, pubkey: &str) {
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

    fn update_member_admin(&mut self, pubkey: &str, is_admin: bool) {
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
                Ok(vec![ChatEvent::Commit {
                    total: self.commits,
                }])
            }
            crate::controller::services::WrapperOutcome::None => Ok(Vec::new()),
        }
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

    pub fn request_invite(
        &mut self,
        tx: &UnboundedSender<Operation>,
        pubkey_input: String,
        is_admin: bool,
    ) -> Result<()> {
        let trimmed = pubkey_input.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("pubkey required"));
        }

        let parsed_pk = PublicKey::from_hex(trimmed)
            .or_else(|_| PublicKey::from_bech32(trimmed))
            .context("parse invite pubkey")?;
        let pubkey = parsed_pk.to_hex();

        if pubkey == self.identity.public_key_hex() {
            return Err(anyhow!("cannot invite self"));
        }

        if self
            .members
            .get(&pubkey)
            .map(|member| member.joined)
            .unwrap_or(false)
        {
            return Err(anyhow!("member already present"));
        }

        if self.pending_invites.contains_key(&pubkey) {
            return Err(anyhow!("invite already pending"));
        }

        self.peer_pubkeys.insert(pubkey.clone());
        self.pending_invites
            .insert(pubkey.clone(), PendingInvite { is_admin });

        if is_admin {
            self.update_member_admin(&pubkey, true);
        }

        self.ensure_member(&pubkey);

        self.emit_status(format!(
            "Requesting key package from {}",
            short_key(&pubkey)
        ));

        schedule(
            tx,
            Operation::OutgoingHandshake(HandshakeMessage {
                message_type: HandshakeMessageType::RequestKeyPackage,
                data: HandshakeMessageBody::Request {
                    pubkey: Some(pubkey.clone()),
                    is_admin: Some(is_admin),
                },
            }),
        );

        Ok(())
    }

    fn handle_member_addition(
        &mut self,
        tx: &UnboundedSender<Operation>,
        invitee_pub: String,
        event_json: String,
        _bundle: Option<String>,
    ) -> Result<()> {
        let requested_admin = self
            .pending_invites
            .remove(&invitee_pub)
            .map(|invite| invite.is_admin)
            .unwrap_or(false);

        if requested_admin {
            self.update_member_admin(&invitee_pub, true);
        }

        self.peer_pubkeys.insert(invitee_pub.clone());
        self.ensure_member(&invitee_pub);

        let artifacts = self
            .identity
            .add_members(&[event_json.clone()])
            .map_err(|err| anyhow!("add members failed: {err}"))?;

        self.commits += 1;

        schedule(
            tx,
            Operation::PublishWrapper(artifacts.commit.bytes.clone()),
        );

        let group_hex = self.identity.group_id_hex().unwrap_or_default();
        for welcome in artifacts.welcomes {
            schedule(
                tx,
                Operation::OutgoingHandshake(HandshakeMessage {
                    message_type: HandshakeMessageType::Welcome,
                    data: HandshakeMessageBody::Welcome {
                        welcome: welcome.welcome.clone(),
                        group_id_hex: Some(group_hex.clone()),
                    },
                }),
            );
            (self.callback)(ChatEvent::InviteGenerated {
                welcome: welcome.welcome,
                recipient: welcome.recipient.clone(),
                is_admin: self.admin_pubkeys.contains(&welcome.recipient),
            });
        }

        self.emit_status(format!("Sent welcome to {}", short_key(&invitee_pub)));

        Ok(())
    }

    pub fn on_start(
        &mut self,
        tx: &UnboundedSender<Operation>,
        listener: Box<dyn HandshakeListener>,
    ) -> Result<()> {
        self.emit_status("Connecting handshake relay…");
        let params = HandshakeConnectParams {
            url: self.session.nostr_url.clone(),
            session: self.session.session_id.clone(),
            role: self.session.bootstrap_role,
            secret_hex: self.session.secret_hex.clone(),
        };
        self.nostr.connect(params, listener);
        self.emit_handshake_phase(self.handshake_phase());

        match self.session.bootstrap_role {
            SessionRole::Initial => {
                self.emit_status("Requesting key package…");
                schedule(
                    tx,
                    Operation::OutgoingHandshake(HandshakeMessage {
                        message_type: HandshakeMessageType::RequestKeyPackage,
                        data: HandshakeMessageBody::Request {
                            pubkey: self.session.peer_pubkeys.get(0).cloned(),
                            is_admin: None,
                        },
                    }),
                );
            }
            SessionRole::Invitee => {
                self.emit_status("Generating key package…");
                let export = if let Some(stub) = self.session.stub.clone() {
                    if let Some(event) = stub.key_package_event {
                        let bundle = stub.key_package_bundle.unwrap_or_default();
                        let export = KeyPackageExport {
                            event_json: event,
                            bundle,
                        };
                        self.key_package_cache = Some(export.clone());
                        export
                    } else {
                        let relays = vec![relay_relays_url(&self.session.relay_url)];
                        let export = self.identity.create_key_package(&relays)?;
                        self.key_package_cache = Some(export.clone());
                        export
                    }
                } else {
                    let relays = vec![relay_relays_url(&self.session.relay_url)];
                    let export = self.identity.create_key_package(&relays)?;
                    self.key_package_cache = Some(export.clone());
                    export
                };
                schedule(
                    tx,
                    Operation::OutgoingHandshake(HandshakeMessage {
                        message_type: HandshakeMessageType::KeyPackage,
                        data: HandshakeMessageBody::KeyPackage {
                            event: export.event_json,
                            bundle: Some(export.bundle.clone()),
                            pubkey: Some(self.identity.public_key_hex()),
                        },
                    }),
                );
            }
        }

        Ok(())
    }

    pub fn on_incoming_handshake(
        &mut self,
        tx: &UnboundedSender<Operation>,
        message: HandshakeMessage,
    ) -> Result<()> {
        match self.session.bootstrap_role {
            SessionRole::Initial => self.handle_handshake_as_creator(tx, message),
            SessionRole::Invitee => self.handle_handshake_as_joiner(tx, message),
        }
    }

    fn handle_handshake_as_creator(
        &mut self,
        tx: &UnboundedSender<Operation>,
        message: HandshakeMessage,
    ) -> Result<()> {
        match message.message_type {
            HandshakeMessageType::KeyPackage => {
                let (event, bundle, pubkey) = match message.data {
                    HandshakeMessageBody::KeyPackage {
                        event,
                        bundle,
                        pubkey,
                    } => (event, bundle, pubkey),
                    _ => return Err(anyhow!("missing key package payload")),
                };
                let invitee_pub =
                    match pubkey.or_else(|| self.session.peer_pubkeys.first().cloned()) {
                        Some(key) => key,
                        None => return Err(anyhow!("invitee pubkey missing")),
                    };

                if self.handshake == HandshakeState::Established {
                    return self.handle_member_addition(tx, invitee_pub, event, bundle);
                }

                self.peer_pubkeys.insert(invitee_pub.clone());
                self.ensure_member(&invitee_pub);
                if self.admin_pubkeys.contains(&invitee_pub) {
                    self.update_member_admin(&invitee_pub, true);
                }
                self.key_package_cache = Some(KeyPackageExport {
                    event_json: event.clone(),
                    bundle: bundle.clone().unwrap_or_default(),
                });
                let GroupArtifacts {
                    group_id_hex,
                    welcome,
                } = self
                    .identity
                    .create_group(&event, &invitee_pub, &self.session.admin_pubkeys)
                    .map_err(|err| anyhow!("create_group failed: {err}"))?;
                self.welcome_json = Some(welcome.clone());
                self.emit_status("Group created; sending welcome…");
                schedule(
                    tx,
                    Operation::OutgoingHandshake(HandshakeMessage {
                        message_type: HandshakeMessageType::Welcome,
                        data: HandshakeMessageBody::Welcome {
                            welcome: welcome.clone(),
                            group_id_hex: Some(group_id_hex.clone()),
                        },
                    }),
                );
                self.handshake = HandshakeState::Established;
                self.emit_handshake_phase(HandshakePhase::Finalizing);
                schedule(tx, Operation::ConnectMoq);
                self.mark_member_joined(&self.identity.public_key_hex());
                self.mark_member_joined(&invitee_pub);
                Ok(())
            }
            HandshakeMessageType::RequestWelcome => {
                if let Some(welcome) = self.welcome_json.clone() {
                    let group_id_hex = self.identity.group_id_hex().unwrap_or_default();
                    schedule(
                        tx,
                        Operation::OutgoingHandshake(HandshakeMessage {
                            message_type: HandshakeMessageType::Welcome,
                            data: HandshakeMessageBody::Welcome {
                                welcome,
                                group_id_hex: Some(group_id_hex),
                            },
                        }),
                    );
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn handle_handshake_as_joiner(
        &mut self,
        tx: &UnboundedSender<Operation>,
        message: HandshakeMessage,
    ) -> Result<()> {
        match message.message_type {
            HandshakeMessageType::Welcome => {
                let (welcome, group_id_hex) = match message.data {
                    HandshakeMessageBody::Welcome {
                        welcome,
                        group_id_hex,
                    } => (welcome, group_id_hex),
                    _ => return Err(anyhow!("missing welcome payload")),
                };
                if let Some(export) = self.key_package_cache.clone() {
                    if !export.bundle.is_empty() {
                        let _ = self.identity.import_key_package_bundle(&export.bundle);
                    }
                }
                self.emit_status("Accepting welcome…");
                let accepted_group = self.identity.accept_welcome(&welcome)?;
                self.mark_member_joined(&self.identity.public_key_hex());
                let known_peers = self.session.peer_pubkeys.clone();
                for peer in known_peers {
                    self.peer_pubkeys.insert(peer.clone());
                    self.ensure_member(&peer);
                    if self.admin_pubkeys.contains(&peer) {
                        self.update_member_admin(&peer, true);
                    }
                    self.mark_member_joined(&peer);
                }
                if let Some(provided) = group_id_hex {
                    if provided != accepted_group {
                        log::warn!(
                            "Provided group id {} differs from accepted {}",
                            provided,
                            accepted_group
                        );
                    }
                }
                self.handshake = HandshakeState::Established;
                self.emit_handshake_phase(HandshakePhase::Finalizing);
                schedule(tx, Operation::ConnectMoq);
                schedule(
                    tx,
                    Operation::Emit(ChatEvent::status(format!(
                        "Joined group {}",
                        self.identity.group_id_hex().unwrap_or_default()
                    ))),
                );
                Ok(())
            }
            HandshakeMessageType::RequestKeyPackage => {
                if let Some(export) = self.key_package_cache.clone() {
                    schedule(
                        tx,
                        Operation::OutgoingHandshake(HandshakeMessage {
                            message_type: HandshakeMessageType::KeyPackage,
                            data: HandshakeMessageBody::KeyPackage {
                                event: export.event_json.clone(),
                                bundle: Some(export.bundle.clone()),
                                pubkey: Some(self.identity.public_key_hex()),
                            },
                        }),
                    );
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

fn now_timestamp() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0) as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

fn schedule(tx: &UnboundedSender<Operation>, op: Operation) {
    if let Err(err) = tx.unbounded_send(op) {
        log::error!("operation queue closed: {err}");
    }
}

fn short_key(key: &str) -> String {
    if key.len() <= 12 {
        key.to_string()
    } else {
        format!("{}…{}", &key[..6], &key[key.len() - 4..])
    }
}

fn relay_relays_url(url: &str) -> String {
    url.parse::<url::Url>()
        .map(|parsed| {
            let scheme = if parsed.scheme() == "https" {
                "wss"
            } else {
                "wss"
            };
            format!("{scheme}://{}", parsed.host_str().unwrap_or("localhost"))
        })
        .unwrap_or_else(|_| "wss://localhost".to_string())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::controller::events::{ChatEvent, StubConfig};
    use crate::controller::services::{stub, IdentityService};
    use crate::messages::{WrapperFrame, WrapperKind};

    mod scenario {
        use super::WrapperFrame;
        use super::WrapperKind;

        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/scenario.rs"
        ));
    }
    use scenario::DeterministicScenario;

    #[test]
    fn ingest_backlog_matches_expected_messages() {
        let mut scenario = DeterministicScenario::new().expect("deterministic scenario");
        let config = scenario.config.clone();

        let wrappers = scenario
            .conversation
            .initial_backlog()
            .expect("initial backlog");

        let session = SessionParams {
            bootstrap_role: SessionRole::Invitee,
            relay_url: "stub://relay".to_string(),
            nostr_url: "stub://nostr".to_string(),
            session_id: "test-session".to_string(),
            secret_hex: config.invitee_secret_hex.clone(),
            peer_pubkeys: vec![config.creator_pubkey.clone()],
            group_id_hex: Some(config.group_id_hex.clone()),
            admin_pubkeys: Vec::new(),
            stub: Some(StubConfig::default()),
        };

        let identity = IdentityService::create(&session.secret_hex).expect("identity");
        identity
            .import_key_package_bundle(&config.invitee_key_package.bundle)
            .expect("import bundle");
        let accepted_group = identity
            .accept_welcome(&config.welcome_json)
            .expect("accept welcome");
        assert_eq!(accepted_group, config.group_id_hex);

        let (nostr, moq) = stub::make_stub_services(&session);

        let captured = Rc::new(RefCell::new(Vec::<ChatEvent>::new()));
        let callback = {
            let captured = captured.clone();
            Rc::new(move |event: ChatEvent| {
                captured.borrow_mut().push(event);
            }) as Rc<dyn Fn(ChatEvent)>
        };

        let config = ControllerConfig {
            identity,
            session,
            nostr,
            moq,
            callback,
        };

        let mut state = ControllerState::new(config);

        let mut ordered_events = Vec::new();
        for wrapper in &wrappers {
            let events = state
                .handle_incoming_frame(wrapper.bytes.clone())
                .expect("ingest frame");
            ordered_events.extend(events);
        }

        let expected_messages: Vec<_> = wrappers
            .iter()
            .filter_map(|wrapper| match &wrapper.kind {
                WrapperKind::Application { content, .. } => Some(content.clone()),
                WrapperKind::Commit => None,
            })
            .collect();

        let observed_messages: Vec<_> = ordered_events
            .iter()
            .filter_map(|event| match event {
                ChatEvent::Message { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(
            observed_messages, expected_messages,
            "message order mismatch"
        );

        let commit_events = ordered_events
            .iter()
            .filter(|event| matches!(event, ChatEvent::Commit { .. }))
            .count();
        let expected_commits = wrappers
            .iter()
            .filter(|wrapper| matches!(wrapper.kind, WrapperKind::Commit))
            .count();
        assert_eq!(commit_events, expected_commits, "commit count mismatch");
    }
}
