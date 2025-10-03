use anyhow::Error;
use std::fmt;

use crate::controller::events::RecoveryAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorSeverity {
    Transient,
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorStage {
    Handshake,
    Messaging,
    Invite,
}

impl fmt::Display for ErrorStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorStage::Handshake => write!(f, "handshake"),
            ErrorStage::Messaging => write!(f, "messaging"),
            ErrorStage::Invite => write!(f, "invite"),
        }
    }
}

#[derive(Debug)]
pub struct ControllerError {
    pub stage: ErrorStage,
    pub severity: ErrorSeverity,
    detail: Error,
    user_message: Option<String>,
    recovery_action: Option<RecoveryAction>,
}

impl ControllerError {
    pub fn fatal(stage: ErrorStage, detail: Error) -> Self {
        Self {
            stage,
            severity: ErrorSeverity::Fatal,
            detail,
            user_message: None,
            recovery_action: Some(default_recovery_action(stage)),
        }
    }

    pub fn transient(stage: ErrorStage, detail: Error) -> Self {
        Self {
            stage,
            severity: ErrorSeverity::Transient,
            detail,
            user_message: None,
            recovery_action: Some(RecoveryAction::None),
        }
    }

    pub fn with_user_message(mut self, message: impl Into<String>) -> Self {
        self.user_message = Some(message.into());
        self
    }

    pub fn with_recovery_action(mut self, action: RecoveryAction) -> Self {
        self.recovery_action = Some(action);
        self
    }

    pub fn into_parts(
        self,
    ) -> (
        ErrorSeverity,
        ErrorStage,
        String,
        Option<RecoveryAction>,
        Error,
    ) {
        let severity = self.severity;
        let stage = self.stage;
        let message = self
            .user_message
            .unwrap_or_else(|| default_user_message(stage).to_owned());
        (severity, stage, message, self.recovery_action, self.detail)
    }
}

fn default_user_message(stage: ErrorStage) -> &'static str {
    match stage {
        ErrorStage::Handshake => "Handshake failed. Refresh the page or request a new invite.",
        ErrorStage::Messaging => {
            "Failed to process encrypted message. Refresh or request a new invite."
        }
        ErrorStage::Invite => "Invite request failed. Verify the participant key and try again.",
    }
}

fn default_recovery_action(stage: ErrorStage) -> RecoveryAction {
    match stage {
        ErrorStage::Handshake => RecoveryAction::Refresh,
        ErrorStage::Messaging => RecoveryAction::Refresh,
        ErrorStage::Invite => RecoveryAction::Retry,
    }
}
