use buzz_sim_protocol::{
    FinalStatus, RepositoryRevision, RunState, VerificationAccepted,
    VerificationRequest, VERIFICATION_PROTOCOL_VERSION,
};
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

#[test]
fn verdict_strings_are_stable() {
    assert_eq!(serde_json::to_string(&FinalStatus::Passed).unwrap(), "\"passed\"");
    assert_eq!(serde_json::to_string(&FinalStatus::PolicyBlocked).unwrap(), "\"policy_blocked\"");
    assert_eq!(serde_json::to_string(&FinalStatus::InfraError).unwrap(), "\"infra_error\"");
    assert_eq!(serde_json::to_string(&FinalStatus::TimedOut).unwrap(), "\"timed_out\"");
    assert_eq!(serde_json::to_string(&RunState::Preparing).unwrap(), "\"preparing\"");
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
