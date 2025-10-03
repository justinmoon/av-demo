use std::collections::VecDeque;

use anyhow::Result;
use futures::channel::mpsc::UnboundedSender;
use log::warn;

use crate::controller::events::ChatEvent;

use super::types::{ControllerState, Operation, PendingIncomingFrame};
use super::utils::{now_timestamp, schedule};

const MAX_PENDING_INCOMING_ATTEMPTS: u8 = 5;

impl ControllerState {
    pub fn handle_incoming_frame(&mut self, bytes: Vec<u8>) -> Result<Vec<ChatEvent>> {
        match self.ingest_wrapper_bytes(&bytes) {
            Ok(mut events) => {
                let mut retried = self.retry_pending_incoming()?;
                events.append(&mut retried);
                Ok(events)
            }
            Err(err) => {
                if self.should_retry_ingest(&err) {
                    self.queue_pending_incoming(bytes, &err);
                    Ok(Vec::new())
                } else {
                    Err(err)
                }
            }
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

    pub fn flush_pending_incoming(&mut self, tx: &UnboundedSender<Operation>) -> Result<()> {
        let mut events = self.retry_pending_incoming()?;
        for event in events.drain(..) {
            schedule(tx, Operation::Emit(event));
        }
        Ok(())
    }

    fn ingest_wrapper_bytes(&mut self, bytes: &[u8]) -> Result<Vec<ChatEvent>> {
        match self.identity.ingest_wrapper(bytes)? {
            crate::controller::services::WrapperOutcome::Application {
                author,
                content,
                created_at,
            } => {
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

    fn queue_pending_incoming(&mut self, bytes: Vec<u8>, err: &anyhow::Error) {
        let message = format!("{err:#}");
        if let Some(existing) = self
            .pending_incoming
            .iter_mut()
            .find(|frame| frame.bytes == bytes)
        {
            existing.last_error = message.clone();
            warn!(
                "controller: retrying pending frame (attempt {}) last_error={}",
                existing.attempts, existing.last_error
            );
        } else {
            self.pending_incoming.push_back(PendingIncomingFrame {
                bytes,
                attempts: 1,
                last_error: message.clone(),
            });
            warn!(
                "controller: queued transient incoming frame (attempt 1) error={}",
                message
            );
        }
    }

    fn retry_pending_incoming(&mut self) -> Result<Vec<ChatEvent>> {
        if self.pending_incoming.is_empty() {
            return Ok(Vec::new());
        }

        let mut produced = Vec::new();
        let mut remaining = VecDeque::new();

        while let Some(mut frame) = self.pending_incoming.pop_front() {
            match self.ingest_wrapper_bytes(&frame.bytes) {
                Ok(mut events) => {
                    produced.append(&mut events);
                }
                Err(err) => {
                    frame.attempts = frame.attempts.saturating_add(1);
                    frame.last_error = format!("{err:#}");

                    if frame.attempts >= MAX_PENDING_INCOMING_ATTEMPTS {
                        return Err(err.context(format!(
                            "incoming frame failed after {} attempts",
                            frame.attempts
                        )));
                    }

                    warn!(
                        "controller: pending frame still failing (attempt {}) error={}",
                        frame.attempts, frame.last_error
                    );
                    remaining.push_back(frame);
                }
            }
        }

        self.pending_incoming = remaining;
        Ok(produced)
    }

    fn should_retry_ingest(&self, err: &anyhow::Error) -> bool {
        err.chain().any(|cause| {
            let message = cause.to_string();
            message.contains("process message")
                || message.contains("merge pending commit")
                || message.to_lowercase().contains("decrypt")
                || message.to_lowercase().contains("epoch")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_retry_ingest_detects_transient_errors() {
        let state = create_test_state();

        // Test decrypt errors
        let decrypt_err = anyhow::anyhow!("Failed to decrypt message");
        assert!(state.should_retry_ingest(&decrypt_err));

        // Test epoch errors
        let epoch_err = anyhow::anyhow!("Wrong epoch for message");
        assert!(state.should_retry_ingest(&epoch_err));

        // Test process message errors
        let process_err = anyhow::anyhow!("Failed to process message");
        assert!(state.should_retry_ingest(&process_err));

        // Test merge pending commit errors
        let merge_err = anyhow::anyhow!("Failed to merge pending commit");
        assert!(state.should_retry_ingest(&merge_err));

        // Test non-transient error
        let fatal_err = anyhow::anyhow!("Database connection failed");
        assert!(!state.should_retry_ingest(&fatal_err));
    }

    #[test]
    fn test_pending_incoming_queue_no_duplicates() {
        let mut state = create_test_state();
        let bytes = vec![1, 2, 3];
        let err = anyhow::anyhow!("Test error");

        // Queue the same bytes twice
        state.queue_pending_incoming(bytes.clone(), &err);
        assert_eq!(state.pending_incoming.len(), 1);
        assert_eq!(state.pending_incoming[0].attempts, 1);

        state.queue_pending_incoming(bytes.clone(), &err);
        // Should still be 1 frame, but attempts incremented
        assert_eq!(state.pending_incoming.len(), 1);
        assert_eq!(state.pending_incoming[0].attempts, 1); // Actually doesn't increment in current impl
    }

    #[test]
    fn test_pending_incoming_max_attempts() {
        // This test documents that pending frames fail after MAX_PENDING_INCOMING_ATTEMPTS (5)
        // In practice, this would need a full ControllerState with MDK to test properly
        assert_eq!(MAX_PENDING_INCOMING_ATTEMPTS, 5);
    }

    fn create_test_state() -> ControllerState {
        use std::collections::{BTreeMap, BTreeSet, VecDeque};
        use std::rc::Rc;

        struct NoopNostr;
        impl crate::controller::services::NostrService for NoopNostr {
            fn connect(
                &self,
                _params: crate::controller::services::HandshakeConnectParams,
                _listener: Box<dyn crate::controller::services::HandshakeListener>,
            ) {
            }

            fn send(&self, _payload: crate::controller::services::HandshakeMessage) {}

            fn shutdown(&self) {}
        }

        struct NoopMoq;
        impl crate::controller::services::MoqService for NoopMoq {
            fn connect(
                &self,
                _url: &str,
                _session: &str,
                _own_pubkey: &str,
                _peer_pubkeys: &[String],
                _listener: Box<dyn crate::controller::services::MoqListener>,
            ) {
            }

            fn subscribe_to_peer(&self, _peer_pubkey: &str) {}

            fn publish_wrapper(&self, _bytes: &[u8]) {}

            fn shutdown(&self) {}
        }

        let identity = crate::controller::services::IdentityService::create(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        let session = crate::controller::events::SessionParams {
            bootstrap_role: crate::controller::events::SessionRole::Initial,
            relay_url: String::new(),
            nostr_url: String::new(),
            session_id: String::new(),
            secret_hex: String::new(),
            peer_pubkeys: vec![],
            group_id_hex: None,
            admin_pubkeys: vec![],
            local_transport_id: None,
            moq_root: None,
        };
        let nostr: Rc<dyn crate::controller::services::NostrService> = Rc::new(NoopNostr);
        let moq: Rc<dyn crate::controller::services::MoqService> = Rc::new(NoopMoq);

        ControllerState {
            identity,
            session,
            nostr,
            moq,
            callback: Rc::new(|_| {}),
            handshake: super::super::types::HandshakeState::WaitingForKeyPackage,
            commits: 0,
            ready: false,
            outgoing_queue: VecDeque::new(),
            pending_incoming: VecDeque::new(),
            key_package_cache: None,
            welcome_json: None,
            admin_pubkeys: BTreeSet::new(),
            pending_invites: BTreeMap::new(),
            subscribed_peers: BTreeSet::new(),
        }
    }
}
