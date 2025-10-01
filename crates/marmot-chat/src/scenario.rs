use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use mdk_core::{
    groups::{NostrGroupConfigData, UpdateGroupResult},
    messages::MessageProcessingResult,
    MDK,
};
use mdk_memory_storage::MdkMemoryStorage;
use mdk_storage_traits::GroupId;
use nostr::{Event, EventBuilder, JsonUtil, Keys, Kind, RelayUrl, SecretKey, Timestamp};
use openmls::group::MlsGroup;
use openmls::prelude::{KeyPackageBundle, OpenMlsProvider};
use openmls_traits::storage::StorageProvider;

pub const ALICE_SECRET: &str = "4d36e7068b0eeef39b4e2ff1f908db8b27c12075b1219777084ffcf86490b6ae";
pub const BOB_SECRET: &str = "6e8a52c9ac36ca5293b156d8af4d7f6aeb52208419bd99c75472fc6f4321a5fd";

const ROOT_PREFIX: &str = "marmot";
const DEFAULT_TRACK: &str = "wrappers";
const DEFAULT_INBOX_TRACK: &str = "wrappers-inbox";

#[derive(Debug, Clone)]
pub struct ConfigKeyPackage {
    pub event_json: String,
    pub event: Event,
    pub bundle: String,
}

#[derive(Debug, Clone)]
pub struct Phase4Config {
    pub welcome_json: String,
    pub bob_key_package: ConfigKeyPackage,
    pub bob_secret_hex: String,
    pub alice_pubkey: String,
    pub bob_pubkey: String,
    pub group_id: GroupId,
    pub group_id_hex: String,
    pub group_root: String,
    pub wrappers_track: String,
    pub inbox_track: String,
}

pub struct Phase4Scenario {
    pub config: Phase4Config,
    pub conversation: Conversation,
}

impl Phase4Scenario {
    pub fn new() -> Result<Self> {
        let mut alice = Participant::new("Alice", ALICE_SECRET)?;
        let bob_keys = Keys::new(SecretKey::from_hex(BOB_SECRET)?);

        let key_pkg = alice.generate_member_key_package(&bob_keys)?;

        let config = NostrGroupConfigData::new(
            "MoQ Demo".to_string(),
            "Phase 1 Step 4".to_string(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://relay.example.com").unwrap()],
            vec![alice.keys.public_key(), bob_keys.public_key()],
        );

        let group_result = alice
            .mdk
            .create_group(
                &alice.keys.public_key(),
                vec![key_pkg.event.clone()],
                config,
            )
            .context("alice create group")?;

        let welcome = group_result
            .welcome_rumors
            .first()
            .ok_or_else(|| anyhow!("missing welcome"))?
            .clone();
        let welcome_json = welcome.as_json();

        let group_id = group_result.group.mls_group_id.clone();
        let group_id_hex = hex::encode(group_id.as_slice());
        let group_root = derive_group_root(&alice.mdk, &group_id).context("derive group root")?;

        let alice_pubkey = alice.keys.public_key().to_hex();
        let bob_pubkey = bob_keys.public_key().to_hex();

        let conversation = Conversation::new(group_id.clone(), alice);

        let config = Phase4Config {
            welcome_json,
            bob_key_package: key_pkg,
            bob_secret_hex: BOB_SECRET.to_string(),
            alice_pubkey,
            bob_pubkey,
            group_id,
            group_id_hex,
            group_root,
            wrappers_track: DEFAULT_TRACK.to_string(),
            inbox_track: DEFAULT_INBOX_TRACK.to_string(),
        };

        Ok(Self {
            config,
            conversation,
        })
    }
}

pub struct Conversation {
    group_id: GroupId,
    alice: Participant,
    live_counter: u64,
}

impl Conversation {
    pub fn new(group_id: GroupId, alice: Participant) -> Self {
        Self {
            group_id,
            alice,
            live_counter: 0,
        }
    }

    pub fn initial_backlog(&mut self) -> Result<Vec<WrapperFrame>> {
        let mut frames = Vec::new();
        frames
            .push(self.send_message("Hello from Alice over MoQ!", Timestamp::from(1_700_000_001))?);
        frames.push(self.send_message(
            "Second message before rotation",
            Timestamp::from(1_700_000_002),
        )?);
        frames.push(self.rotate_epoch()?);
        frames.push(self.send_message(
            "Post-rotation hello from Alice",
            Timestamp::from(1_700_000_003),
        )?);
        Ok(frames)
    }

    pub fn next_live_frame(&mut self) -> Result<WrapperFrame> {
        self.live_counter += 1;
        let content = format!("Live message {} from Alice", self.live_counter);
        self.send_message(&content, Timestamp::now())
    }

    pub fn group_id(&self) -> &GroupId {
        &self.group_id
    }

    pub fn alice_mut(&mut self) -> &mut Participant {
        &mut self.alice
    }

    fn send_message(&mut self, content: &str, timestamp: Timestamp) -> Result<WrapperFrame> {
        let rumor = EventBuilder::new(Kind::TextNote, content)
            .custom_created_at(timestamp)
            .build(self.alice.keys.public_key());

        let wrapper = self
            .alice
            .mdk
            .create_message(&self.group_id, rumor)
            .context("alice create message")?;

        Ok(WrapperFrame {
            bytes: wrapper.as_json().into_bytes(),
            kind: WrapperKind::Application {
                author: self.alice.keys.public_key().to_hex(),
                content: content.to_string(),
            },
        })
    }

    fn rotate_epoch(&mut self) -> Result<WrapperFrame> {
        let UpdateGroupResult {
            evolution_event, ..
        } = self
            .alice
            .mdk
            .self_update(&self.group_id)
            .context("alice self update")?;

        let json = evolution_event.as_json();
        let event = Event::from_json(&json).context("commit event")?;
        let _ = self.alice.ingest(&event, &self.group_id)?;

        Ok(WrapperFrame {
            bytes: json.into_bytes(),
            kind: WrapperKind::Commit,
        })
    }
}

pub struct Participant {
    pub name: &'static str,
    pub keys: Keys,
    pub mdk: MDK<MdkMemoryStorage>,
}

impl Participant {
    fn new(name: &'static str, secret_hex: &'static str) -> Result<Self> {
        let secret = SecretKey::from_hex(secret_hex).context("parse secret key")?;
        let keys = Keys::new(secret);
        Ok(Self {
            name,
            keys,
            mdk: MDK::new(MdkMemoryStorage::default()),
        })
    }

    fn generate_member_key_package(&mut self, member_keys: &Keys) -> Result<ConfigKeyPackage> {
        let (encoded, tags) = self
            .mdk
            .create_key_package_for_event(&member_keys.public_key(), Vec::new())
            .context("create key package")?;

        let event = EventBuilder::new(Kind::MlsKeyPackage, encoded)
            .tags(tags)
            .build(member_keys.public_key())
            .sign_with_keys(member_keys)
            .context("sign key package")?;

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
        let bundle_bytes = serde_json::to_vec(&bundle).context("serialize key package bundle")?;
        let bundle_b64 = BASE64.encode(bundle_bytes);

        let event_json = event.as_json();

        Ok(ConfigKeyPackage {
            event_json,
            event,
            bundle: bundle_b64,
        })
    }

    pub fn ingest(
        &mut self,
        event: &Event,
        group_id: &GroupId,
    ) -> Result<Option<DecryptedApplication>> {
        match self
            .mdk
            .process_message(event)
            .with_context(|| format!("{} process message", self.name))?
        {
            MessageProcessingResult::ApplicationMessage(msg) => Ok(Some(DecryptedApplication {
                content: msg.content,
                author: msg.pubkey.to_hex(),
                created_at: msg.created_at.as_u64(),
            })),
            MessageProcessingResult::Commit => {
                self.mdk
                    .merge_pending_commit(group_id)
                    .with_context(|| format!("{} merge commit", self.name))?;
                Ok(None)
            }
            MessageProcessingResult::Proposal(_) => Ok(None),
            MessageProcessingResult::ExternalJoinProposal => Ok(None),
            MessageProcessingResult::Unprocessable => {
                anyhow::bail!("{} unable to process wrapper", self.name);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecryptedApplication {
    pub content: String,
    pub author: String,
    pub created_at: u64,
}

#[derive(Clone)]
pub struct WrapperFrame {
    pub bytes: Vec<u8>,
    pub kind: WrapperKind,
}

#[derive(Clone)]
pub enum WrapperKind {
    Application { author: String, content: String },
    Commit,
}

impl WrapperKind {
    pub fn label(&self) -> &'static str {
        match self {
            WrapperKind::Application { .. } => "application",
            WrapperKind::Commit => "commit",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            WrapperKind::Application { author, content } => format!("{author}: {content}"),
            WrapperKind::Commit => "commit".to_string(),
        }
    }
}

fn derive_group_root(mdk: &MDK<MdkMemoryStorage>, group_id: &GroupId) -> Result<String> {
    let mls_group = MlsGroup::load(mdk.provider.storage(), group_id.inner())
        .context("load MLS group")?
        .ok_or_else(|| anyhow!("group not found"))?;
    let exported = mls_group
        .export_secret(mdk.provider.crypto(), "moq-group-root-v1", &[], 16)
        .context("export moq group root")?;
    Ok(format!("{ROOT_PREFIX}/{}", hex::encode(exported)))
}
