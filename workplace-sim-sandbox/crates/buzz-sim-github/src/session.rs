use std::collections::{BTreeMap, BTreeSet};

use buzz_sim_protocol::RepositoryRevision;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ActorBinding, ActorDirectory, ActorKind, DestinationRepository, GitCredentialScope,
    ProvisioningError, RepositoryAccess, ResolvedActor, SeedPlan, SourceRevision,
};

/// One source-to-destination repository specification inside a simulation session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRepositorySpec {
    /// Immutable source revision.
    pub source: SourceRevision,
    /// Session-owned destination repository.
    pub destination: DestinationRepository,
    /// Optional exact read scope for a private source repository.
    pub source_scope: Option<GitCredentialScope>,
}

impl SessionRepositorySpec {
    /// Creates a repository specification. Cross-field validation happens during compilation.
    #[must_use]
    pub fn new(
        source: SourceRevision,
        destination: DestinationRepository,
        source_scope: Option<GitCredentialScope>,
    ) -> Self {
        Self {
            source,
            destination,
            source_scope,
        }
    }
}

/// Complete declarative input for one isolated GitHub simulation session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionProvisioningSpec {
    /// Simulation session identifier.
    pub session_id: Uuid,
    /// Repositories copied into the session.
    pub repositories: Vec<SessionRepositorySpec>,
    /// Player, NPC, and service GitHub identity mappings.
    pub actors: Vec<ActorBinding>,
}

impl SessionProvisioningSpec {
    /// Creates an uncompiled specification.
    #[must_use]
    pub fn new(
        session_id: Uuid,
        repositories: Vec<SessionRepositorySpec>,
        actors: Vec<ActorBinding>,
    ) -> Self {
        Self {
            session_id,
            repositories,
            actors,
        }
    }
}

/// Exact destination grant emitted for one actor and one session repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryGrant {
    /// Stable simulation actor identifier.
    pub actor_id: String,
    /// Canonical GitHub login.
    pub github_login: String,
    /// Principal category.
    pub actor_kind: ActorKind,
    /// Stable scenario-local repository identifier.
    pub repository_id: String,
    /// Destination Git host.
    pub destination_host: String,
    /// Destination owner.
    pub destination_owner: String,
    /// Destination repository name.
    pub destination_repository: String,
    /// Granted access level.
    pub access: RepositoryAccess,
}

impl RepositoryGrant {
    /// Returns the uncredentialed destination clone URL.
    #[must_use]
    pub fn clone_url(&self) -> String {
        format!(
            "https://{}/{}/{}.git",
            self.destination_host, self.destination_owner, self.destination_repository
        )
    }
}

/// Deterministic, validation-complete plan consumed by a GitHub API adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionProvisioningPlan {
    /// Simulation session identifier.
    pub session_id: Uuid,
    /// Seed plans sorted by scenario-local repository identifier.
    pub seeds: Vec<SeedPlan>,
    /// Validated actors sorted by stable actor identifier.
    pub actors: Vec<ResolvedActor>,
    /// Exact destination grants sorted by repository then actor.
    pub grants: Vec<RepositoryGrant>,
}

impl SessionProvisioningPlan {
    /// Compiles a declarative session specification into deterministic seed and grant operations.
    pub fn compile(spec: SessionProvisioningSpec) -> Result<Self, ProvisioningError> {
        if spec.repositories.is_empty() {
            return Err(ProvisioningError::EmptyRepositorySet);
        }

        let actor_directory = ActorDirectory::new(spec.actors)?;
        let mut seeds = Vec::with_capacity(spec.repositories.len());
        let mut repository_ids = BTreeSet::<String>::new();
        let mut destinations = BTreeMap::<String, DestinationRepository>::new();
        let mut destination_coordinates = BTreeSet::<String>::new();

        for repository in spec.repositories {
            let repository_id = repository.source.repository_id.clone();
            if !repository_ids.insert(repository_id.clone()) {
                return Err(ProvisioningError::DuplicateRepositoryId(repository_id));
            }
            let destination_coordinate = format!(
                "{}/{}/{}",
                repository.destination.host.to_ascii_lowercase(),
                repository.destination.owner.to_ascii_lowercase(),
                repository.destination.name.to_ascii_lowercase()
            );
            if !destination_coordinates.insert(destination_coordinate.clone()) {
                return Err(ProvisioningError::DuplicateDestination(
                    destination_coordinate,
                ));
            }

            let seed = SeedPlan::new(
                spec.session_id,
                repository.source,
                repository.destination.clone(),
                repository.source_scope,
            )?;
            destinations.insert(repository_id, repository.destination);
            seeds.push(seed);
        }
        seeds.sort_by(|left, right| {
            left.source
                .repository_id
                .cmp(&right.source.repository_id)
        });

        let mut actors = actor_directory.actors().cloned().collect::<Vec<_>>();
        actors.sort_by(|left, right| left.actor_id.cmp(&right.actor_id));

        let mut grants = Vec::new();
        for actor in &actors {
            for (repository_id, access) in &actor.repository_access {
                let destination = destinations.get(repository_id).ok_or_else(|| {
                    ProvisioningError::UnknownRepository {
                        actor_id: actor.actor_id.clone(),
                        repository_id: repository_id.clone(),
                    }
                })?;
                grants.push(RepositoryGrant {
                    actor_id: actor.actor_id.clone(),
                    github_login: actor.github_login.clone(),
                    actor_kind: actor.kind,
                    repository_id: repository_id.clone(),
                    destination_host: destination.host.clone(),
                    destination_owner: destination.owner.clone(),
                    destination_repository: destination.name.clone(),
                    access: *access,
                });
            }
        }
        grants.sort_by(|left, right| {
            (&left.repository_id, &left.actor_id).cmp(&(
                &right.repository_id,
                &right.actor_id,
            ))
        });

        Ok(Self {
            session_id: spec.session_id,
            seeds,
            actors,
            grants,
        })
    }

    /// Finds the seed plan for a scenario-local repository identifier.
    #[must_use]
    pub fn seed_for(&self, repository_id: &str) -> Option<&SeedPlan> {
        self.seeds
            .binary_search_by(|seed| seed.source.repository_id.as_str().cmp(repository_id))
            .ok()
            .map(|index| &self.seeds[index])
    }

    /// Converts current destination heads into exact sandbox verification revisions.
    pub fn verification_revisions(
        &self,
        head_commits: &BTreeMap<String, String>,
    ) -> Result<Vec<RepositoryRevision>, ProvisioningError> {
        self.seeds
            .iter()
            .map(|seed| {
                let repository_id = &seed.source.repository_id;
                let head_commit = head_commits
                    .get(repository_id)
                    .ok_or_else(|| ProvisioningError::MissingHeadCommit(repository_id.clone()))?;
                seed.verification_revision(head_commit.clone())
            })
            .collect()
    }
}
