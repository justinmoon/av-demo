use std::{net::UdpSocket, process::Stdio, sync::Once, time::Duration};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use mdk_core::groups::{NostrGroupConfigData, UpdateGroupResult};
use mdk_core::messages::MessageProcessingResult;
use mdk_core::{GroupId, MDK};
use mdk_memory_storage::MdkMemoryStorage;
use moq_lite::{self, BroadcastConsumer, Track, TrackConsumer};
use moq_native::ClientConfig;
use nostr::{Event, EventBuilder, EventId, JsonUtil, Keys, Kind, RelayUrl, SecretKey};
use sha2::{Digest, Sha256};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};
use tracing::{debug, info};
use url::Url;

const ALICE_SECRET: &str = "4d36e7068b0eeef39b4e2ff1f908db8b27c12075b1219777084ffcf86490b6ae";
const BOB_SECRET: &str = "6e8a52c9ac36ca5293b156d8af4d7f6aeb52208419bd99c75472fc6f4321a5fd";
const RELAY_WORKSPACE: &str = "/Users/justin/code/moq/moq/rs";
const RELAY_BIN: &str = "/Users/justin/code/moq/moq/rs/target/debug/moq-relay";
const BROADCAST_LABEL: &str = "wrappers";
const AUTH_PUBLIC_ROOT: &str = "marmot";

static INIT_LOGGING: Once = Once::new();

fn init_tracing() {
    INIT_LOGGING.call_once(|| {
        let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .init();
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mdk_wrappers_flow_over_local_moq() -> Result<()> {
    init_tracing();

    ensure_moq_relay_built().await?;

    let port = find_free_udp_port()?;
    let mut relay = RelayHandle::spawn(port).await?;

    // Allow the relay to finish its startup before clients connect.
    sleep(Duration::from_millis(500)).await;

    let PreparedState {
        bob_mdk,
        group_id,
        wrappers,
        expected_messages,
    } = prepare_mls_state()?;

    let root = derive_group_root(&group_id);
    let moq_url = Url::parse(&format!("https://127.0.0.1:{}/{}", port, root))?;

    let (start_tx, start_rx) = oneshot::channel();
    let subscriber = tokio::spawn(collect_wrappers(moq_url.clone(), wrappers.len(), start_tx));

    publish_wrappers(moq_url.clone(), wrappers, start_rx).await?;

    let received = subscriber.await.context("subscriber task failed")??;

    let decrypted = process_wrappers(&bob_mdk, &group_id, received)?;

    assert_eq!(decrypted, expected_messages, "decrypted messages mismatch");

    relay.shutdown().await?;
    Ok(())
}

struct PreparedState {
    bob_mdk: MDK<MdkMemoryStorage>,
    group_id: GroupId,
    wrappers: Vec<Vec<u8>>,
    expected_messages: Vec<String>,
}

fn prepare_mls_state() -> Result<PreparedState> {
    let alice_mdk = MDK::new(MdkMemoryStorage::default());
    let bob_mdk = MDK::new(MdkMemoryStorage::default());

    let alice_keys = Keys::new(SecretKey::from_hex(ALICE_SECRET)?);
    let bob_keys = Keys::new(SecretKey::from_hex(BOB_SECRET)?);

    let (encoded, tags) = bob_mdk
        .create_key_package_for_event(&bob_keys.public_key(), Vec::<RelayUrl>::new())
        .context("bob key package")?;

    let bob_key_pkg = EventBuilder::new(Kind::MlsKeyPackage, encoded)
        .tags(tags)
        .build(bob_keys.public_key())
        .sign_with_keys(&bob_keys)
        .context("sign bob key package")?;

    let config = NostrGroupConfigData::new(
        "MoQ Demo".to_string(),
        "Phase 1 Step 3".to_string(),
        None,
        None,
        None,
        vec![],
        vec![alice_keys.public_key(), bob_keys.public_key()],
    );

    let group_result = alice_mdk
        .create_group(&alice_keys.public_key(), vec![bob_key_pkg.clone()], config)
        .context("create group")?;

    let welcome = group_result
        .welcome_rumors
        .first()
        .ok_or_else(|| anyhow!("missing welcome rumor"))?
        .clone();

    bob_mdk
        .process_welcome(&EventId::all_zeros(), &welcome)
        .context("process welcome")?;

    let mut welcomes = bob_mdk
        .get_pending_welcomes()
        .context("get pending welcomes")?;
    let welcome_to_accept = welcomes
        .pop()
        .ok_or_else(|| anyhow!("no pending welcomes"))?;
    bob_mdk
        .accept_welcome(&welcome_to_accept)
        .context("accept welcome")?;

    let group_id = group_result.group.mls_group_id.clone();

    let mut wrappers = Vec::new();
    let mut expected_messages = Vec::new();

    let push_message_wrapper = |content: &str,
                                mdk: &MDK<MdkMemoryStorage>,
                                keys: &Keys,
                                dest: &mut Vec<Vec<u8>>,
                                exp: &mut Vec<String>|
     -> Result<()> {
        let rumor = EventBuilder::new(Kind::TextNote, content).build(keys.public_key());
        let wrapper = mdk
            .create_message(&group_id, rumor)
            .context("create message wrapper")?;
        dest.push(wrapper.as_json().into_bytes());
        exp.push(content.to_string());
        Ok(())
    };

    push_message_wrapper(
        "hello from alice",
        &alice_mdk,
        &alice_keys,
        &mut wrappers,
        &mut expected_messages,
    )?;

    push_message_wrapper(
        "second before rotation",
        &alice_mdk,
        &alice_keys,
        &mut wrappers,
        &mut expected_messages,
    )?;

    let UpdateGroupResult {
        evolution_event: commit_event,
        ..
    } = alice_mdk.self_update(&group_id).context("self update")?;

    wrappers.push(commit_event.as_json().into_bytes());
    let _ = alice_mdk
        .process_message(&commit_event)
        .context("alice ingest own commit")?;
    alice_mdk
        .merge_pending_commit(&group_id)
        .context("alice merge commit")?;

    push_message_wrapper(
        "post rotation message",
        &alice_mdk,
        &alice_keys,
        &mut wrappers,
        &mut expected_messages,
    )?;

    Ok(PreparedState {
        bob_mdk,
        group_id,
        wrappers,
        expected_messages,
    })
}

fn process_wrappers(
    bob_mdk: &MDK<MdkMemoryStorage>,
    group_id: &GroupId,
    frames: Vec<Vec<u8>>,
) -> Result<Vec<String>> {
    let mut decrypted = Vec::new();

    for frame in frames {
        let json = String::from_utf8(frame).context("wrapper not utf-8")?;
        let event = Event::from_json(&json).context("invalid wrapper event")?;

        match bob_mdk.process_message(&event).context("process wrapper")? {
            MessageProcessingResult::ApplicationMessage(message) => {
                decrypted.push(message.content);
            }
            MessageProcessingResult::Commit => {
                bob_mdk
                    .merge_pending_commit(group_id)
                    .context("bob merge commit")?;
            }
            MessageProcessingResult::Proposal(_) => {}
            MessageProcessingResult::ExternalJoinProposal => {}
            MessageProcessingResult::Unprocessable => {
                return Err(anyhow!("unprocessed wrapper"));
            }
        }
    }

    Ok(decrypted)
}

fn derive_group_root(group_id: &GroupId) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"moq-group-root-v1");
    hasher.update(group_id.as_slice());
    let digest = hasher.finalize();
    let label = hex::encode(&digest[..16]);
    format!("{AUTH_PUBLIC_ROOT}/{label}")
}

async fn publish_wrappers(
    url: Url,
    wrappers: Vec<Vec<u8>>,
    start: oneshot::Receiver<()>,
) -> Result<()> {
    let mut client_config = ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    let client = moq_native::Client::new(client_config).context("create publisher client")?;

    let connection = connect_with_retry(&client, &url).await?;

    let moq_lite::Produce {
        producer: origin_producer,
        consumer: origin_consumer,
    } = moq_lite::Origin::produce();
    let session = moq_lite::Session::connect(connection, origin_consumer, None)
        .await
        .context("publisher session")?;
    info!("publisher connected");

    let mut broadcast = moq_lite::Broadcast::produce();
    let mut track = broadcast.producer.create_track(Track {
        name: BROADCAST_LABEL.to_string(),
        priority: 0,
    });

    origin_producer.publish_broadcast("", broadcast.consumer.clone());

    start.await.context("publisher start signal")?;

    for (idx, wrapper) in wrappers.into_iter().enumerate() {
        track.write_frame(Bytes::from(wrapper));
        info!(wrapper_index = idx, "publisher wrote wrapper");
        sleep(Duration::from_millis(20)).await;
    }

    sleep(Duration::from_millis(500)).await;
    session.close(moq_lite::Error::Cancel);
    Ok(())
}

async fn collect_wrappers(
    url: Url,
    expected: usize,
    start_signal: oneshot::Sender<()>,
) -> Result<Vec<Vec<u8>>> {
    let mut client_config = ClientConfig::default();
    client_config.tls.disable_verify = Some(true);
    let client = moq_native::Client::new(client_config).context("create subscriber client")?;

    let connection = connect_with_retry(&client, &url).await?;
    let moq_lite::Produce {
        producer: origin_producer,
        consumer: origin_consumer,
    } = moq_lite::Origin::produce();
    let session = moq_lite::Session::connect(connection, None, Some(origin_producer))
        .await
        .context("subscriber session")?;
    info!("subscriber connected");

    let broadcast = wait_for_broadcast(&origin_consumer, "").await?;
    info!("subscriber broadcast ready");
    let mut track: TrackConsumer = broadcast.subscribe_track(&Track::new(BROADCAST_LABEL));
    start_signal
        .send(())
        .map_err(|_| anyhow!("publisher start receiver dropped"))?;

    let mut frames = Vec::new();
    while frames.len() < expected {
        match track.next_group().await {
            Ok(Some(mut group)) => {
                while let Some(frame) = group.read_frame().await.context("read frame")? {
                    info!(received_bytes = frame.len(), "subscriber received frame");
                    frames.push(frame.to_vec());
                }
            }
            Ok(None) => break,
            Err(moq_lite::Error::Cancel) => break,
            Err(err) => return Err(anyhow!("next group failed: {err}")),
        }
    }

    session.close(moq_lite::Error::Cancel);
    if frames.len() < expected {
        anyhow::bail!("received {} frames, expected {}", frames.len(), expected);
    }

    Ok(frames)
}

async fn wait_for_broadcast(
    origin: &moq_lite::OriginConsumer,
    path: &str,
) -> Result<BroadcastConsumer> {
    loop {
        if let Some(broadcast) = origin.consume_broadcast(path) {
            return Ok(broadcast);
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn connect_with_retry(
    client: &moq_native::Client,
    url: &Url,
) -> Result<web_transport_quinn::Session> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..10 {
        info!(attempt, "connecting to {url}");
        match client.connect(url.clone()).await {
            Ok(conn) => return Ok(conn),
            Err(err) => {
                last_err = Some(err);
                sleep(Duration::from_millis(200)).await;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("failed to connect to {url}")))
}

async fn ensure_moq_relay_built() -> Result<()> {
    let status = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("moq-relay")
        .arg("--quiet")
        .current_dir(RELAY_WORKSPACE)
        .status()
        .await
        .context("build moq-relay")?;

    anyhow::ensure!(status.success(), "cargo build failed for moq-relay");
    Ok(())
}

fn find_free_udp_port() -> Result<u16> {
    let socket = UdpSocket::bind("127.0.0.1:0").context("bind probe socket")?;
    let port = socket.local_addr().context("probe addr")?.port();
    drop(socket);
    Ok(port)
}

struct RelayHandle {
    child: Option<Child>,
}

impl RelayHandle {
    async fn spawn(port: u16) -> Result<Self> {
        let mut cmd = Command::new(RELAY_BIN);
        cmd.arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--tls-generate")
            .arg("localhost")
            .arg("--auth-public")
            .arg(AUTH_PUBLIC_ROOT)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(RELAY_WORKSPACE);

        info!(port, "starting moq-relay");
        let mut child = cmd.spawn().context("spawn moq-relay")?;

        // Give the relay up to 2 seconds to start listening.
        timeout(Duration::from_secs(2), async {
            loop {
                // Poll once with a short sleep; if the process exited we will notice below
                sleep(Duration::from_millis(50)).await;
                if let Some(status) = child.try_wait().context("poll relay status")? {
                    return Err(anyhow!("moq-relay exited early with status {status}"));
                }
                // We don't have a direct readiness signal; rely on the short delay.
                if child.id().is_some() {
                    break;
                }
            }
            Ok(())
        })
        .await
        .unwrap_or(Ok(()))?;

        debug!(port, "moq-relay running");
        Ok(Self { child: Some(child) })
    }

    async fn shutdown(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            child.start_kill().ok();
            let _ = child.wait().await;
        }
        Ok(())
    }
}

impl Drop for RelayHandle {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}
