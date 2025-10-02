use anyhow::Result;

use crate::controller::events::ChatEvent;

use super::types::ControllerState;
use super::utils::now_timestamp;

impl ControllerState {
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
}
