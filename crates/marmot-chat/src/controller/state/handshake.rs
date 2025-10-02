use anyhow::{anyhow, Context, Result};
use futures::channel::mpsc::UnboundedSender;
use log::{debug, info};

use nostr::prelude::*;

use crate::controller::events::{ChatEvent, HandshakePhase, SessionRole};
use crate::controller::services::{
    GroupArtifacts, HandshakeConnectParams, HandshakeListener, HandshakeMessage,
    HandshakeMessageBody, HandshakeMessageType, KeyPackageExport,
};

use super::core::{ControllerState, HandshakeState, Operation, PendingInvite};
use super::utils::{relay_relays_url, schedule, short_key};

impl ControllerState {
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

        info!(
            "controller: request_invite start pubkey_input={} is_admin={} ready={} handshake_state={:?}",
            trimmed,
            is_admin,
            self.ready,
            self.handshake
        );

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
            info!(
                "controller: request_invite abort pubkey={} already joined",
                pubkey
            );
            return Err(anyhow!("member already present"));
        }

        if self.pending_invites.contains_key(&pubkey) {
            info!(
                "controller: request_invite abort pubkey={} already pending",
                pubkey
            );
            return Err(anyhow!("invite already pending"));
        }

        self.peer_pubkeys.insert(pubkey.clone());
        self.pending_invites
            .insert(pubkey.clone(), PendingInvite { is_admin });

        debug!(
            "controller: request_invite pending_invites now {:?}",
            self.pending_invites.keys().collect::<Vec<_>>()
        );

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

        info!(
            "controller: request_invite queued handshake pubkey={} is_admin={} pending_invites={}",
            pubkey,
            is_admin,
            self.pending_invites.len()
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
        info!(
            "controller: handle_member_addition pubkey={} pending_admin={} handshake_state={:?}",
            invitee_pub,
            self.pending_invites
                .get(&invitee_pub)
                .map(|invite| invite.is_admin)
                .unwrap_or(false),
            self.handshake
        );
        let requested_admin = self
            .pending_invites
            .remove(&invitee_pub)
            .map(|invite| invite.is_admin)
            .unwrap_or(false);

        self.peer_pubkeys.insert(invitee_pub.clone());

        let artifacts = self
            .identity
            .add_members(&[event_json.clone()])
            .map_err(|err| anyhow!("add members failed: {err}"))?;

        info!(
            "controller: add_members produced commit_bytes={} welcomes={} total_members={}",
            artifacts.commit.bytes.len(),
            artifacts.welcomes.len(),
            self.members.len()
        );

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
                        recipient: Some(welcome.recipient.clone()),
                    },
                }),
            );
            (self.callback)(ChatEvent::InviteGenerated {
                welcome: welcome.welcome,
                recipient: welcome.recipient.clone(),
                is_admin: self.admin_pubkeys.contains(&welcome.recipient),
            });
        }

        info!("controller: welcome dispatched to {}", invitee_pub);

        // Sync members from MDK to update roster and subscribe to new peer's MoQ track
        self.sync_members_from_identity()?;

        // Apply admin status after member is created
        if requested_admin {
            self.update_member_admin(&invitee_pub, true);
        }

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
                let relays = vec![relay_relays_url(&self.session.relay_url)];
                let export = self.identity.create_key_package(&relays)?;
                self.key_package_cache = Some(export);
                self.emit_handshake_phase(HandshakePhase::WaitingForWelcome);
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
                            recipient: Some(invitee_pub.clone()),
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
                let target_pub = match message.data.clone() {
                    HandshakeMessageBody::Request { pubkey, .. } => pubkey,
                    _ => None,
                };
                if let Some(welcome) = self.welcome_json.clone() {
                    let group_id_hex = self.identity.group_id_hex().unwrap_or_default();
                    schedule(
                        tx,
                        Operation::OutgoingHandshake(HandshakeMessage {
                            message_type: HandshakeMessageType::Welcome,
                            data: HandshakeMessageBody::Welcome {
                                welcome,
                                group_id_hex: Some(group_id_hex),
                                recipient: target_pub,
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
                        recipient,
                    } => {
                        if let Some(recipient) = recipient {
                            if recipient != self.identity.public_key_hex() {
                                return Ok(());
                            }
                        }
                        (welcome, group_id_hex)
                    }
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

                // Sync all members from MDK to get complete peer list before connecting to MoQ
                if let Ok(all_members) = self.identity.list_members() {
                    for pubkey in all_members {
                        if pubkey != self.identity.public_key_hex() {
                            self.peer_pubkeys.insert(pubkey.clone());
                            self.ensure_member(&pubkey);
                            self.mark_member_joined(&pubkey);
                        }
                    }
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
                let target_pub = match message.data.clone() {
                    HandshakeMessageBody::Request { pubkey, .. } => pubkey,
                    _ => None,
                };
                if let Some(target) = target_pub {
                    if target != self.identity.public_key_hex() {
                        return Ok(());
                    }
                }
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
