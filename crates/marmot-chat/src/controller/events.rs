use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum SessionRole {
    Initial,
    Invitee,
}

impl SessionRole {
    pub fn peer(self) -> SessionRole {
        match self {
            SessionRole::Initial => SessionRole::Invitee,
            SessionRole::Invitee => SessionRole::Initial,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SessionRole::Initial => "initial",
            SessionRole::Invitee => "invitee",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Option<SessionRole> {
        match value {
            "initial" => Some(SessionRole::Initial),
            "invitee" => Some(SessionRole::Invitee),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionParams {
    pub bootstrap_role: SessionRole,
    pub relay_url: String,
    pub nostr_url: String,
    pub session_id: String,
    pub secret_hex: String,
    #[serde(default)]
    pub peer_pubkeys: Vec<String>,
    #[serde(default)]
    pub group_id_hex: Option<String>,
    #[serde(default)]
    pub admin_pubkeys: Vec<String>,
    #[serde(default)]
    pub local_transport_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberInfo {
    pub pubkey: String,
    #[serde(default)]
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Status {
        text: String,
    },
    Ready {
        ready: bool,
    },
    Message {
        author: String,
        content: String,
        created_at: u64,
        local: bool,
    },
    Commit {
        total: u32,
    },
    Roster {
        members: Vec<MemberInfo>,
    },
    MemberJoined {
        member: MemberInfo,
    },
    MemberUpdated {
        member: MemberInfo,
    },
    MemberLeft {
        pubkey: String,
    },
    InviteGenerated {
        welcome: String,
        recipient: String,
        is_admin: bool,
    },
    Error {
        message: String,
    },
    Handshake {
        phase: HandshakePhase,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandshakePhase {
    Initializing,
    WaitingForKeyPackage,
    WaitingForWelcome,
    Finalizing,
    Connected,
}

impl ChatEvent {
    pub fn status<T: Into<String>>(text: T) -> Self {
        ChatEvent::Status { text: text.into() }
    }

    pub fn error<T: Into<String>>(message: T) -> Self {
        ChatEvent::Error {
            message: message.into(),
        }
    }
}
