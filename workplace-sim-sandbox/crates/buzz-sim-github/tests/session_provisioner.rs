use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use buzz_sim_github::{
    ActorBinding, ActorKind, CreatedRepository, DestinationRepository, GitHubApiError,
    GitHubRepositoryApi, GitHubSessionProvisioner, GrantOutcome, ProvisioningFailurePhase,
    RepositoryAccess, RepositoryGrant, RepositorySeeder, SeedPlan, SeededRepository,
    SessionProvisioningPlan, SessionProvisioningSpec, SessionRepositorySpec, SourceRevision,
};
use uuid::Uuid;

const BASE_SHA: &str = "1111111111111111111111111111111111111111";
const LEGACY_HEAD: &str = "2222222222222222222222222222222222222222";
const PRICING_HEAD: &str = "3333333333333333333333333333333333333333";

#[derive(Debug, Clone, Default)]
struct CallLog(Arc<Mutex<Vec<String>>>);

impl CallLog {
    fn push(&self, value: impl Into<String>) {
        self.0.lock().unwrap().push(value.into());
    }

    fn snapshot(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Debug, Clone)]
struct FakeApi {
    calls: CallLog,
    fail_grant_actor: Option<String>,
    fail_delete_repository: Option<String>,
}

#[async_trait]
impl GitHubRepositoryApi for FakeApi {
    async fn create_repository(
        &self,
        destination: &DestinationRepository,
    ) -> Result<CreatedRepository, GitHubApiError> {
        self.calls
            .push(format!("create:{}", destination.repository_id));
        Ok(CreatedRepository {
            id: 100,
            name: destination.name.clone(),
            clone_url: destination.clone_url(),
            private: destination.private,
            default_branch: destination.default_branch.clone(),
        })
    }

    async fn grant_repository_access(
        &self,
        grant: &RepositoryGrant,
    ) -> Result<GrantOutcome, GitHubApiError> {
        self.calls
            .push(format!("grant:{}:{}", grant.repository_id, grant.actor_id));
        if self.fail_grant_actor.as_deref() == Some(grant.actor_id.as_str()) {
            return Err(GitHubApiError::HttpStatus {
                operation: "grant_repository_access",
                status: 403,
                body: "denied".to_string(),
            });
        }
        Ok(GrantOutcome::AccessUpdated)
    }

    async fn delete_repository(
        &self,
        destination: &DestinationRepository,
    ) -> Result<(), GitHubApiError> {
        self.calls
            .push(format!("delete:{}", destination.repository_id));
        if self.fail_delete_repository.as_deref() == Some(destination.repository_id.as_str()) {
            return Err(GitHubApiError::HttpStatus {
                operation: "delete_repository",
                status: 403,
                body: "deletion denied".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct FakeSeeder {
    calls: CallLog,
    fail_repository: Option<String>,
}

#[async_trait]
impl RepositorySeeder for FakeSeeder {
    async fn seed_repository(
        &self,
        plan: &SeedPlan,
    ) -> Result<SeededRepository, GitHubApiError> {
        self.calls.push(format!(
            "seed:{}:{}",
            plan.source.repository_id, plan.source.commit_sha
        ));
        if self.fail_repository.as_deref() == Some(plan.source.repository_id.as_str()) {
            return Err(GitHubApiError::Transport("seed failed".to_string()));
        }
        let head_commit_sha = match plan.source.repository_id.as_str() {
            "legacy-cart" => LEGACY_HEAD,
            "pricing-api" => PRICING_HEAD,
            other => panic!("unexpected repository {other}"),
        };
        Ok(SeededRepository {
            repository_id: plan.source.repository_id.clone(),
            head_commit_sha: head_commit_sha.to_string(),
        })
    }
}

fn repository(repository_id: &str) -> SessionRepositorySpec {
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
            format!("session-{repository_id}"),
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
    entries: &[(&str, RepositoryAccess)],
) -> ActorBinding {
    ActorBinding::new(
        actor_id,
        login,
        if actor_id.starts_with("player:") {
            ActorKind::Player
        } else {
            ActorKind::Npc
        },
        entries
            .iter()
            .map(|(repository_id, access)| ((*repository_id).to_string(), *access))
            .collect(),
    )
    .unwrap()
}

fn plan() -> SessionProvisioningPlan {
    SessionProvisioningPlan::compile(SessionProvisioningSpec::new(
        Uuid::parse_str("00000000-0000-4000-8000-000000000077").unwrap(),
        vec![repository("pricing-api"), repository("legacy-cart")],
        vec![
            actor(
                "npc:minseo",
                "staff-minseo",
                &[
                    ("legacy-cart", RepositoryAccess::Read),
                    ("pricing-api", RepositoryAccess::Maintain),
                ],
            ),
            actor(
                "player:developer",
                "player-dev",
                &[("legacy-cart", RepositoryAccess::Write)],
            ),
        ],
    ))
    .unwrap()
}

#[tokio::test]
async fn provisioner_creates_and_seeds_before_granting_actor_access() {
    let calls = CallLog::default();
    let provisioner = GitHubSessionProvisioner::new(
        FakeApi {
            calls: calls.clone(),
            fail_grant_actor: None,
            fail_delete_repository: None,
        },
        FakeSeeder {
            calls: calls.clone(),
            fail_repository: None,
        },
    );

    let plan = plan();
    let provisioned = provisioner.provision(&plan).await.unwrap();

    assert_eq!(
        calls.snapshot(),
        vec![
            "create:legacy-cart".to_string(),
            format!("seed:legacy-cart:{BASE_SHA}"),
            "create:pricing-api".to_string(),
            format!("seed:pricing-api:{BASE_SHA}"),
            "grant:legacy-cart:npc:minseo".to_string(),
            "grant:legacy-cart:player:developer".to_string(),
            "grant:pricing-api:npc:minseo".to_string(),
        ]
    );
    assert_eq!(provisioned.repositories.len(), 2);
    assert_eq!(provisioned.grants.len(), 3);
    assert_eq!(
        provisioned.head_commits,
        BTreeMap::from([
            ("legacy-cart".to_string(), LEGACY_HEAD.to_string()),
            ("pricing-api".to_string(), PRICING_HEAD.to_string()),
        ])
    );
    let verification = provisioned.verification_revisions(&plan).unwrap();
    assert_eq!(verification[0].head_commit_sha, LEGACY_HEAD);
    assert_eq!(verification[1].head_commit_sha, PRICING_HEAD);
}

#[tokio::test]
async fn seed_failure_rolls_back_created_repositories_in_reverse_order() {
    let calls = CallLog::default();
    let provisioner = GitHubSessionProvisioner::new(
        FakeApi {
            calls: calls.clone(),
            fail_grant_actor: None,
            fail_delete_repository: None,
        },
        FakeSeeder {
            calls: calls.clone(),
            fail_repository: Some("pricing-api".to_string()),
        },
    );

    let failure = provisioner.provision(&plan()).await.unwrap_err();

    assert_eq!(failure.phase, ProvisioningFailurePhase::Seed);
    assert_eq!(failure.repository_id.as_deref(), Some("pricing-api"));
    assert!(failure.rollback_failures.is_empty());
    assert_eq!(
        calls.snapshot(),
        vec![
            "create:legacy-cart".to_string(),
            format!("seed:legacy-cart:{BASE_SHA}"),
            "create:pricing-api".to_string(),
            format!("seed:pricing-api:{BASE_SHA}"),
            "delete:pricing-api".to_string(),
            "delete:legacy-cart".to_string(),
        ]
    );
}

#[tokio::test]
async fn grant_failure_reports_rollback_failures_without_masking_primary_error() {
    let calls = CallLog::default();
    let provisioner = GitHubSessionProvisioner::new(
        FakeApi {
            calls: calls.clone(),
            fail_grant_actor: Some("player:developer".to_string()),
            fail_delete_repository: Some("legacy-cart".to_string()),
        },
        FakeSeeder {
            calls: calls.clone(),
            fail_repository: None,
        },
    );

    let failure = provisioner.provision(&plan()).await.unwrap_err();

    assert_eq!(failure.phase, ProvisioningFailurePhase::Grant);
    assert_eq!(failure.actor_id.as_deref(), Some("player:developer"));
    assert_eq!(failure.repository_id.as_deref(), Some("legacy-cart"));
    assert_eq!(failure.rollback_failures.len(), 1);
    assert_eq!(failure.rollback_failures[0].repository_id, "legacy-cart");
    assert!(failure.to_string().contains("grant_repository_access"));
    assert_eq!(
        calls.snapshot().last().map(String::as_str),
        Some("delete:legacy-cart")
    );
}
