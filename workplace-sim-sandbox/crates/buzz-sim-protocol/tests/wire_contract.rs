use buzz_sim_protocol::{
    canonical_json_bytes, commit_set_digest, normalized_result_digest, request_digest, ArtifactRef,
    AssertionResult, CheckPhase, CheckResult, EnvironmentEvidence, EvidenceVisibility,
    FailureSummary, FinalStatus, RepositoryRevision, RunState, VerificationAccepted,
    VerificationRequest, VerificationResult, VerifiedRepository, VERIFICATION_PROTOCOL_VERSION,
};
use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

fn request() -> VerificationRequest {
    VerificationRequest {
        version: VERIFICATION_PROTOCOL_VERSION,
        run_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        session_id: Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        scenario_id: "coupon-tax-inclusive".into(),
        scenario_version: "1.0.0".into(),
        expected_manifest_digest: "c".repeat(64),
        repositories: vec![RepositoryRevision {
            repository_id: "pricing-api".into(),
            clone_url: "https://github.com/example/pricing-api.git".into(),
            base_commit_sha: "a".repeat(40),
            head_commit_sha: "b".repeat(40),
        }],
    }
}

fn artifact(name: &str, visibility: EvidenceVisibility, path: &str) -> ArtifactRef {
    ArtifactRef {
        name: name.into(),
        sha256: "e".repeat(64),
        byte_len: 12,
        visibility,
        path: Some(path.into()),
    }
}

fn fixture_result() -> VerificationResult {
    VerificationResult {
        version: VERIFICATION_PROTOCOL_VERSION,
        run_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        session_id: Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
        scenario_id: "coupon-tax-inclusive".into(),
        scenario_version: "1.0.0".into(),
        status: FinalStatus::Failed,
        request_digest: "1".repeat(64),
        commit_set_digest: "2".repeat(64),
        repositories: vec![VerifiedRepository {
            repository_id: "pricing-api".into(),
            base_commit_sha: "a".repeat(40),
            head_commit_sha: "b".repeat(40),
            changed_paths: vec!["src/main.rs".into()],
        }],
        environment: EnvironmentEvidence {
            manifest_digest: "3".repeat(64),
            image_reference: "toolchain@sha256:deadbeef".into(),
            image_digest: format!("sha256:{}", "4".repeat(64)),
            backend: "docker-cli-v1".into(),
        },
        normalized_result_digest: String::new(),
        started_at: Utc.with_ymd_and_hms(2026, 8, 31, 0, 0, 0).unwrap(),
        finished_at: Utc.with_ymd_and_hms(2026, 8, 31, 0, 0, 3).unwrap(),
        checks: vec![
            CheckResult {
                id: "public-build".into(),
                phase: CheckPhase::Build,
                status: FinalStatus::Passed,
                visibility: EvidenceVisibility::Player,
                assertions: vec![AssertionResult {
                    key: "exit_code".into(),
                    passed: true,
                    expected: Some(json!(0)),
                    observed: Some(json!(0)),
                    message: None,
                }],
                stdout_artifact: Some(artifact(
                    "build-stdout",
                    EvidenceVisibility::Player,
                    "/tmp/run/build.out",
                )),
                stderr_artifact: None,
                duration_ms: 300,
            },
            CheckResult {
                id: "tax-inclusive-contract".into(),
                phase: CheckPhase::Probe,
                status: FinalStatus::Failed,
                visibility: EvidenceVisibility::EvaluatorOnly,
                assertions: vec![AssertionResult {
                    key: "/discount_total".into(),
                    passed: false,
                    expected: Some(json!(1000)),
                    observed: Some(json!(909)),
                    message: Some("hidden expected value".into()),
                }],
                stdout_artifact: Some(artifact(
                    "hidden-stdout",
                    EvidenceVisibility::EvaluatorOnly,
                    "/tmp/run/secret.out",
                )),
                stderr_artifact: None,
                duration_ms: 400,
            },
        ],
        artifacts: vec![
            artifact(
                "build-stdout",
                EvidenceVisibility::Player,
                "/tmp/run/build.out",
            ),
            artifact(
                "hidden-stdout",
                EvidenceVisibility::EvaluatorOnly,
                "/tmp/run/secret.out",
            ),
        ],
        failure: Some(FailureSummary {
            code: "checks_failed".into(),
            message: "tax-inclusive-contract observed 909".into(),
        }),
    }
}

#[test]
fn verdict_strings_are_stable() {
    assert_eq!(
        serde_json::to_string(&FinalStatus::Passed).unwrap(),
        "\"passed\""
    );
    assert_eq!(
        serde_json::to_string(&FinalStatus::PolicyBlocked).unwrap(),
        "\"policy_blocked\""
    );
    assert_eq!(
        serde_json::to_string(&FinalStatus::InfraError).unwrap(),
        "\"infra_error\""
    );
    assert_eq!(
        serde_json::to_string(&FinalStatus::TimedOut).unwrap(),
        "\"timed_out\""
    );
    assert_eq!(
        serde_json::to_string(&RunState::Preparing).unwrap(),
        "\"preparing\""
    );
}

#[test]
fn accepted_response_round_trips() {
    let accepted = VerificationAccepted {
        version: VERIFICATION_PROTOCOL_VERSION,
        run_id: request().run_id,
        request_digest: "d".repeat(64),
        state: RunState::Queued,
    };
    let decoded: VerificationAccepted =
        serde_json::from_slice(&serde_json::to_vec(&accepted).unwrap()).unwrap();
    assert_eq!(decoded, accepted);
}

#[test]
fn public_projection_never_serializes_hidden_probe_details() {
    let result = fixture_result();
    let projection = result.public_projection();
    let encoded = serde_json::to_string(&projection).unwrap();
    assert!(!encoded.contains("tax-inclusive-contract"));
    assert!(!encoded.contains("discount_total"));
    assert!(!encoded.contains("909"));
    assert!(!encoded.contains("secret.out"));
    assert_eq!(projection.hidden_checks.failed, 1);
    assert_eq!(projection.public_checks.len(), 1);
    assert_eq!(projection.player_artifacts[0].path, None);
}

#[test]
fn canonical_json_sorts_object_keys_and_preserves_array_order() {
    let value = json!({"z": 1, "a": {"y": true, "b": false}, "list": [2, 1]});
    let bytes = canonical_json_bytes(&value).unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        r#"{"a":{"b":false,"y":true},"list":[2,1],"z":1}"#
    );
}

#[test]
fn run_id_timestamps_duration_paths_and_order_do_not_change_normalized_digest() {
    let first = fixture_result();
    let mut second = first.clone();
    second.run_id = Uuid::new_v4();
    second.session_id = Uuid::new_v4();
    second.started_at += chrono::Duration::seconds(10);
    second.finished_at += chrono::Duration::seconds(15);
    second.checks.reverse();
    second.checks[0].assertions.reverse();
    second.checks[0].duration_ms = 9999;
    second.artifacts.reverse();
    for artifact in &mut second.artifacts {
        artifact.path = Some("/different/temp/path".into());
    }
    assert_eq!(
        normalized_result_digest(&first).unwrap(),
        normalized_result_digest(&second).unwrap()
    );
}

#[test]
fn objective_change_changes_normalized_digest() {
    let first = fixture_result();
    let mut second = first.clone();
    second.checks[1].assertions[0].observed = Some(json!(1000));
    second.checks[1].assertions[0].passed = true;
    second.checks[1].status = FinalStatus::Passed;
    assert_ne!(
        normalized_result_digest(&first).unwrap(),
        normalized_result_digest(&second).unwrap()
    );
}

#[test]
fn request_digest_ignores_run_id_but_commit_set_is_order_independent() {
    let first = request();
    let mut second = first.clone();
    second.run_id = Uuid::new_v4();
    assert_eq!(
        request_digest(&first).unwrap(),
        request_digest(&second).unwrap()
    );

    let mut reversed = first.repositories.clone();
    reversed.reverse();
    assert_eq!(
        commit_set_digest(&first.repositories).unwrap(),
        commit_set_digest(&reversed).unwrap()
    );
}
