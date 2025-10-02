pub mod events;
pub mod local_transport;
pub mod services;
mod state;

pub use state::ControllerConfig;

use std::cell::RefCell;
use std::rc::Rc;

use events::{ChatEvent, SessionParams};
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use state::{ControllerState, Operation};

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
                    self.emit_error(err);
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
                    self.emit_error(err);
                }
            }
            Operation::ConnectMoq => {
                let listener: Box<dyn MoqListener> = Box::new(ControllerMoqListener {
                    op_tx: self.op_tx.clone(),
                });
                let state_ref = self.state.borrow();
                state_ref.moq.connect(
                    &state_ref.session.relay_url,
                    &state_ref.session.session_id,
                    state_ref.session.bootstrap_role,
                    state_ref.session.bootstrap_role.peer(),
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
                    Err(err) => self.emit_error(err),
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
                    Err(err) => self.emit_error(err),
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
                    Err(err) => self.emit_error(err),
                }
            }
            Operation::InviteMember { pubkey, is_admin } => {
                if let Err(err) =
                    self.state
                        .borrow_mut()
                        .request_invite(&self.op_tx, pubkey, is_admin)
                {
                    self.emit_error(err);
                }
            }
            Operation::Shutdown => {
                self.state.borrow().moq.shutdown();
                self.state.borrow().nostr.shutdown();
            }
        }
    }

    fn emit_error(&self, err: anyhow::Error) {
        let message = format!("{err:#}");
        if let Ok(_) = self
            .op_tx
            .unbounded_send(Operation::Emit(ChatEvent::error(message.clone())))
        {
            log::error!("controller error: {message}");
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
