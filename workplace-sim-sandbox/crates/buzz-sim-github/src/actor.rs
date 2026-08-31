use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{validate_identifier, ProvisioningError};

/// Kind of simulator principal represented by a GitHub identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// The human player.
    Player,
    /// A persistent non-player character.
    Npc,
    /// A simulator-owned automation identity.
    Service,
}

/// Repository permission granted to one actor inside a session repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAccess {
    /// Read source, issues, pull requests, and checks.
    Read,
    /// Create branches, commits, and pull requests.
    Write,
    /// Perform repository-maintainer actions allowed by the scenario.
    Maintain,
}

impl RepositoryAccess {
    /// Returns whether this access level permits repository writes.
    #[must_use]
    pub const fn can_write(self) -> bool {
        matches!(self, Self::Write | Self::Maintain)
    }

    /// Returns whether this access level satisfies a requested level.
    #[must_use]
    pub const fn allows(self, requested: Self) -> bool {
        matches!(
            (self, requested),
            (Self::Maintain, _)
                | (Self::Write, Self::Read | Self::Write)
                | (Self::Read, Self::Read)
        )
    }
}

/// Scenario-owned mapping from a simulator actor to one GitHub login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorBinding {
    /// Stable simulation actor identifier.
    pub actor_id: String,
    /// Canonical lowercase GitHub login.
    pub github_login: String,
    /// Principal category.
    pub kind: ActorKind,
    /// Scenario-local repository identifier to access level.
    pub repository_access: BTreeMap<String, RepositoryAccess>,
}

impl ActorBinding {
    /// Creates and validates an actor binding.
    pub fn new(
        actor_id: impl Into<String>,
        github_login: impl Into<String>,
        kind: ActorKind,
        repository_access: BTreeMap<String, RepositoryAccess>,
    ) -> Result<Self, ProvisioningError> {
        Self {
            actor_id: actor_id.into(),
            github_login: github_login.into(),
            kind,
            repository_access,
        }
        .normalized()
    }

    fn normalized(mut self) -> Result<Self, ProvisioningError> {
        validate_identifier("actor_id", &self.actor_id)?;
        self.github_login = canonical_github_login(&self.github_login)?;
        for repository_id in self.repository_access.keys() {
            validate_identifier("repository_id", repository_id)?;
        }
        Ok(self)
    }
}

/// Validated actor returned by [`ActorDirectory`] lookups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedActor {
    /// Stable simulation actor identifier.
    pub actor_id: String,
    /// Canonical lowercase GitHub login.
    pub github_login: String,
    /// Principal category.
    pub kind: ActorKind,
    /// Scenario-local repository identifier to access level.
    pub repository_access: BTreeMap<String, RepositoryAccess>,
}

impl ResolvedActor {
    /// Returns the actor's access to a scenario-local repository.
    #[must_use]
    pub fn access_for(&self, repository_id: &str) -> Option<RepositoryAccess> {
        self.repository_access.get(repository_id).copied()
    }

    /// Returns whether this actor may write the named session repository.
    #[must_use]
    pub fn can_write(&self, repository_id: &str) -> bool {
        self.access_for(repository_id)
            .is_some_and(RepositoryAccess::can_write)
    }
}

impl From<ActorBinding> for ResolvedActor {
    fn from(binding: ActorBinding) -> Self {
        Self {
            actor_id: binding.actor_id,
            github_login: binding.github_login,
            kind: binding.kind,
            repository_access: binding.repository_access,
        }
    }
}

/// Bidirectional, uniqueness-checked actor identity directory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActorDirectory {
    by_actor_id: BTreeMap<String, ResolvedActor>,
    actor_id_by_login: BTreeMap<String, String>,
}

impl ActorDirectory {
    /// Builds a directory and rejects duplicate actor IDs or GitHub logins.
    pub fn new<I>(bindings: I) -> Result<Self, ProvisioningError>
    where
        I: IntoIterator<Item = ActorBinding>,
    {
        let mut directory = Self::default();
        let mut actor_ids = BTreeSet::new();
        let mut github_logins = BTreeSet::new();
        for binding in bindings {
            let actor = ResolvedActor::from(binding.normalized()?);
            if !actor_ids.insert(actor.actor_id.clone()) {
                return Err(ProvisioningError::DuplicateActorId(actor.actor_id));
            }
            if !github_logins.insert(actor.github_login.clone()) {
                return Err(ProvisioningError::DuplicateGitHubLogin(actor.github_login));
            }
            directory
                .actor_id_by_login
                .insert(actor.github_login.clone(), actor.actor_id.clone());
            directory.by_actor_id.insert(actor.actor_id.clone(), actor);
        }
        Ok(directory)
    }

    /// Resolves a stable simulator actor identifier.
    #[must_use]
    pub fn resolve_by_actor_id(&self, actor_id: &str) -> Option<&ResolvedActor> {
        self.by_actor_id.get(actor_id)
    }

    /// Resolves a GitHub login case-insensitively.
    #[must_use]
    pub fn resolve_by_github_login(&self, github_login: &str) -> Option<&ResolvedActor> {
        let canonical = canonical_github_login(github_login).ok()?;
        let actor_id = self.actor_id_by_login.get(&canonical)?;
        self.by_actor_id.get(actor_id)
    }

    /// Iterates over actors in stable actor-ID order.
    pub fn actors(&self) -> impl Iterator<Item = &ResolvedActor> {
        self.by_actor_id.values()
    }

    /// Returns the number of registered actors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_actor_id.len()
    }

    /// Returns whether no actors are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_actor_id.is_empty()
    }
}

fn canonical_github_login(value: &str) -> Result<String, ProvisioningError> {
    let login = value.trim().to_ascii_lowercase();
    let valid = !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && !login.ends_with('-')
        && !login.contains("--")
        && login
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    if valid {
        Ok(login)
    } else {
        Err(ProvisioningError::InvalidGitHubLogin(value.to_string()))
    }
}
