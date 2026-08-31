use buzz_sim_agent::{ConversationSurface, NpcActionDraft, NpcReplyDraft, ValidatedNpcTurn};
use buzz_sim_protocol::{canonical_json_bytes, sha256_hex};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::ExecutionError;

pub(crate) fn expected_action_id(
    turn: &ValidatedNpcTurn,
    index: usize,
    action: &NpcActionDraft,
) -> Result<String, ExecutionError> {
    digest(&json!({
        "session_id": turn.session_id,
        "turn_id": turn.turn_id,
        "actor_id": turn.actor_id,
        "index": index,
        "action": action,
    }))
}

pub(crate) fn reply_operation_id(
    turn: &ValidatedNpcTurn,
    surface: &ConversationSurface,
    reply: &NpcReplyDraft,
) -> Result<String, ExecutionError> {
    digest(&json!({
        "kind": "npc_reply",
        "session_id": turn.session_id,
        "turn_id": turn.turn_id,
        "actor_id": turn.actor_id,
        "surface": surface,
        "reply": reply,
    }))
}

pub(crate) fn command_fingerprint<T: Serialize>(value: &T) -> Result<String, ExecutionError> {
    let value =
        serde_json::to_value(value).map_err(|error| ExecutionError::Digest(error.to_string()))?;
    digest(&value)
}

pub(crate) fn verification_run_id(operation_id: &str) -> Result<Uuid, ExecutionError> {
    if operation_id.len() != 64 || !operation_id.chars().all(|value| value.is_ascii_hexdigit()) {
        return Err(ExecutionError::Digest(
            "operation id must be a 64-character SHA-256 value".to_string(),
        ));
    }
    let mut bytes = [0_u8; 16];
    for (index, chunk) in operation_id.as_bytes()[..32].chunks_exact(2).enumerate() {
        bytes[index] = (decode_nibble(chunk[0])? << 4) | decode_nibble(chunk[1])?;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(Uuid::from_bytes(bytes))
}

fn decode_nibble(value: u8) -> Result<u8, ExecutionError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(ExecutionError::Digest(
            "operation id contains non-hexadecimal data".to_string(),
        )),
    }
}

fn digest(value: &serde_json::Value) -> Result<String, ExecutionError> {
    let bytes =
        canonical_json_bytes(value).map_err(|error| ExecutionError::Digest(error.to_string()))?;
    Ok(sha256_hex(&bytes))
}
