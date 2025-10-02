use futures::channel::mpsc::UnboundedSender;

use crate::controller::events::ChatEvent;

use super::types::{ControllerState, Operation};
use super::utils::schedule;

impl ControllerState {
    pub fn enqueue_outgoing(&mut self, bytes: Vec<u8>) {
        self.outgoing_queue.push_back(bytes);
    }

    pub fn take_next_outgoing(&mut self) -> Option<Vec<u8>> {
        self.outgoing_queue.pop_front()
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
