//! Tool call transcript — typed records of every tool call made during execution.

#[cfg(not(feature = "std"))]
use alloc::{borrow::ToOwned, format, string::String, vec::Vec};
use sha2::{Digest, Sha256};

/// Status of a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ToolCallStatus {
    Ok,
    Error,
}

/// A single recorded tool call.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ToolCallRecord {
    /// 0-indexed sequence number.
    pub seq: usize,
    /// The tool name string.
    pub tool_name: String,
    /// Canonical JSON bytes of the serialized arguments. Serializes as a
    /// string holding the JSON text.
    #[cfg_attr(feature = "serde", serde(with = "json_text"))]
    pub args_canonical: Vec<u8>,
    /// Byte length of `args_canonical`.
    pub args_bytes: usize,
    /// SHA-256 hex string of the canonical response bytes (empty string on error).
    pub response_hash: String,
    /// Byte length of the canonical response bytes (0 on error).
    pub response_bytes: usize,
    /// Canonical JSON bytes of the response table (empty on error).
    /// Used to construct an `OracleTape` for zkVM replay. Serializes as a
    /// string holding the JSON text.
    #[cfg_attr(feature = "serde", serde(with = "json_text"))]
    pub response_canonical: Vec<u8>,
    /// Error message returned by the host (empty string on success).
    /// Used to replay `Err(msg)` responses from an `OracleTape`.
    pub error_message: String,
    /// Provenance attestation blob the host sourced for this response (empty
    /// when none). Bind-only: committed alongside the response bytes, not
    /// verified here. Always empty for failed calls. Serializes as lowercase
    /// hex.
    #[cfg_attr(feature = "serde", serde(default, with = "lower_hex"))]
    pub attestation: Vec<u8>,
    /// Gas charged for this tool call (0 for failed calls).
    pub gas_charged: u64,
    /// Status of the call.
    pub status: ToolCallStatus,
}

/// Serde form of canonical JSON bytes: a string holding the JSON text.
///
/// Canonical JSON is ASCII (non-ASCII is escaped), so the text is exact. Bytes
/// that are not UTF-8 are an error rather than a lossy conversion.
#[cfg(feature = "serde")]
mod json_text {
    #[cfg(not(feature = "std"))]
    use alloc::vec::Vec;
    use core::fmt;
    use serde::{Deserializer, Serializer, de, ser};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        let text = core::str::from_utf8(bytes)
            .map_err(|_| ser::Error::custom("canonical JSON bytes are not valid UTF-8"))?;
        serializer.serialize_str(text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        struct Text;
        impl de::Visitor<'_> for Text {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string holding canonical JSON text")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Vec<u8>, E> {
                Ok(v.as_bytes().to_vec())
            }
        }
        deserializer.deserialize_str(Text)
    }
}

/// Serde form of opaque bytes: a lowercase hex string. Deserializing rejects
/// anything else, including uppercase digits and odd lengths.
#[cfg(feature = "serde")]
mod lower_hex {
    #[cfg(not(feature = "std"))]
    use alloc::{string::String, vec::Vec};
    use core::fmt;
    use serde::{Deserializer, Serializer, de};

    const DIGITS: &[u8; 16] = b"0123456789abcdef";

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        let mut text = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            text.push(DIGITS[(b >> 4) as usize] as char);
            text.push(DIGITS[(b & 0x0f) as usize] as char);
        }
        serializer.serialize_str(&text)
    }

    fn nibble(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        struct Hex;
        impl de::Visitor<'_> for Hex {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a lowercase hex string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Vec<u8>, E> {
                let v = v.as_bytes();
                if !v.len().is_multiple_of(2) {
                    return Err(E::custom("hex string has odd length"));
                }
                v.chunks(2)
                    .map(|pair| match (nibble(pair[0]), nibble(pair[1])) {
                        (Some(hi), Some(lo)) => Ok((hi << 4) | lo),
                        _ => Err(E::custom("invalid lowercase hex digit")),
                    })
                    .collect()
            }
        }
        deserializer.deserialize_str(Hex)
    }
}

/// Accumulates tool call records for the current execution.
#[derive(Debug, Default)]
pub struct Transcript {
    records: Vec<ToolCallRecord>,
}

impl Transcript {
    pub fn new() -> Self {
        Transcript {
            records: Vec::new(),
        }
    }

    /// Record a successful tool call with no provenance attestation.
    pub fn record_ok(
        &mut self,
        tool_name: &str,
        args_canonical: Vec<u8>,
        response_canonical: Vec<u8>,
        gas_charged: u64,
    ) {
        self.record_ok_attested(
            tool_name,
            args_canonical,
            response_canonical,
            Vec::new(),
            gas_charged,
        );
    }

    /// Record a successful tool call along with the provenance attestation the
    /// host sourced for it (empty `attestation` is equivalent to `record_ok`).
    pub fn record_ok_attested(
        &mut self,
        tool_name: &str,
        args_canonical: Vec<u8>,
        response_canonical: Vec<u8>,
        attestation: Vec<u8>,
        gas_charged: u64,
    ) {
        let seq = self.records.len();
        let args_bytes = args_canonical.len();
        let response_bytes = response_canonical.len();
        let response_hash = sha256_hex(&response_canonical);

        self.records.push(ToolCallRecord {
            seq,
            tool_name: tool_name.to_owned(),
            args_canonical,
            args_bytes,
            response_hash,
            response_bytes,
            response_canonical,
            error_message: String::new(),
            attestation,
            gas_charged,
            status: ToolCallStatus::Ok,
        });
    }

    /// Record a failed tool call.
    pub fn record_error(
        &mut self,
        tool_name: &str,
        args_canonical: Vec<u8>,
        gas_charged: u64,
        error_message: &str,
    ) {
        let seq = self.records.len();
        let args_bytes = args_canonical.len();

        self.records.push(ToolCallRecord {
            seq,
            tool_name: tool_name.to_owned(),
            args_canonical,
            args_bytes,
            response_hash: String::new(),
            response_bytes: 0,
            response_canonical: Vec::new(),
            error_message: error_message.to_owned(),
            attestation: Vec::new(),
            gas_charged,
            status: ToolCallStatus::Error,
        });
    }

    pub fn records(&self) -> &[ToolCallRecord] {
        &self.records
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let hash_bytes = Sha256::digest(data);
    hash_bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_transcript() {
        let t = Transcript::new();
        assert_eq!(t.len(), 0);
        assert!(t.is_empty());
        assert_eq!(t.records().len(), 0);
    }

    #[test]
    fn record_ok_basic() {
        let mut t = Transcript::new();
        t.record_ok(
            "search",
            b"{\"query\":\"x\"}".to_vec(),
            b"{\"result\":1}".to_vec(),
            200,
        );
        assert_eq!(t.len(), 1);
        let r = &t.records()[0];
        assert_eq!(r.seq, 0);
        assert_eq!(r.tool_name, "search");
        assert_eq!(r.args_canonical, b"{\"query\":\"x\"}");
        assert_eq!(r.args_bytes, 13);
        assert_eq!(r.response_bytes, 12);
        assert_eq!(r.gas_charged, 200);
        assert_eq!(r.status, ToolCallStatus::Ok);
        // response_hash should be a 64-char hex string
        assert_eq!(r.response_hash.len(), 64);
        assert!(r.response_hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn record_ok_correct_sha256() {
        let mut t = Transcript::new();
        let response = b"hello";
        t.record_ok("tool", vec![], response.to_vec(), 0);

        // Known SHA-256 of "hello"
        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(t.records()[0].response_hash, expected);
    }

    #[test]
    fn seq_increments() {
        let mut t = Transcript::new();
        t.record_ok("a", vec![], vec![], 0);
        t.record_ok("b", vec![], vec![], 0);
        t.record_error("c", vec![], 0, "err c");
        assert_eq!(t.records()[0].seq, 0);
        assert_eq!(t.records()[1].seq, 1);
        assert_eq!(t.records()[2].seq, 2);
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn record_error_status() {
        let mut t = Transcript::new();
        t.record_error("fail_tool", b"{}".to_vec(), 0, "something failed");
        let r = &t.records()[0];
        assert_eq!(r.status, ToolCallStatus::Error);
        assert_eq!(r.tool_name, "fail_tool");
        assert_eq!(r.response_hash, "");
        assert_eq!(r.response_bytes, 0);
        assert_eq!(r.gas_charged, 0);
    }

    #[test]
    fn record_ok_has_empty_attestation_by_default() {
        let mut t = Transcript::new();
        t.record_ok("tool", vec![], b"resp".to_vec(), 0);
        assert!(t.records()[0].attestation.is_empty());
    }

    #[test]
    fn record_ok_attested_stores_blob() {
        let mut t = Transcript::new();
        t.record_ok_attested("tool", vec![], b"resp".to_vec(), b"sig".to_vec(), 0);
        assert_eq!(t.records()[0].attestation, b"sig");
        // Attestation does not perturb the response hash.
        let mut t2 = Transcript::new();
        t2.record_ok("tool", vec![], b"resp".to_vec(), 0);
        assert_eq!(t.records()[0].response_hash, t2.records()[0].response_hash);
    }

    #[test]
    fn record_error_has_empty_attestation() {
        let mut t = Transcript::new();
        t.record_error("broken", vec![], 0, "boom");
        assert!(t.records()[0].attestation.is_empty());
    }

    #[test]
    fn record_error_after_ok() {
        let mut t = Transcript::new();
        t.record_ok("first", vec![], b"resp".to_vec(), 100);
        t.record_error("second", vec![], 0, "second failed");
        assert_eq!(t.len(), 2);
        assert_eq!(t.records()[0].status, ToolCallStatus::Ok);
        assert_eq!(t.records()[1].status, ToolCallStatus::Error);
        assert_eq!(t.records()[1].seq, 1);
    }

    // ── Serde form ────────────────────────────────────────────────────────────

    #[cfg(all(feature = "serde", feature = "std"))]
    fn attested_record() -> ToolCallRecord {
        let mut t = Transcript::new();
        t.record_ok_attested(
            "transfer",
            b"{\"amount\":20}".to_vec(),
            b"{\"ok\":true}".to_vec(),
            vec![0x00, 0xab, 0xff],
            113,
        );
        t.records()[0].clone()
    }

    #[cfg(all(feature = "serde", feature = "std"))]
    const ATTESTED_RECORD_JSON: &str = r#"{"seq":0,"tool_name":"transfer","args_canonical":"{\"amount\":20}","args_bytes":13,"response_hash":"4062edaf750fb8074e7e83e0c9028c94e32468a8b6f1614774328ef045150f93","response_bytes":11,"response_canonical":"{\"ok\":true}","error_message":"","attestation":"00abff","gas_charged":113,"status":"Ok"}"#;

    #[cfg(all(feature = "serde", feature = "std"))]
    fn assert_same_record(a: &ToolCallRecord, b: &ToolCallRecord) {
        assert_eq!(a.seq, b.seq);
        assert_eq!(a.tool_name, b.tool_name);
        assert_eq!(a.args_canonical, b.args_canonical);
        assert_eq!(a.args_bytes, b.args_bytes);
        assert_eq!(a.response_hash, b.response_hash);
        assert_eq!(a.response_bytes, b.response_bytes);
        assert_eq!(a.response_canonical, b.response_canonical);
        assert_eq!(a.error_message, b.error_message);
        assert_eq!(a.attestation, b.attestation);
        assert_eq!(a.gas_charged, b.gas_charged);
        assert_eq!(a.status, b.status);
    }

    #[cfg(all(feature = "serde", feature = "std"))]
    #[test]
    fn record_serializes_bytes_as_json_text_and_hex() {
        let record = attested_record();
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(json, ATTESTED_RECORD_JSON);

        let back: ToolCallRecord = serde_json::from_str(&json).unwrap();
        assert_same_record(&back, &record);
    }

    #[cfg(all(feature = "serde", feature = "std"))]
    #[test]
    fn record_with_empty_attestation_round_trips() {
        let mut t = Transcript::new();
        t.record_error("fail", b"{}".to_vec(), 0, "denied");
        let record = t.records()[0].clone();
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains(r#""response_canonical":"""#), "{json}");
        assert!(json.contains(r#""attestation":"""#), "{json}");
        let back: ToolCallRecord = serde_json::from_str(&json).unwrap();
        assert_same_record(&back, &record);

        // A record without the field at all still loads, with no attestation.
        let without = json.replace(r#","attestation":"""#, "");
        let back: ToolCallRecord = serde_json::from_str(&without).unwrap();
        assert!(back.attestation.is_empty());
    }

    #[cfg(all(feature = "serde", feature = "std"))]
    #[test]
    fn record_with_non_utf8_args_fails_to_serialize() {
        let mut record = attested_record();
        record.args_canonical = vec![b'"', 0xff, b'"'];
        let err = serde_json::to_string(&record).unwrap_err();
        assert!(err.to_string().contains("not valid UTF-8"), "{err}");
    }

    #[cfg(all(feature = "serde", feature = "std"))]
    #[test]
    fn record_with_bad_hex_attestation_fails_to_deserialize() {
        // Sanity: the untouched fixture deserializes.
        serde_json::from_str::<ToolCallRecord>(ATTESTED_RECORD_JSON).unwrap();
        // The old number-array form is rejected too.
        for bad in [
            r#""0g""#,
            r#""abc""#,
            r#""00ABFF""#,
            r#""0x00""#,
            "[0,171,255]",
        ] {
            let json = ATTESTED_RECORD_JSON.replace(r#""00abff""#, bad);
            assert!(
                serde_json::from_str::<ToolCallRecord>(&json).is_err(),
                "accepted attestation {bad}"
            );
        }
    }
}
