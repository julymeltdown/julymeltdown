#![deny(unsafe_code)]
#![warn(missing_docs)]
//! Transport-neutral contracts for deterministic source-code verification.

mod digest;
mod verification;

pub use digest::{
    canonical_json_bytes, commit_set_digest, environment_digest, manifest_digest,
    normalized_result_digest, request_digest, sha256_hex, ProtocolDigestError,
};
pub use verification::*;
