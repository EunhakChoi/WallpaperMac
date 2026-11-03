//! Native observed value envelope. Namespace/key-to-file policy belongs to the host.
use crate::{Error, Result};
use serde_json::Value;
const PREFIX: &[u8; 8] = b"LSKV0001";
const LIMIT: usize = 16 * 1024 * 1024;
pub fn encode_storage_value(value: &Value) -> Result<Vec<u8>> {
    let payload =
        serde_json::to_vec(value).map_err(|e| Error::new("storage encode", e.to_string()))?;
    if payload.len() > LIMIT {
        return Err(Error::new("storage encode", "value size limit exceeded"));
    }
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(PREFIX);
    out.extend_from_slice(&payload);
    Ok(out)
}
pub fn decode_storage_value(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > LIMIT + 8 {
        return Err(Error::new("storage decode", "value size limit exceeded"));
    }
    if !bytes.starts_with(PREFIX) {
        return Err(Error::new("storage decode", "LSKV0001 prefix required"));
    }
    serde_json::from_slice(&bytes[8..]).map_err(|e| Error::new("storage decode", e.to_string()))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_envelope_utf8_roundtrip_and_bad_prefix() {
        let value = json!({"한글":[1,true,null,"x"]});
        let bytes = encode_storage_value(&value).unwrap();
        assert_eq!(&bytes[..8], b"LSKV0001");
        assert_eq!(decode_storage_value(&bytes).unwrap(), value);
        assert!(decode_storage_value(b"LSKV0002{}").is_err());
        assert!(decode_storage_value(b"LSKV0001[").is_err());
    }
}
