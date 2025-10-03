use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct WrapperFrame {
    pub bytes: Vec<u8>,
    pub kind: WrapperKind,
}

#[derive(Clone, Debug)]
pub enum WrapperKind {
    Application { author: String, content: String },
    Directory(DirectoryMessage),
    Commit,
}

impl WrapperKind {
    pub fn label(&self) -> &'static str {
        match self {
            WrapperKind::Application { .. } => "application",
            WrapperKind::Directory(_) => "directory",
            WrapperKind::Commit => "commit",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            WrapperKind::Application { author, content } => {
                format!("{author}: {content}")
            }
            WrapperKind::Directory(dir) => {
                format!("directory: {} tracks from {}", dir.tracks.len(), dir.sender)
            }
            WrapperKind::Commit => "commit".to_string(),
        }
    }
}

/// Directory message: MLS application message listing current media tracks
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryMessage {
    /// Sender's public key (hex-encoded Nostr npub)
    pub sender: String,
    /// Current epoch number when this directory was emitted
    pub epoch: u64,
    /// List of active media tracks
    pub tracks: Vec<TrackEntry>,
}

/// Individual media track entry in the directory
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackEntry {
    /// Random-looking track label derived from MLS exporter
    /// Format: hex(MLS-Exporter("moq-track-lbl-v1", sender_leaf || kind || epoch, 16))
    pub label: String,
    /// Track kind (audio, video, screen)
    pub kind: TrackKind,
    /// Codec configuration
    pub codec: CodecInfo,
    /// Optional simulcast layers
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub simulcast: Vec<SimulcastLayer>,
}

/// Media track kind
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Audio,
    Video,
    Screen,
}

/// Codec configuration (minimal RTP-ish/hang-ish fields)
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub struct CodecInfo {
    /// Codec name (e.g., "opus", "vp8", "vp9", "h264", "av1")
    pub name: String,
    /// Clock rate in Hz (e.g., 48000 for Opus, 90000 for video)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_rate: Option<u32>,
    /// Number of channels (audio only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
    /// Codec-specific parameters (fmtp-style)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<(String, String)>,
}

/// Simulcast layer information
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SimulcastLayer {
    /// Layer ID
    pub id: String,
    /// Target bitrate in kbps
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u32>,
    /// Resolution (width x height)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<(u32, u32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_directory_message_serialization() {
        let directory = DirectoryMessage {
            sender: "abc123".to_string(),
            epoch: 5,
            tracks: vec![
                TrackEntry {
                    label: "track001".to_string(),
                    kind: TrackKind::Audio,
                    codec: CodecInfo {
                        name: "opus".to_string(),
                        clock_rate: Some(48000),
                        channels: Some(2),
                        params: vec![],
                    },
                    simulcast: vec![],
                },
                TrackEntry {
                    label: "track002".to_string(),
                    kind: TrackKind::Video,
                    codec: CodecInfo {
                        name: "vp8".to_string(),
                        clock_rate: Some(90000),
                        channels: None,
                        params: vec![],
                    },
                    simulcast: vec![],
                },
            ],
        };

        // Test JSON serialization
        let json = serde_json::to_string(&directory).expect("serialize to JSON");
        assert!(json.contains("\"sender\":\"abc123\""));
        assert!(json.contains("\"epoch\":5"));
        assert!(json.contains("\"kind\":\"audio\""));
        assert!(json.contains("\"kind\":\"video\""));

        // Test round-trip
        let deserialized: DirectoryMessage =
            serde_json::from_str(&json).expect("deserialize from JSON");
        assert_eq!(deserialized, directory);
    }

    #[test]
    fn test_track_kind_serialization() {
        assert_eq!(
            serde_json::to_string(&TrackKind::Audio).unwrap(),
            "\"audio\""
        );
        assert_eq!(
            serde_json::to_string(&TrackKind::Video).unwrap(),
            "\"video\""
        );
        assert_eq!(
            serde_json::to_string(&TrackKind::Screen).unwrap(),
            "\"screen\""
        );
    }

    #[test]
    fn test_codec_info_minimal() {
        let codec = CodecInfo {
            name: "opus".to_string(),
            clock_rate: None,
            channels: None,
            params: vec![],
        };
        let json = serde_json::to_string(&codec).unwrap();
        // Optional fields should be omitted
        assert!(!json.contains("clock_rate"));
        assert!(!json.contains("channels"));
        assert!(!json.contains("params"));

        let deserialized: CodecInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, codec);
    }

    #[test]
    fn test_simulcast_layer() {
        let layer = SimulcastLayer {
            id: "high".to_string(),
            bitrate: Some(2000),
            resolution: Some((1920, 1080)),
        };
        let json = serde_json::to_string(&layer).unwrap();
        assert!(json.contains("\"id\":\"high\""));
        assert!(json.contains("\"bitrate\":2000"));
        assert!(json.contains("[1920,1080]"));

        let deserialized: SimulcastLayer = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, layer);
    }

    #[test]
    fn test_empty_directory() {
        let directory = DirectoryMessage {
            sender: "test".to_string(),
            epoch: 0,
            tracks: vec![],
        };
        let json = serde_json::to_string(&directory).unwrap();
        let deserialized: DirectoryMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, directory);
    }
}
