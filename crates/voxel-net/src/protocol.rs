//! Wire protocol types, serialization helpers, and length-prefixed framing
//! for reliable QUIC streams and unreliable QUIC datagrams.

use glam::{IVec3, Vec3};
use iroh::endpoint::{ReadExactError, RecvStream, SendStream};
use serde::{Deserialize, Serialize};
use voxel_core::VoxelId;

/// Player identifier within a co-op session.
///
/// The host is always `0`; guests are assigned sequential IDs starting at `1`.
pub type PlayerId = u16;

/// Maximum permitted byte length for a single length-prefixed reliable frame (8 MiB).
pub const MAX_RELIABLE_FRAME_LEN: usize = 8 * 1024 * 1024;

/// Static profile metadata for a player in the session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: PlayerId,
    pub name: String,
    pub visor_rgb: [f32; 3],
}

/// Ordered, reliable messages sent over each peer's bidirectional QUIC control stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ReliableMessage {
    Handshake {
        player_name: String,
        visor_rgb: [f32; 3],
    },
    Welcome {
        assigned_id: PlayerId,
        level_seed: u64,
        players: Vec<PlayerInfo>,
        voxel_edits: Vec<(IVec3, VoxelId)>,
        beacon_pos_m: Vec3,
    },
    PlayerJoined(PlayerInfo),
    PlayerLeft {
        player_id: PlayerId,
    },
    StartDescent {
        level_seed: u64,
        depth_tier: u8,
    },
    VoxelDelta {
        coord: IVec3,
        material: VoxelId,
    },
    FlareThrown {
        origin_m: Vec3,
        velocity_m: Vec3,
    },
    BeaconMoved {
        pos_m: Vec3,
    },
}

/// Low-latency, unreliable, unordered messages sent as QUIC datagrams.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum UnreliableDatagram {
    PlayerPose {
        player_id: PlayerId,
        tick: u32,
        feet_pos_m: Vec3,
        yaw: f32,
        pitch: f32,
        crouching: bool,
        flashlight_on: bool,
    },
    VoiceFrame {
        speaker_id: PlayerId,
        seq: u16,
        opus_payload: Vec<u8>,
    },
}

/// Serializes a [`ReliableMessage`] with `postcard`.
pub fn encode_reliable(msg: &ReliableMessage) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(msg)
}

/// Deserializes a [`ReliableMessage`] from `postcard` bytes.
pub fn decode_reliable(bytes: &[u8]) -> Result<ReliableMessage, postcard::Error> {
    postcard::from_bytes(bytes)
}

/// Serializes an [`UnreliableDatagram`] with `postcard`.
pub fn encode_datagram(dgram: &UnreliableDatagram) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(dgram)
}

/// Deserializes an [`UnreliableDatagram`] from `postcard` bytes.
pub fn decode_datagram(bytes: &[u8]) -> Result<UnreliableDatagram, postcard::Error> {
    postcard::from_bytes(bytes)
}

/// Encodes a [`ReliableMessage`] into a length-prefixed (`u32` little-endian) frame.
pub fn frame_reliable(msg: &ReliableMessage) -> Result<Vec<u8>, String> {
    let payload = encode_reliable(msg).map_err(|e| format!("encode reliable message failed: {e}"))?;
    if payload.len() > MAX_RELIABLE_FRAME_LEN {
        return Err(format!(
            "reliable frame too large: {} bytes (max {})",
            payload.len(),
            MAX_RELIABLE_FRAME_LEN
        ));
    }
    let len = u32::try_from(payload.len())
        .map_err(|_| "reliable frame length overflows u32".to_string())?;
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&len.to_le_bytes());
    framed.extend_from_slice(&payload);
    Ok(framed)
}

/// Writes a single length-prefixed (`u32` little-endian) [`ReliableMessage`] to a QUIC [`SendStream`].
pub async fn write_reliable_frame(
    send: &mut SendStream,
    msg: &ReliableMessage,
) -> Result<(), String> {
    let framed = frame_reliable(msg)?;
    send.write_all(&framed)
        .await
        .map_err(|e| format!("stream write failed: {e}"))
}

/// Reads a single length-prefixed (`u32` little-endian) [`ReliableMessage`] from a QUIC [`RecvStream`].
///
/// Returns `Ok(None)` when the peer cleanly finishes the stream at a frame boundary.
pub async fn read_reliable_frame(recv: &mut RecvStream) -> Result<Option<ReliableMessage>, String> {
    let mut len_buf = [0u8; 4];
    match recv.read_exact(&mut len_buf).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(ReadExactError::FinishedEarly(n)) => {
            return Err(format!(
                "stream ended mid-header after {n} of 4 bytes"
            ));
        }
        Err(ReadExactError::ReadError(e)) => {
            return Err(format!("stream header read failed: {e}"));
        }
    }

    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_RELIABLE_FRAME_LEN {
        return Err(format!(
            "incoming reliable frame too large: {len} bytes (max {MAX_RELIABLE_FRAME_LEN})"
        ));
    }

    let mut payload = vec![0u8; len];
    if len > 0 {
        recv.read_exact(&mut payload)
            .await
            .map_err(|e| format!("stream payload read failed: {e}"))?;
    }

    let msg =
        decode_reliable(&payload).map_err(|e| format!("decode reliable message failed: {e}"))?;
    Ok(Some(msg))
}
