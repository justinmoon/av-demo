use anyhow::Result;
use log::{info, warn};

use crate::controller::events::{ChatEvent, MemberInfo};

use super::types::{ControllerState, MemberRecord};

impl ControllerState {
    pub(super) fn emit_roster(&self) {
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
}
