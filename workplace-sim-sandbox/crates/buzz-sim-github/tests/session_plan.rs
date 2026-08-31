use std::collections::BTreeMap;

use buzz_sim_github::{
    ActorBinding, ActorKind, DestinationRepository, ProvisioningError, RepositoryAccess,
    SessionProvisioningPlan, SessionProvisioningSpec, SessionRepositorySpec, SourceRevision,
};
use uuid::Uuid;

const BASE_SHA: &str = "1111111111111111111111111111111111111111";
const HEAD_SHA: &str = "2222222222222222222222222222222222222222";

fn repository(repository_id: &str, destination_name: &str) -> SessionRepositorySpec {
    SessionRepositorySpec::new(
        SourceRevision::new(
            repository_id,
            format!("https://github.com/acme/{repository_id}.git"),
            BASE_SHA,
        )
        .unwrap(),
        DestinationRepository::new(
            repository_id,
            "acme-sim",
            destination_name,
            "main",
            true,
        )
        .unwrap(),
        None,
    )
}

fn actor(
    actor_id: &str,
    login: &str,
    kind: ActorKind,
    entries: &[(&str, RepositoryAccess)],
) -> ActorBinding {
    ActorBinding::new(
        actor_id,
        login,
        kind,
        entries
            .iter()
            .map(|(repository_id, access)| ((*repository_id).to_string(), *access))
            .collect(),
    )
    .unwrap()
}

#[test]
fn compilation_is_deterministic_and_targets_only_session_repositories() {
    let session_id = Uuid::parse_str("00000000-0000-4000-8000-000000000042").unwrap();
    let spec = SessionProvisioningSpec::new(
        session_id,
        vec![
            repository("pricing-api", "session-pricing-api"),
            repository("legacy-cart", "session-legacy-cart"),
        ],
        vec![
            actor(
                "npc:minseo",
                "staff-minseo",
                ActorKind::Npc,
                &[
                    ("pricing-api", RepositoryAccess::Maintain),
                    ("legacy-cart", RepositoryAccess::Read),
                ],
            ),
            actor(
                "player:developer",
                "player-dev",
                ActorKind::Player,
                &[("legacy-cart", RepositoryAccess::Write)],
            ),
        ],
    );

    let plan = SessionProvisioningPlan::compile(spec).unwrap();

    assert_eq!(plan.session_id, session_id);
    assert_eq!(plan.seeds[0].source.repository_id, "legacy-cart");
    assert_eq!(plan.seeds[1].source.repository_id, "pricing-api");
    assert_eq!(plan.actors[0].actor_id, "npc:minseo");
    assert_eq!(plan.actors[1].actor_id, "player:developer");
    assert_eq!(plan.grants.len(), 3);
    assert_eq!(plan.grants[0].repository_id, "legacy-cart");
    assert_eq!(plan.grants[0].actor_id, "npc:minseo");
    assert_eq!(
        plan.grants[0].clone_url(),
        "https://github.com/acme-sim/session-legacy-cart.git"
    );
    assert!(plan
        .grants
        .iter()
        .all(|grant| grant.destination_owner == "acme-sim"));
    assert!(plan
        .grants
        .iter()
        .all(|grant| !grant.clone_url().contains("github.com/acme/")));
}

#[test]
fn actor_access_to_unknown_repository_is_rejected() {
    let spec = SessionProvisioningSpec::new(
        Uuid::nil(),
        vec![repository("legacy-cart", "session-legacy-cart")],
        vec![actor(
            "npc:minseo",
            "staff-minseo",
            ActorKind::Npc,
            &[("payment-core", RepositoryAccess::Read)],
        )],
    );

    assert_eq!(
        SessionProvisioningPlan::compile(spec).unwrap_err(),
        ProvisioningError::UnknownRepository {
            actor_id: "npc:minseo".to_string(),
            repository_id: "payment-core".to_string(),
        }
    );
}

#[test]
fn duplicate_destination_repository_is_rejected() {
    let first = repository("legacy-cart", "shared-session-repo");
    let second = repository("pricing-api", "shared-session-repo");
    let spec = SessionProvisioningSpec::new(Uuid::nil(), vec![first, second], vec![]);

    assert_eq!(
        SessionProvisioningPlan::compile(spec).unwrap_err(),
        ProvisioningError::DuplicateDestination(
            "github.com/acme-sim/shared-session-repo".to_string()
        )
    );
}

#[test]
fn verification_projection_requires_every_repository_head() {
    let plan = SessionProvisioningPlan::compile(SessionProvisioningSpec::new(
        Uuid::nil(),
        vec![
            repository("legacy-cart", "session-legacy-cart"),
            repository("pricing-api", "session-pricing-api"),
        ],
        vec![],
    ))
    .unwrap();
    let heads = BTreeMap::from([("legacy-cart".to_string(), HEAD_SHA.to_string())]);

    assert_eq!(
        plan.verification_revisions(&heads).unwrap_err(),
        ProvisioningError::MissingHeadCommit("pricing-api".to_string())
    );
}

#[test]
fn verification_projection_preserves_stable_repository_order() {
    let plan = SessionProvisioningPlan::compile(SessionProvisioningSpec::new(
        Uuid::nil(),
        vec![
            repository("pricing-api", "session-pricing-api"),
            repository("legacy-cart", "session-legacy-cart"),
        ],
        vec![],
    ))
    .unwrap();
    let heads = BTreeMap::from([
        ("pricing-api".to_string(), HEAD_SHA.to_string()),
        ("legacy-cart".to_string(), HEAD_SHA.to_string()),
    ]);

    let revisions = plan.verification_revisions(&heads).unwrap();
    assert_eq!(revisions[0].repository_id, "legacy-cart");
    assert_eq!(revisions[1].repository_id, "pricing-api");
    assert!(revisions
        .iter()
        .all(|revision| revision.clone_url.contains("acme-sim")));
}

#[test]
fn repository_grants_preserve_the_destination_enterprise_host() {
    let repository = SessionRepositorySpec::new(
        SourceRevision::new(
            "legacy-cart",
            "https://git.example.com/acme/legacy-cart.git",
            BASE_SHA,
        )
        .unwrap(),
        DestinationRepository::with_host(
            "legacy-cart",
            "git.example.com",
            "sim-org",
            "session-legacy-cart",
            "main",
            true,
        )
        .unwrap(),
        None,
    );
    let plan = SessionProvisioningPlan::compile(SessionProvisioningSpec::new(
        Uuid::nil(),
        vec![repository],
        vec![actor(
            "player:developer",
            "player-dev",
            ActorKind::Player,
            &[("legacy-cart", RepositoryAccess::Write)],
        )],
    ))
    .unwrap();

    assert_eq!(
        plan.grants[0].clone_url(),
        "https://git.example.com/sim-org/session-legacy-cart.git"
    );
}
