use std::collections::BTreeMap;

use buzz_sim_github::{
    ActorBinding, ActorDirectory, ActorKind, ProvisioningError, RepositoryAccess,
};

fn access(entries: &[(&str, RepositoryAccess)]) -> BTreeMap<String, RepositoryAccess> {
    entries
        .iter()
        .map(|(repository_id, level)| ((*repository_id).to_string(), *level))
        .collect()
}

#[test]
fn resolves_actor_and_github_login_bidirectionally() {
    let binding = ActorBinding::new(
        "npc:minseo",
        "Min-Seo",
        ActorKind::Npc,
        access(&[("legacy-cart", RepositoryAccess::Maintain)]),
    )
    .unwrap();
    let directory = ActorDirectory::new([binding]).unwrap();

    let by_actor = directory.resolve_by_actor_id("npc:minseo").unwrap();
    let by_login = directory.resolve_by_github_login("MIN-SEO").unwrap();

    assert_eq!(by_actor, by_login);
    assert_eq!(by_actor.github_login, "min-seo");
    assert!(by_actor.can_write("legacy-cart"));
    assert!(!by_actor.can_write("mobile-contracts"));
}

#[test]
fn duplicate_canonical_logins_are_rejected() {
    let first = ActorBinding::new(
        "npc:minseo",
        "Min-Seo",
        ActorKind::Npc,
        BTreeMap::new(),
    )
    .unwrap();
    let second = ActorBinding::new(
        "npc:yujin",
        "min-seo",
        ActorKind::Npc,
        BTreeMap::new(),
    )
    .unwrap();

    assert_eq!(
        ActorDirectory::new([first, second]).unwrap_err(),
        ProvisioningError::DuplicateGitHubLogin("min-seo".to_string())
    );
}

#[test]
fn duplicate_actor_ids_are_rejected() {
    let first = ActorBinding::new(
        "player:developer",
        "player-dev",
        ActorKind::Player,
        BTreeMap::new(),
    )
    .unwrap();
    let second = ActorBinding::new(
        "player:developer",
        "player-alt",
        ActorKind::Player,
        BTreeMap::new(),
    )
    .unwrap();

    assert_eq!(
        ActorDirectory::new([first, second]).unwrap_err(),
        ProvisioningError::DuplicateActorId("player:developer".to_string())
    );
}

#[test]
fn read_access_never_implies_write_access() {
    let binding = ActorBinding::new(
        "npc:yujin",
        "qa-yujin",
        ActorKind::Npc,
        access(&[("mobile-contracts", RepositoryAccess::Read)]),
    )
    .unwrap();
    let directory = ActorDirectory::new([binding]).unwrap();
    let actor = directory.resolve_by_actor_id("npc:yujin").unwrap();

    assert_eq!(
        actor.access_for("mobile-contracts"),
        Some(RepositoryAccess::Read)
    );
    assert!(!actor.can_write("mobile-contracts"));
    assert!(RepositoryAccess::Maintain.allows(RepositoryAccess::Write));
    assert!(!RepositoryAccess::Read.allows(RepositoryAccess::Write));
}
