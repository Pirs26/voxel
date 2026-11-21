//! Human-pasteable join code encoding and decoding for `iroh::EndpointAddr`.

use data_encoding::BASE32_NOPAD;
use iroh::EndpointAddr;

const JOIN_CODE_PREFIX: &str = "ECHO-";

/// Encodes an [`iroh::EndpointAddr`] into a pasteable join code prefixed with `"ECHO-"`.
///
/// Uses `postcard` binary serialization followed by unpadded uppercase RFC 4648 Base32
/// so the resulting token is compact, URL-safe, and resilient to chat copy/paste.
pub fn encode_join_code(addr: &EndpointAddr) -> Result<String, String> {
    let bytes = postcard::to_allocvec(addr)
        .map_err(|e| format!("failed to serialize endpoint address: {e}"))?;
    let encoded = BASE32_NOPAD.encode(&bytes);
    Ok(format!("{JOIN_CODE_PREFIX}{encoded}"))
}

/// Decodes a pasteable `"ECHO-..."` join code back into an [`iroh::EndpointAddr`].
///
/// Ignores surrounding or embedded whitespace and accepts the `"ECHO-"` prefix
/// case-insensitively.
pub fn decode_join_code(code: &str) -> Result<EndpointAddr, String> {
    let cleaned: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err("join code is empty".to_string());
    }

    let upper = cleaned.to_ascii_uppercase();
    let payload = upper.strip_prefix(JOIN_CODE_PREFIX).ok_or_else(|| {
        format!("invalid join code: expected '{JOIN_CODE_PREFIX}' prefix")
    })?;

    if payload.is_empty() {
        return Err("invalid join code: missing payload after 'ECHO-' prefix".to_string());
    }

    let bytes = BASE32_NOPAD
        .decode(payload.as_bytes())
        .map_err(|e| format!("invalid join code Base32 encoding: {e}"))?;

    postcard::from_bytes(&bytes)
        .map_err(|e| format!("invalid join code address payload: {e}"))
}
