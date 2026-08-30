use std::io::Write;

use serde::Serialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::{ArtifactRef, CheckResult, RepositoryRevision, VerificationRequest, VerificationResult};

/// Errors raised while constructing deterministic protocol fingerprints.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolDigestError {
    /// A protocol value could not be serialized.
    #[error("failed to serialize digest material: {0}")]
    Serialize(#[from] serde_json::Error),
    /// A canonical JSON string could not be written.
    #[error("failed to write canonical JSON: {0}")]
    Write(#[from] std::io::Error),
}

/// Serializes JSON with lexicographically sorted object keys.
pub fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, ProtocolDigestError> {
    let mut output = Vec::new();
    write_value(value, &mut output)?;
    Ok(output)
}

fn write_value(value: &Value, output: &mut Vec<u8>) -> Result<(), ProtocolDigestError> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(value) => output.extend_from_slice(if *value { b"true" } else { b"false" }),
        Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        Value::String(value) => serde_json::to_writer(output, value)?,
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_value(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(values) => write_object(values, output)?,
    }
    Ok(())
}

fn write_object(values: &Map<String, Value>, output: &mut Vec<u8>) -> Result<(), ProtocolDigestError> {
    output.push(b'{');
    let mut keys = values.keys().collect::<Vec<_>>();
    keys.sort_unstable();
    for (index, key) in keys.into_iter().enumerate() {
        if index > 0 {
            output.push(b',');
        }
        serde_json::to_writer(&mut *output, key)?;
        output.push(b':');
        if let Some(value) = values.get(key) {
            write_value(value, output)?;
        }
    }
    output.push(b'}');
    output.flush()?;
    Ok(())
}

/// Returns the lowercase SHA-256 digest of bytes.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn digest_serializable<T: Serialize>(value: &T) -> Result<String, ProtocolDigestError> {
    let value = serde_json::to_value(value)?;
    Ok(sha256_hex(&canonical_json_bytes(&value)?))
}

/// Computes the semantic request fingerprint, excluding the idempotency run ID.
pub fn request_digest(request: &VerificationRequest) -> Result<String, ProtocolDigestError> {
    let mut repositories = request.repositories.clone();
    repositories.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    digest_serializable(&json!({
        "version": request.version,
        "session_id": request.session_id,
        "scenario_id": request.scenario_id,
        "scenario_version": request.scenario_version,
        "expected_manifest_digest": request.expected_manifest_digest,
        "repositories": repositories,
    }))
}

/// Computes an exact repository set fingerprint independent of input ordering.
pub fn commit_set_digest(repositories: &[RepositoryRevision]) -> Result<String, ProtocolDigestError> {
    let mut material = repositories
        .iter()
        .map(|repository| {
            json!({
                "repository_id": repository.repository_id,
                "base_commit_sha": repository.base_commit_sha,
                "head_commit_sha": repository.head_commit_sha,
            })
        })
        .collect::<Vec<_>>();
    material.sort_by(|left, right| {
        left["repository_id"]
            .as_str()
            .cmp(&right["repository_id"].as_str())
    });
    digest_serializable(&material)
}

/// Computes the SHA-256 fingerprint of exact trusted manifest bytes.
#[must_use]
pub fn manifest_digest(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

/// Computes a stable immutable execution-environment fingerprint.
pub fn environment_digest(
    manifest_digest: &str,
    image_digest: &str,
    backend: &str,
) -> Result<String, ProtocolDigestError> {
    digest_serializable(&json!({
        "manifest_digest": manifest_digest,
        "image_digest": image_digest,
        "backend": backend,
    }))
}

/// Computes a deterministic semantic result fingerprint.
pub fn normalized_result_digest(result: &VerificationResult) -> Result<String, ProtocolDigestError> {
    let mut repositories = result.repositories.clone();
    repositories.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    for repository in &mut repositories {
        repository.changed_paths.sort();
    }

    let mut checks = result.checks.clone();
    normalize_checks(&mut checks);

    let mut artifacts = result.artifacts.clone();
    normalize_artifacts(&mut artifacts);

    digest_serializable(&json!({
        "version": result.version,
        "scenario_id": result.scenario_id,
        "scenario_version": result.scenario_version,
        "status": result.status,
        "request_digest": result.request_digest,
        "commit_set_digest": result.commit_set_digest,
        "repositories": repositories,
        "environment": result.environment,
        "checks": checks.into_iter().map(normalized_check_value).collect::<Vec<_>>(),
        "artifacts": artifacts.into_iter().map(normalized_artifact_value).collect::<Vec<_>>(),
        "failure_code": result.failure.as_ref().map(|failure| &failure.code),
    }))
}

fn normalize_checks(checks: &mut [CheckResult]) {
    for check in checks.iter_mut() {
        check.assertions.sort_by(|left, right| left.key.cmp(&right.key));
    }
    checks.sort_by(|left, right| left.id.cmp(&right.id));
}

fn normalize_artifacts(artifacts: &mut [ArtifactRef]) {
    artifacts.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.sha256.cmp(&right.sha256))
    });
}

fn normalized_check_value(check: CheckResult) -> Value {
    json!({
        "id": check.id,
        "phase": check.phase,
        "status": check.status,
        "visibility": check.visibility,
        "assertions": check.assertions,
        "stdout": check.stdout_artifact.map(normalized_artifact_value),
        "stderr": check.stderr_artifact.map(normalized_artifact_value),
    })
}

fn normalized_artifact_value(artifact: ArtifactRef) -> Value {
    json!({
        "name": artifact.name,
        "sha256": artifact.sha256,
        "byte_len": artifact.byte_len,
        "visibility": artifact.visibility,
    })
}
