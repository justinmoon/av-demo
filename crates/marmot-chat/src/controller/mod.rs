mod error;
pub mod events;
pub mod services;
mod state;

pub use state::{ControllerConfig, ControllerState};

use std::cell::RefCell;
use std::rc::Rc;

use error::{ControllerError, ErrorSeverity, ErrorStage};
use events::{ChatEvent, RecoveryAction, SessionParams};
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use log::warn;
use state::Operation;

use services::{HandshakeListener, HandshakeMessage, MoqListener};

pub struct ChatController {
    state: Rc<RefCell<ControllerState>>,
    op_tx: UnboundedSender<Operation>,
}

impl ChatController {
    pub fn new(config: ControllerConfig) -> Self {
        let state = Rc::new(RefCell::new(ControllerState::new(config)));
        let (op_tx, op_rx) = unbounded();
        let runtime = ChatRuntime::new(state.clone(), op_tx.clone());
        runtime.spawn(op_rx);
        Self { state, op_tx }
    }

    pub fn session(&self) -> SessionParams {
        self.state.borrow().session.clone()
    }

    pub fn start(&self) {
        let _ = self.op_tx.unbounded_send(Operation::Start);
    }

    pub fn send_text(&self, content: String) {
        let _ = self.op_tx.unbounded_send(Operation::SendText(content));
    }

    pub fn rotate_epoch(&self) {
        let _ = self.op_tx.unbounded_send(Operation::RotateEpoch);
    }

    pub fn shutdown(&self) {
        let _ = self.op_tx.unbounded_send(Operation::Shutdown);
    }

    pub fn invite_member(&self, pubkey: String, is_admin: bool) {
        let _ = self
            .op_tx
            .unbounded_send(Operation::InviteMember { pubkey, is_admin });
    }

    pub(crate) fn state(&self) -> Rc<RefCell<ControllerState>> {
        self.state.clone()
    }
}

struct ChatRuntime {
    state: Rc<RefCell<ControllerState>>,
    op_tx: UnboundedSender<Operation>,
}

impl ChatRuntime {
    fn new(state: Rc<RefCell<ControllerState>>, op_tx: UnboundedSender<Operation>) -> Self {
        Self { state, op_tx }
    }

    fn spawn(self, op_rx: UnboundedReceiver<Operation>) {
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(self.run(op_rx));

        #[cfg(not(target_arch = "wasm32"))]
        futures::executor::LocalPool::new().run_until(self.run(op_rx));
    }

    async fn run(mut self, mut op_rx: UnboundedReceiver<Operation>) {
        while let Some(operation) = op_rx.next().await {
            self.handle_operation(operation);
        }
    }

    fn handle_operation(&mut self, operation: Operation) {
        match operation {
            Operation::Start => {
                let listener: Box<dyn HandshakeListener> = Box::new(ControllerHandshakeListener {
                    op_tx: self.op_tx.clone(),
                });
                if let Err(err) = self.state.borrow_mut().on_start(&self.op_tx, listener) {
                    self.emit_error(self.classify_handshake_error(
                        err,
                        "Handshake failed to start. Refresh the page or request a new invite.",
                    ));
                }
            }
            Operation::OutgoingHandshake(message) => {
                self.state.borrow().nostr.send(message);
            }
            Operation::IncomingHandshake(message) => {
                if let Err(err) = self
                    .state
                    .borrow_mut()
                    .on_incoming_handshake(&self.op_tx, message)
                {
                    self.emit_error(self.classify_handshake_error(
                        err,
                        "Handshake message failed. Refresh the page or request a new invite.",
                    ));
                }
            }
            Operation::ConnectMoq => {
                let listener: Box<dyn MoqListener> = Box::new(ControllerMoqListener {
                    op_tx: self.op_tx.clone(),
                });
                let state_ref = self.state.borrow();
                let own_pubkey = state_ref.identity.public_key_hex();
                let peer_pubkeys: Vec<String> = match state_ref.identity.list_members() {
                    Ok(members) => members
                        .into_iter()
                        .filter(|pubkey| pubkey != &own_pubkey)
                        .collect(),
                    Err(err) => {
                        warn!("controller: failed to list members for MoQ connect: {err:#}");
                        state_ref
                            .session
                            .peer_pubkeys
                            .iter()
                            .filter(|pubkey| *pubkey != &own_pubkey)
                            .cloned()
                            .collect()
                    }
                };
                // Use MLS-derived moq_root if available, otherwise fall back to session_id
                let moq_path = state_ref
                    .session
                    .moq_root
                    .as_deref()
                    .unwrap_or(&state_ref.session.session_id);
                state_ref.moq.connect(
                    &state_ref.session.relay_url,
                    moq_path,
                    &own_pubkey,
                    &peer_pubkeys,
                    listener,
                );
            }
            Operation::IncomingFrame(bytes) => {
                let events_result = self.state.borrow_mut().handle_incoming_frame(bytes);
                match events_result {
                    Ok(events) => {
                        for event in events {
                            let _ = self.op_tx.unbounded_send(Operation::Emit(event));
                        }
                    }
                    Err(err) => self.emit_error(
                        ControllerError::fatal(ErrorStage::Messaging, err).with_user_message(
                            "Failed to process encrypted update. Refresh or request a new invite.",
                        ),
                    ),
                }
            }
            Operation::PublishWrapper(bytes) => {
                self.state.borrow_mut().publish_or_queue(bytes);
            }
            Operation::Ready => {
                self.state.borrow_mut().on_ready(&self.op_tx);
            }
            Operation::Emit(event) => {
                (self.state.borrow().callback)(event);
            }
            Operation::SendText(content) => {
                let result = self.state.borrow_mut().handle_outgoing_message(&content);
                match result {
                    Ok((bytes, event)) => {
                        let _ = self.op_tx.unbounded_send(Operation::PublishWrapper(bytes));
                        let _ = self.op_tx.unbounded_send(Operation::Emit(event));
                    }
                    Err(err) => self.emit_error(
                        ControllerError::fatal(ErrorStage::Messaging, err).with_user_message(
                            "Failed to send message. Refresh the page and try again.",
                        ),
                    ),
                }
            }
            Operation::RotateEpoch => {
                let result = self.state.borrow_mut().handle_self_update();
                match result {
                    Ok((bytes, events)) => {
                        let _ = self.op_tx.unbounded_send(Operation::PublishWrapper(bytes));
                        for event in events {
                            let _ = self.op_tx.unbounded_send(Operation::Emit(event));
                        }
                    }
                    Err(err) => self.emit_error(
                        ControllerError::fatal(ErrorStage::Messaging, err).with_user_message(
                            "Epoch rotation failed. Refresh the page and try again.",
                        ),
                    ),
                }
            }
            Operation::InviteMember { pubkey, is_admin } => {
                if let Err(err) =
                    self.state
                        .borrow_mut()
                        .request_invite(&self.op_tx, pubkey, is_admin)
                {
                    self.emit_error(self.classify_invite_error(err));
                }
            }
            Operation::Shutdown => {
                self.state.borrow().moq.shutdown();
                self.state.borrow().nostr.shutdown();
            }
        }
    }

    fn classify_handshake_error(
        &self,
        err: anyhow::Error,
        default_message: &'static str,
    ) -> ControllerError {
        let is_history_failure = err
            .chain()
            .any(|cause| cause.to_string().contains("incoming frame failed"));
        if is_history_failure {
            ControllerError::fatal(ErrorStage::Messaging, err).with_user_message(
                "Failed to catch up on encrypted history. Refresh or request a new invite.",
            )
        } else {
            ControllerError::fatal(ErrorStage::Handshake, err).with_user_message(default_message)
        }
    }

    fn classify_invite_error(&self, err: anyhow::Error) -> ControllerError {
        let message_text = err.to_string();
        let lower = message_text.to_lowercase();

        if lower.contains("pubkey required") {
            ControllerError::transient(ErrorStage::Invite, err)
                .with_user_message("Please enter a participant pubkey before requesting an invite.")
                .with_recovery_action(RecoveryAction::None)
        } else if lower.contains("parse invite pubkey") {
            ControllerError::transient(ErrorStage::Invite, err)
                .with_user_message(
                    "Invite pubkey is invalid. Use the participant's hex or npub key.",
                )
                .with_recovery_action(RecoveryAction::None)
        } else if lower.contains("cannot invite self") {
            ControllerError::transient(ErrorStage::Invite, err)
                .with_user_message("You cannot invite your own key into the room.")
                .with_recovery_action(RecoveryAction::None)
        } else if lower.contains("member already present") {
            ControllerError::transient(ErrorStage::Invite, err)
                .with_user_message("That participant is already in the roster.")
                .with_recovery_action(RecoveryAction::None)
        } else if lower.contains("invite already pending") {
            ControllerError::transient(ErrorStage::Invite, err)
                .with_user_message("An invite for that participant is still pending approval.")
                .with_recovery_action(RecoveryAction::None)
        } else if lower.contains("relay") || lower.contains("nostr") {
            ControllerError::fatal(ErrorStage::Invite, err)
                .with_user_message("Failed to publish invite to relay. Check your connection.")
                .with_recovery_action(RecoveryAction::CheckConnection)
        } else {
            ControllerError::fatal(ErrorStage::Invite, err)
                .with_user_message("Invite failed. Verify the participant key and try again.")
                .with_recovery_action(RecoveryAction::Retry)
        }
    }

    fn emit_error(&self, err: ControllerError) {
        let (severity, stage, message, recovery_action, detail) = err.into_parts();
        match severity {
            ErrorSeverity::Transient => {
                if let Err(send_err) = self
                    .op_tx
                    .unbounded_send(Operation::Emit(ChatEvent::non_fatal_error(message.clone())))
                {
                    log::warn!(
                        "controller transient {stage} error: {detail:#}; failed to emit non-fatal error: {send_err}"
                    );
                } else {
                    log::warn!("controller transient {stage} error: {detail:#}");
                }
            }
            ErrorSeverity::Fatal => {
                let event = if let Some(action) = recovery_action {
                    ChatEvent::error_with_recovery(message.clone(), action)
                } else {
                    ChatEvent::error(message.clone())
                };
                let send_result = self.op_tx.unbounded_send(Operation::Emit(event));
                match send_result {
                    Ok(()) => {
                        log::error!("controller fatal {stage} error: {detail:#}");
                    }
                    Err(send_err) => {
                        log::error!(
                            "controller fatal {stage} error: {detail:#}; failed to emit error event: {send_err}"
                        );
                    }
                }
            }
        }
    }
}

struct ControllerHandshakeListener {
    op_tx: UnboundedSender<Operation>,
}

impl HandshakeListener for ControllerHandshakeListener {
    fn on_message(&self, message: HandshakeMessage) {
        let _ = self
            .op_tx
            .unbounded_send(Operation::IncomingHandshake(message));
    }
}

struct ControllerMoqListener {
    op_tx: UnboundedSender<Operation>,
}

impl MoqListener for ControllerMoqListener {
    fn on_frame(&self, bytes: Vec<u8>) {
        let _ = self.op_tx.unbounded_send(Operation::IncomingFrame(bytes));
    }

    fn on_ready(&self) {
        let _ = self.op_tx.unbounded_send(Operation::Ready);
    }

    fn on_error(&self, message: String) {
        let _ = self
            .op_tx
            .unbounded_send(Operation::Emit(ChatEvent::error(message)));
    }

    fn on_closed(&self) {
        let _ = self
            .op_tx
            .unbounded_send(Operation::Emit(ChatEvent::status("MoQ connection closed")));
    }
}
