use anyhow::Result;
use log::{info, warn};

use crate::controller::events::{ChatEvent, MemberInfo};

use super::types::ControllerState;
use super::utils::short_key;

impl ControllerState {
    pub(super) fn emit_roster(&self) {
        let all_members = match self.identity.list_members() {
            Ok(members) => members,
            Err(err) => {
                warn!("Failed to list members: {err:#}");
                return;
            }
        };

        let members: Vec<MemberInfo> = all_members
            .into_iter()
            .map(|pubkey| MemberInfo {
                pubkey: pubkey.clone(),
                is_admin: self.admin_pubkeys.contains(&pubkey),
            })
            .collect();

        if !members.is_empty() {
            (self.callback)(ChatEvent::Roster { members });
        }
    }

    pub(super) fn notify_new_member(&self, pubkey: &str) {
        let member = MemberInfo {
            pubkey: pubkey.to_string(),
            is_admin: self.admin_pubkeys.contains(pubkey),
        };
        (self.callback)(ChatEvent::MemberJoined { member });
        self.emit_roster();
    }

    pub(super) fn update_member_admin(&mut self, pubkey: &str, is_admin: bool) {
        if is_admin {
            self.admin_pubkeys.insert(pubkey.to_string());
        } else {
            self.admin_pubkeys.remove(pubkey);
        }

        match self.identity.list_members() {
            Ok(members) => {
                if members.iter().any(|member| member == pubkey) {
                    let member = MemberInfo {
                        pubkey: pubkey.to_string(),
                        is_admin,
                    };
                    (self.callback)(ChatEvent::MemberUpdated {
                        member: member.clone(),
                    });
                    self.emit_roster();
                }
            }
            Err(err) => {
                warn!("Failed to list members while updating admin: {err:#}");
            }
        }
    }

    pub(super) fn sync_members_from_identity(&mut self) -> Result<()> {
        let members = match self.identity.list_members() {
            Ok(members) => members,
            Err(err) => {
                warn!("Failed to list members during sync: {err:#}");
                return Ok(());
            }
        };
        let own_pubkey = self.identity.public_key_hex();
        for pubkey in members {
            if pubkey != own_pubkey && !self.subscribed_peers.contains(&pubkey) {
                info!(
                    "Syncing members: subscribing to peer {}",
                    short_key(&pubkey)
                );
                self.moq.subscribe_to_peer(&pubkey);
                self.subscribed_peers.insert(pubkey.clone());
                self.notify_new_member(&pubkey);
            }
        }
        Ok(())
    }
}
