use buzz_sim_github::{
    CredentialAccess, DestinationRepository, GitCredentialScope, ProvisioningError, SeedOperation,
    SeedPlan, SourceRevision,
};
use uuid::Uuid;

const BASE_SHA: &str = "1111111111111111111111111111111111111111";
const HEAD_SHA: &str = "2222222222222222222222222222222222222222";

fn source() -> SourceRevision {
    SourceRevision::new(
        "legacy-cart",
        "https://github.com/acme/legacy-cart.git",
        BASE_SHA,
    )
    .unwrap()
}

fn destination() -> DestinationRepository {
    DestinationRepository::new(
        "legacy-cart",
        "acme-sim",
        "session-legacy-cart",
        "main",
        true,
    )
    .unwrap()
}

#[test]
fn source_revision_requires_a_full_git_object_id() {
    assert_eq!(
        SourceRevision::new(
            "legacy-cart",
            "https://github.com/acme/legacy-cart.git",
            "deadbee",
        )
        .unwrap_err(),
        ProvisioningError::InvalidCommitSha
    );
}

#[test]
fn seed_plan_scopes_credentials_to_exact_repositories() {
    let source = source();
    let source_scope = GitCredentialScope::for_source(&source).unwrap();
    let destination = destination();
    let plan = SeedPlan::new(
        Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap(),
        source.clone(),
        destination.clone(),
        Some(source_scope.clone()),
    )
    .unwrap();

    assert!(source_scope.allows_clone_url(&source.clone_url, CredentialAccess::Read));
    assert!(!source_scope.allows_clone_url(&source.clone_url, CredentialAccess::Write));
    assert!(!source_scope.allows_clone_url(&destination.clone_url(), CredentialAccess::Read));
    assert!(plan
        .destination_scope
        .allows_clone_url(&destination.clone_url(), CredentialAccess::Write));
    assert!(!plan
        .destination_scope
        .allows_clone_url(&source.clone_url, CredentialAccess::Read));
    assert_eq!(plan.target_ref, "refs/heads/main");

    let operations = plan.operations();
    assert!(matches!(
        &operations[0],
        SeedOperation::FetchExactCommit {
            clone_url,
            commit_sha,
        } if clone_url == &source.clone_url && commit_sha == BASE_SHA
    ));
    assert!(matches!(
        &operations[1],
        SeedOperation::PushExactCommit {
            clone_url,
            commit_sha,
            ref_name,
        } if clone_url == &destination.clone_url()
            && commit_sha == BASE_SHA
            && ref_name == "refs/heads/main"
    ));
}

#[test]
fn source_and_destination_must_not_be_the_same_repository() {
    let destination =
        DestinationRepository::new("legacy-cart", "acme", "legacy-cart", "main", true).unwrap();

    assert_eq!(
        SeedPlan::new(Uuid::nil(), source(), destination, None).unwrap_err(),
        ProvisioningError::SourceEqualsDestination("github.com/acme/legacy-cart".to_string())
    );
}

#[test]
fn source_scope_cannot_be_reused_for_another_repository() {
    let other = SourceRevision::new(
        "pricing-api",
        "https://github.com/acme/pricing-api.git",
        BASE_SHA,
    )
    .unwrap();
    let wrong_scope = GitCredentialScope::for_source(&other).unwrap();

    assert!(matches!(
        SeedPlan::new(Uuid::nil(), source(), destination(), Some(wrong_scope)),
        Err(ProvisioningError::CredentialScopeMismatch { .. })
    ));
}

#[test]
fn verification_revision_targets_the_isolated_destination() {
    let plan = SeedPlan::new(Uuid::nil(), source(), destination(), None).unwrap();
    let revision = plan.verification_revision(HEAD_SHA).unwrap();

    assert_eq!(revision.repository_id, "legacy-cart");
    assert_eq!(revision.base_commit_sha, BASE_SHA);
    assert_eq!(revision.head_commit_sha, HEAD_SHA);
    assert_eq!(
        revision.clone_url,
        "https://github.com/acme-sim/session-legacy-cart.git"
    );
}

#[test]
fn serialized_plan_contains_no_token_or_embedded_credential() {
    let plan = SeedPlan::new(Uuid::nil(), source(), destination(), None).unwrap();
    let serialized = serde_json::to_string(&plan).unwrap();

    assert!(!serialized.contains("token"));
    assert!(!serialized.contains("password"));
    assert!(!serialized.contains('@'));
}
