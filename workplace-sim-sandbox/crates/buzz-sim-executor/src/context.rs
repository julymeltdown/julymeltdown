use std::collections::{BTreeMap, BTreeSet};

use buzz_sim_agent::{NpcPersona, PersonaDirectory};
use buzz_sim_github::{
    ActorDirectory, ActorKind, DestinationRepository, RepositoryAccess, ResolvedActor,
};
use uuid::Uuid;

use crate::ExecutionError;

/// Immutable execution facts for one session repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryExecutionTarget {
    /// Session-owned GitHub repository coordinate.
    pub destination: DestinationRepository,
    /// Exact source commit used to seed the session repository.
    pub base_commit_sha: String,
    /// Current exact head commit eligible for work and verification.
    pub head_commit_sha: String,
    /// Trusted SHA-256 verification manifest digest.
    pub manifest_digest: String,
}

impl RepositoryExecutionTarget {
    /// Creates and validates one private session repository target.
    pub fn new(
        destination: DestinationRepository,
        base_commit_sha: impl Into<String>,
        head_commit_sha: impl Into<String>,
        manifest_digest: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        let target = Self {
            destination,
            base_commit_sha: base_commit_sha.into().to_ascii_lowercase(),
            head_commit_sha: head_commit_sha.into().to_ascii_lowercase(),
            manifest_digest: manifest_digest.into().to_ascii_lowercase(),
        };
        if !target.destination.private {
            return Err(ExecutionError::InvalidContext {
                reason: format!(
                    "session repository {:?} must be private",
                    target.destination.repository_id
                ),
            });
        }
        if !full_git_object_id(&target.base_commit_sha) {
            return Err(ExecutionError::InvalidContext {
                reason: format!(
                    "repository {:?} has invalid base commit",
                    target.destination.repository_id
                ),
            });
        }
        if !full_git_object_id(&target.head_commit_sha) {
            return Err(ExecutionError::InvalidContext {
                reason: format!(
                    "repository {:?} has invalid head commit",
                    target.destination.repository_id
                ),
            });
        }
        if !sha256_digest(&target.manifest_digest) {
            return Err(ExecutionError::InvalidContext {
                reason: format!(
                    "repository {:?} has invalid manifest digest",
                    target.destination.repository_id
                ),
            });
        }
        Ok(target)
    }

    /// Returns the scenario-local repository identifier.
    #[must_use]
    pub fn repository_id(&self) -> &str {
        &self.destination.repository_id
    }
}

/// Current authority and routing projection used to execute one NPC turn.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    session_id: Uuid,
    player_actor_id: String,
    scenario_id: String,
    scenario_version: String,
    personas: PersonaDirectory,
    actors: ActorDirectory,
    repositories: BTreeMap<String, RepositoryExecutionTarget>,
    review_routes: BTreeMap<String, BTreeSet<String>>,
}

impl ExecutionContext {
    /// Builds a fail-closed execution context from current session projections.
    // Keeping each authority projection explicit makes accidental privilege widening visible.
    #[allow(clippy::too_many_arguments)]
    pub fn new<I>(
        session_id: Uuid,
        player_actor_id: impl Into<String>,
        scenario_id: impl Into<String>,
        scenario_version: impl Into<String>,
        personas: PersonaDirectory,
        actors: ActorDirectory,
        repositories: I,
        review_routes: BTreeMap<String, BTreeSet<String>>,
    ) -> Result<Self, ExecutionError>
    where
        I: IntoIterator<Item = RepositoryExecutionTarget>,
    {
        let player_actor_id = player_actor_id.into();
        let scenario_id = scenario_id.into();
        let scenario_version = scenario_version.into();
        require_text("player_actor_id", &player_actor_id)?;
        require_text("scenario_id", &scenario_id)?;
        require_text("scenario_version", &scenario_version)?;

        let player = actors
            .resolve_by_actor_id(&player_actor_id)
            .ok_or_else(|| ExecutionError::InvalidContext {
                reason: format!("player actor {player_actor_id:?} is missing"),
            })?;
        if player.kind != ActorKind::Player {
            return Err(ExecutionError::InvalidContext {
                reason: format!("actor {player_actor_id:?} must have kind player"),
            });
        }

        let mut repository_map = BTreeMap::new();
        for repository in repositories {
            let repository_id = repository.repository_id().to_string();
            if repository_map
                .insert(repository_id.clone(), repository)
                .is_some()
            {
                return Err(ExecutionError::InvalidContext {
                    reason: format!("duplicate repository {repository_id:?}"),
                });
            }
        }
        if repository_map.is_empty() {
            return Err(ExecutionError::InvalidContext {
                reason: "execution context requires at least one repository".to_string(),
            });
        }

        for persona in personas.personas() {
            let actor = actors.resolve_by_actor_id(&persona.id).ok_or_else(|| {
                ExecutionError::InvalidContext {
                    reason: format!("persona actor {:?} has no GitHub binding", persona.id),
                }
            })?;
            if actor.kind != ActorKind::Npc {
                return Err(ExecutionError::InvalidContext {
                    reason: format!("persona actor {:?} must have kind npc", persona.id),
                });
            }
            for (repository_id, required) in &persona.repository_access {
                if !repository_map.contains_key(repository_id) {
                    return Err(ExecutionError::InvalidContext {
                        reason: format!(
                            "persona {:?} references repository {repository_id:?} outside the session",
                            persona.id
                        ),
                    });
                }
                let current = actor.access_for(repository_id);
                if !current.is_some_and(|access| access.allows(*required)) {
                    return Err(ExecutionError::InvalidContext {
                        reason: format!(
                            "GitHub binding for {:?} does not satisfy {:?} access to {repository_id:?}",
                            persona.id, required
                        ),
                    });
                }
            }
        }

        for (repository_id, reviewer_actor_ids) in &review_routes {
            if !repository_map.contains_key(repository_id) {
                return Err(ExecutionError::InvalidContext {
                    reason: format!("review route references unknown repository {repository_id:?}"),
                });
            }
            if reviewer_actor_ids.is_empty() {
                return Err(ExecutionError::InvalidContext {
                    reason: format!("review route for {repository_id:?} is empty"),
                });
            }
            for actor_id in reviewer_actor_ids {
                let reviewer = actors.resolve_by_actor_id(actor_id).ok_or_else(|| {
                    ExecutionError::InvalidContext {
                        reason: format!("reviewer actor {actor_id:?} is missing"),
                    }
                })?;
                if reviewer.kind != ActorKind::Npc {
                    return Err(ExecutionError::InvalidContext {
                        reason: format!("reviewer actor {actor_id:?} must have kind npc"),
                    });
                }
                let access = reviewer.access_for(repository_id);
                if !access.is_some_and(|value| value.allows(RepositoryAccess::Read)) {
                    return Err(ExecutionError::InvalidContext {
                        reason: format!(
                            "reviewer actor {actor_id:?} cannot read repository {repository_id:?}"
                        ),
                    });
                }
            }
        }

        Ok(Self {
            session_id,
            player_actor_id,
            scenario_id,
            scenario_version,
            personas,
            actors,
            repositories: repository_map,
            review_routes,
        })
    }

    /// Returns the active simulation session identifier.
    #[must_use]
    pub const fn session_id(&self) -> Uuid {
        self.session_id
    }

    /// Returns the human player actor identifier.
    #[must_use]
    pub fn player_actor_id(&self) -> &str {
        &self.player_actor_id
    }

    /// Returns the trusted scenario identifier.
    #[must_use]
    pub fn scenario_id(&self) -> &str {
        &self.scenario_id
    }

    /// Returns the immutable scenario version.
    #[must_use]
    pub fn scenario_version(&self) -> &str {
        &self.scenario_version
    }

    /// Returns the active persona directory.
    #[must_use]
    pub const fn personas(&self) -> &PersonaDirectory {
        &self.personas
    }

    /// Resolves one NPC persona.
    #[must_use]
    pub fn persona(&self, actor_id: &str) -> Option<&NpcPersona> {
        self.personas.resolve(actor_id)
    }

    /// Resolves one GitHub actor binding.
    #[must_use]
    pub fn actor(&self, actor_id: &str) -> Option<&ResolvedActor> {
        self.actors.resolve_by_actor_id(actor_id)
    }

    /// Resolves one current session repository.
    #[must_use]
    pub fn repository(&self, repository_id: &str) -> Option<&RepositoryExecutionTarget> {
        self.repositories.get(repository_id)
    }

    /// Resolves configured reviewer GitHub logins in stable order.
    pub fn reviewer_logins(&self, repository_id: &str) -> Result<BTreeSet<String>, ExecutionError> {
        let actor_ids = self.review_routes.get(repository_id).ok_or_else(|| {
            ExecutionError::NoReviewersConfigured {
                repository_id: repository_id.to_string(),
            }
        })?;
        let mut logins = BTreeSet::new();
        for actor_id in actor_ids {
            let actor = self.actors.resolve_by_actor_id(actor_id).ok_or_else(|| {
                ExecutionError::UnknownActor {
                    actor_id: actor_id.clone(),
                }
            })?;
            logins.insert(actor.github_login.clone());
        }
        if logins.is_empty() {
            Err(ExecutionError::NoReviewersConfigured {
                repository_id: repository_id.to_string(),
            })
        } else {
            Ok(logins)
        }
    }
}

fn require_text(field: &'static str, value: &str) -> Result<(), ExecutionError> {
    if value.trim().is_empty() || value.len() > 256 {
        Err(ExecutionError::InvalidContext {
            reason: format!("{field} must contain 1..=256 bytes"),
        })
    } else {
        Ok(())
    }
}

fn full_git_object_id(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64)
        && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn sha256_digest(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
}
