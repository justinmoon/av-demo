use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use mdk_core::{
    groups::{NostrGroupConfigData, UpdateGroupResult},
    messages::MessageProcessingResult,
    MDK,
};
use mdk_memory_storage::MdkMemoryStorage;
use mdk_storage_traits::{groups::types::Group, GroupId};
use nostr::{Event, EventBuilder, JsonUtil, Kind, PublicKey, SecretKey, Timestamp};
use openmls::group::MlsGroup;
use openmls::prelude::{KeyPackageBundle, OpenMlsProvider};
use openmls_traits::storage::StorageProvider;
use serde::{Deserialize, Serialize};

use crate::messages::{WrapperFrame, WrapperKind};

use super::events::SessionRole;

const DEFAULT_IMAGE_HASH: Option<[u8; 32]> = None;
const DEFAULT_IMAGE_KEY: Option<[u8; 32]> = None;
const DEFAULT_IMAGE_NONCE: Option<[u8; 12]> = None;

#[derive(Debug, Clone)]
pub struct GroupArtifacts {
    pub group_id_hex: String,
    pub welcome: String,
}

#[derive(Debug, Clone)]
pub struct WelcomeArtifact {
    pub welcome: String,
    pub recipient: String,
}

#[derive(Debug, Clone)]
pub struct AddMembersArtifacts {
    pub commit: WrapperFrame,
    pub welcomes: Vec<WelcomeArtifact>,
}

pub struct IdentityHandle {
    pub(crate) keys: nostr::Keys,
    pub(crate) mdk: MDK<MdkMemoryStorage>,
    pub(crate) group_id: Rc<RefCell<Option<GroupId>>>,
}

impl IdentityHandle {
    pub fn public_key_hex(&self) -> String {
        self.keys.public_key().to_hex()
    }

    pub fn group_id_hex(&self) -> Option<String> {
        self.group_id
            .borrow()
            .as_ref()
            .map(|id| hex::encode(id.as_slice()))
    }

    pub fn set_group_id_hex(&self, hex: &str) -> Result<()> {
        let bytes = hex::decode(hex).context("invalid group id hex")?;
        let id = GroupId::from_slice(&bytes);
        *self.group_id.borrow_mut() = Some(id);
        Ok(())
    }

    fn group_id(&self) -> Result<GroupId> {
        self.group_id
            .borrow()
            .clone()
            .ok_or_else(|| anyhow!("group not established"))
    }

    pub fn import_key_package_bundle(&self, bundle_b64: &str) -> Result<()> {
        let bundle_bytes = BASE64
            .decode(bundle_b64)
            .context("invalid key package bundle encoding")?;
        let bundle: KeyPackageBundle =
            serde_json::from_slice(&bundle_bytes).context("parse key package bundle")?;
        let hash_ref = bundle
            .key_package()
            .hash_ref(self.mdk.provider.crypto())
            .context("derive key package hash ref")?;
        self.mdk
            .provider
            .storage()
            .write_key_package::<_, KeyPackageBundle>(&hash_ref, &bundle)
            .map_err(|e| anyhow!("store key package bundle: {:?}", e))?;
        Ok(())
    }

    pub fn create_key_package(&self, relays: &[String]) -> Result<KeyPackageExport> {
        let relays = relays
            .iter()
            .map(|url| nostr::RelayUrl::parse(url))
            .collect::<Result<Vec<_>, _>>()
            .context("parse relay urls")?;
        let (encoded, tags) = self
            .mdk
            .create_key_package_for_event(&self.keys.public_key(), relays)
            .context("create key package")?;
        let event = EventBuilder::new(Kind::MlsKeyPackage, encoded)
            .tags(tags)
            .build(self.keys.public_key())
            .sign_with_keys(&self.keys)
            .context("sign key package")?;
        let bundle = self.export_key_package_bundle(&event.as_json())?;
        Ok(KeyPackageExport {
            event_json: event.as_json(),
            bundle,
        })
    }

    pub fn export_key_package_bundle(&self, event_json: &str) -> Result<String> {
        let event = Event::from_json(event_json).context("parse key package event")?;
        let key_package = self
            .mdk
            .parse_key_package(&event)
            .context("parse key package")?;
        let hash_ref = key_package
            .hash_ref(self.mdk.provider.crypto())
            .context("hash key package")?;
        let bundle = self
            .mdk
            .provider
            .storage()
            .key_package::<_, KeyPackageBundle>(&hash_ref)
            .map_err(|e| anyhow!("load key package bundle: {:?}", e))?
            .ok_or_else(|| anyhow!("key package bundle missing"))?;
        let bytes = serde_json::to_vec(&bundle).context("serialize key package bundle")?;
        Ok(BASE64.encode(bytes))
    }

    pub fn create_group(
        &self,
        invitee_event: &str,
        invitee_pub: &str,
        admin_pubkeys: &[String],
    ) -> Result<GroupArtifacts> {
        let invitee = Event::from_json(invitee_event).context("parse invitee event")?;
        let invitee_pubkey = PublicKey::from_hex(invitee_pub).context("parse invitee pubkey")?;
        let mut admins = vec![self.keys.public_key()];
        for hex in admin_pubkeys {
            if let Ok(pk) = PublicKey::from_hex(hex) {
                if !admins.iter().any(|existing| existing == &pk) {
                    admins.push(pk);
                }
            }
        }
        if !admins.iter().any(|pk| pk == &invitee_pubkey) {
            admins.push(invitee_pubkey);
        }
        let config = NostrGroupConfigData::new(
            "Marmot Chat".to_string(),
            "MoQ/MLS demo".to_string(),
            DEFAULT_IMAGE_HASH,
            DEFAULT_IMAGE_KEY,
            DEFAULT_IMAGE_NONCE,
            vec![],
            admins,
        );
        let result = self
            .mdk
            .create_group(&self.keys.public_key(), vec![invitee], config)
            .context("create group")?;
        let welcome = result
            .welcome_rumors
            .get(0)
            .ok_or_else(|| anyhow!("missing welcome rumor"))?
            .clone();
        let group_id = result.group.mls_group_id.clone();
        *self.group_id.borrow_mut() = Some(group_id.clone());
        Ok(GroupArtifacts {
            group_id_hex: hex::encode(group_id.as_slice()),
            welcome: welcome.as_json(),
        })
    }

    pub fn add_members(&self, key_package_events: &[String]) -> Result<AddMembersArtifacts> {
        let group_id = self.group_id()?;
        let events: Vec<Event> = key_package_events
            .iter()
            .map(|event_json| Event::from_json(event_json).context("parse key package event"))
            .collect::<Result<_, _>>()?;

        let update = self
            .mdk
            .add_members(&group_id, &events)
            .context("add members")?;

        let commit_json = update.evolution_event.as_json();
        let _ = self.ingest_wrapper(commit_json.as_bytes())?;
        self.merge_pending_commit()?;

        let commit_frame = WrapperFrame {
            bytes: commit_json.into_bytes(),
            kind: WrapperKind::Commit,
        };

        let welcomes = update
            .welcome_rumors
            .unwrap_or_default()
            .into_iter()
            .zip(events.iter())
            .map(|(rumor, event)| WelcomeArtifact {
                welcome: rumor.as_json(),
                recipient: event.pubkey.to_hex(),
            })
            .collect();

        Ok(AddMembersArtifacts {
            commit: commit_frame,
            welcomes,
        })
    }

    pub fn accept_welcome(&self, welcome_json: &str) -> Result<String> {
        use nostr::{EventId, UnsignedEvent};

        let welcome_unsigned = UnsignedEvent::from_json(welcome_json.as_bytes())
            .context("parse welcome unsigned event")?;

        self.mdk
            .process_welcome(&EventId::all_zeros(), &welcome_unsigned)
            .context("process welcome")?;

        let mut accepted_group: Option<Group> = None;
        if let Ok(mut welcomes) = self.mdk.get_pending_welcomes() {
            for welcome in welcomes.iter() {
                self.mdk.accept_welcome(welcome).context("accept welcome")?;
            }
            if let Some(latest) = welcomes.pop() {
                if let Ok(groups) = self.mdk.get_groups() {
                    accepted_group = groups
                        .into_iter()
                        .find(|group| group.mls_group_id == latest.mls_group_id);
                }
            }
        }

        let group =
            accepted_group.ok_or_else(|| anyhow!("accepted welcome but group not found"))?;
        *self.group_id.borrow_mut() = Some(group.mls_group_id.clone());
        Ok(hex::encode(group.mls_group_id.as_slice()))
    }

    pub fn ingest_wrapper(&self, bytes: &[u8]) -> Result<WrapperOutcome> {
        let event_json = std::str::from_utf8(bytes).context("wrapper bytes not utf8")?;
        let event = Event::from_json(event_json).context("parse wrapper event")?;
        match self
            .mdk
            .process_message(&event)
            .context("process message")?
        {
            MessageProcessingResult::ApplicationMessage(msg) => Ok(WrapperOutcome::Application {
                author: msg.pubkey.to_hex(),
                content: msg.content,
                created_at: msg.created_at.as_u64(),
            }),
            MessageProcessingResult::Commit => Ok(WrapperOutcome::Commit),
            MessageProcessingResult::Proposal(_)
            | MessageProcessingResult::ExternalJoinProposal => Ok(WrapperOutcome::None),
            MessageProcessingResult::Unprocessable => Ok(WrapperOutcome::None),
        }
    }

    pub fn merge_pending_commit(&self) -> Result<()> {
        let group_id = self.group_id()?;
        self.mdk
            .merge_pending_commit(&group_id)
            .context("merge pending commit")
    }

    pub fn list_members(&self) -> Result<Vec<String>> {
        let group_id = self.group_id()?;
        let members = self.mdk.get_members(&group_id).context("get members")?;
        Ok(members.into_iter().map(|pk| pk.to_hex()).collect())
    }

    pub fn create_message(&self, content: &str) -> Result<WrapperFrame> {
        let rumor = EventBuilder::new(Kind::TextNote, content)
            .custom_created_at(Timestamp::now())
            .build(self.keys.public_key());
        let group_id = self.group_id()?;
        let wrapper = self
            .mdk
            .create_message(&group_id, rumor)
            .context("create message")?;
        Ok(WrapperFrame {
            bytes: wrapper.as_json().into_bytes(),
            kind: WrapperKind::Application {
                author: self.keys.public_key().to_hex(),
                content: content.to_string(),
            },
        })
    }

    pub fn self_update(&self) -> Result<WrapperFrame> {
        let group_id = self.group_id()?;
        let UpdateGroupResult {
            evolution_event, ..
        } = self.mdk.self_update(&group_id).context("self update")?;
        let json = evolution_event.as_json();
        let _event = Event::from_json(&json).context("commit event")?;
        let _ = self.ingest_wrapper(json.as_bytes())?;
        let _ = self.merge_pending_commit()?;
        Ok(WrapperFrame {
            bytes: json.into_bytes(),
            kind: WrapperKind::Commit,
        })
    }

    pub fn derive_group_root(&self) -> Result<String> {
        let group_id = self.group_id()?;
        let mls_group = MlsGroup::load(self.mdk.provider.storage(), group_id.inner())
            .context("load group")?
            .ok_or_else(|| anyhow!("group not found"))?;
        let exported = mls_group
            .export_secret(self.mdk.provider.crypto(), "moq-group-root-v1", &[], 16)
            .context("export group secret")?;
        Ok(format!("marmot/{}", hex::encode(exported)))
    }
}

pub struct IdentityService;

impl IdentityService {
    pub fn create(secret_hex: &str) -> Result<IdentityHandle> {
        let secret = SecretKey::from_hex(secret_hex).context("parse secret hex")?;
        let keys = nostr::Keys::new(secret);
        Ok(IdentityHandle {
            keys,
            mdk: MDK::new(MdkMemoryStorage::default()),
            group_id: Rc::new(RefCell::new(None)),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPackageExport {
    pub event_json: String,
    pub bundle: String,
}

#[derive(Debug, Clone)]
pub enum WrapperOutcome {
    Application {
        author: String,
        content: String,
        created_at: u64,
    },
    Commit,
    None,
}

pub trait HandshakeListener {
    fn on_message(&self, message: HandshakeMessage);
}

pub trait NostrService {
    fn connect(&self, params: HandshakeConnectParams, listener: Box<dyn HandshakeListener>);
    fn send(&self, payload: HandshakeMessage);
    fn shutdown(&self);
}

pub struct HandshakeConnectParams {
    pub url: String,
    pub session: String,
    pub role: SessionRole,
    pub secret_hex: String,
}

#[derive(Debug, Clone)]
pub struct HandshakeMessage {
    pub message_type: HandshakeMessageType,
    pub data: HandshakeMessageBody,
}

#[derive(Debug, Clone)]
pub enum HandshakeMessageType {
    RequestKeyPackage,
    RequestWelcome,
    KeyPackage,
    Welcome,
}

impl HandshakeMessageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            HandshakeMessageType::RequestKeyPackage => "request-key-package",
            HandshakeMessageType::RequestWelcome => "request-welcome",
            HandshakeMessageType::KeyPackage => "key-package",
            HandshakeMessageType::Welcome => "welcome",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "request-key-package" => Some(HandshakeMessageType::RequestKeyPackage),
            "request-welcome" => Some(HandshakeMessageType::RequestWelcome),
            "key-package" => Some(HandshakeMessageType::KeyPackage),
            "welcome" => Some(HandshakeMessageType::Welcome),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum HandshakeMessageBody {
    None,
    Request {
        pubkey: Option<String>,
        is_admin: Option<bool>,
    },
    KeyPackage {
        event: String,
        bundle: Option<String>,
        pubkey: Option<String>,
    },
    Welcome {
        welcome: String,
        group_id_hex: Option<String>,
        recipient: Option<String>,
    },
}

pub trait MoqListener {
    fn on_frame(&self, bytes: Vec<u8>);
    fn on_ready(&self);
    fn on_error(&self, message: String);
    fn on_closed(&self);
}

pub trait MoqService {
    fn connect(
        &self,
        url: &str,
        session: &str,
        own_pubkey: &str,
        peer_pubkeys: &[String],
        listener: Box<dyn MoqListener>,
    );
    fn subscribe_to_peer(&self, peer_pubkey: &str);
    fn publish_wrapper(&self, bytes: &[u8]);
    fn shutdown(&self);
}
