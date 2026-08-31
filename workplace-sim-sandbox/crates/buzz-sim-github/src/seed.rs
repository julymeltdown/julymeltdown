use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use buzz_sim_protocol::RepositoryRevision;

use crate::{validate_full_git_object_id, validate_identifier, ProvisioningError};

/// Access encoded by a short-lived Git credential scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialAccess {
    /// Fetch and inspect repository contents.
    Read,
    /// Push branches and commits into the exact destination repository.
    Write,
}

impl CredentialAccess {
    /// Returns whether this scope can satisfy the requested access.
    #[must_use]
    pub const fn allows(self, requested: Self) -> bool {
        matches!(
            (self, requested),
            (Self::Write, Self::Read | Self::Write) | (Self::Read, Self::Read)
        )
    }
}

/// Immutable source repository revision used to seed a session copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRevision {
    /// Stable scenario-local repository identifier.
    pub repository_id: String,
    /// Uncredentialed HTTPS clone URL.
    pub clone_url: String,
    /// Full SHA-1 or SHA-256 commit identifier.
    pub commit_sha: String,
}

impl SourceRevision {
    /// Creates and validates an exact source revision.
    pub fn new(
        repository_id: impl Into<String>,
        clone_url: impl Into<String>,
        commit_sha: impl Into<String>,
    ) -> Result<Self, ProvisioningError> {
        let revision = Self {
            repository_id: repository_id.into(),
            clone_url: clone_url.into(),
            commit_sha: commit_sha.into().to_ascii_lowercase(),
        };
        validate_identifier("repository_id", &revision.repository_id)?;
        parse_clone_url(&revision.clone_url)?;
        validate_full_git_object_id(&revision.commit_sha)?;
        Ok(revision)
    }
}

/// Session-owned GitHub repository that receives one exact source revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationRepository {
    /// Stable scenario-local repository identifier.
    pub repository_id: String,
    /// Git host, normally `github.com`.
    pub host: String,
    /// GitHub organization or user account that owns session repositories.
    pub owner: String,
    /// Session repository name.
    pub name: String,
    /// Branch populated by the seed operation.
    pub default_branch: String,
    /// Whether the destination must be private.
    pub private: bool,
}

impl DestinationRepository {
    /// Creates a destination hosted on `github.com`.
    pub fn new(
        repository_id: impl Into<String>,
        owner: impl Into<String>,
        name: impl Into<String>,
        default_branch: impl Into<String>,
        private: bool,
    ) -> Result<Self, ProvisioningError> {
        Self::with_host(
            repository_id,
            "github.com",
            owner,
            name,
            default_branch,
            private,
        )
    }

    /// Creates a destination on an explicit GitHub Enterprise host.
    pub fn with_host(
        repository_id: impl Into<String>,
        host: impl Into<String>,
        owner: impl Into<String>,
        name: impl Into<String>,
        default_branch: impl Into<String>,
        private: bool,
    ) -> Result<Self, ProvisioningError> {
        let destination = Self {
            repository_id: repository_id.into(),
            host: host.into().trim().to_ascii_lowercase(),
            owner: owner.into(),
            name: name.into(),
            default_branch: default_branch.into(),
            private,
        };
        validate_identifier("repository_id", &destination.repository_id)?;
        validate_host(&destination.host)?;
        validate_github_name("owner", &destination.owner, true)?;
        validate_github_name("repository", &destination.name, false)?;
        validate_ref_name(&destination.default_branch)?;
        Ok(destination)
    }

    /// Returns the uncredentialed HTTPS clone URL for this destination.
    #[must_use]
    pub fn clone_url(&self) -> String {
        format!(
            "https://{}/{}/{}.git",
            self.host, self.owner, self.name
        )
    }

    fn coordinate(&self) -> RepositoryCoordinate {
        RepositoryCoordinate {
            host: self.host.to_ascii_lowercase(),
            owner: self.owner.to_ascii_lowercase(),
            name: self.name.to_ascii_lowercase(),
        }
    }
}

/// Repository-exact credential boundary; it deliberately contains no token value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitCredentialScope {
    /// Git host this credential may contact.
    pub host: String,
    /// Exact owner this credential may access.
    pub owner: String,
    /// Exact repository this credential may access.
    pub repository: String,
    /// Maximum access permitted by the credential.
    pub access: CredentialAccess,
}

impl GitCredentialScope {
    /// Creates the least-privilege read scope for a source repository.
    pub fn for_source(source: &SourceRevision) -> Result<Self, ProvisioningError> {
        let coordinate = parse_clone_url(&source.clone_url)?;
        Ok(Self {
            host: coordinate.host,
            owner: coordinate.owner,
            repository: coordinate.name,
            access: CredentialAccess::Read,
        })
    }

    /// Creates a scope for an exact destination repository.
    #[must_use]
    pub fn for_destination(
        destination: &DestinationRepository,
        access: CredentialAccess,
    ) -> Self {
        Self {
            host: destination.host.to_ascii_lowercase(),
            owner: destination.owner.to_ascii_lowercase(),
            repository: destination.name.to_ascii_lowercase(),
            access,
        }
    }

    /// Returns whether this scope authorizes the requested operation for a clone URL.
    #[must_use]
    pub fn allows_clone_url(&self, clone_url: &str, requested: CredentialAccess) -> bool {
        let Ok(coordinate) = parse_clone_url(clone_url) else {
            return false;
        };
        self.host.eq_ignore_ascii_case(&coordinate.host)
            && self.owner.eq_ignore_ascii_case(&coordinate.owner)
            && self.repository.eq_ignore_ascii_case(&coordinate.name)
            && self.access.allows(requested)
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "https://{}/{}/{}:{:?}",
            self.host, self.owner, self.repository, self.access
        )
    }
}

/// Shell-neutral operation emitted by a validated seed plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum SeedOperation {
    /// Fetch one exact commit from the immutable source coordinate.
    FetchExactCommit {
        /// Source clone URL.
        clone_url: String,
        /// Full source commit identifier.
        commit_sha: String,
    },
    /// Push the fetched commit to the session-owned destination ref.
    PushExactCommit {
        /// Destination clone URL.
        clone_url: String,
        /// Full source commit identifier that must be pushed.
        commit_sha: String,
        /// Fully qualified destination ref.
        ref_name: String,
    },
}

/// Validated plan for copying one immutable source revision into one isolated repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedPlan {
    /// Simulation session identifier.
    pub session_id: Uuid,
    /// Immutable source revision.
    pub source: SourceRevision,
    /// Session-owned destination repository.
    pub destination: DestinationRepository,
    /// Optional exact read credential for a private source.
    pub source_scope: Option<GitCredentialScope>,
    /// Exact write credential required for the destination.
    pub destination_scope: GitCredentialScope,
    /// Fully qualified destination ref.
    pub target_ref: String,
}

impl SeedPlan {
    /// Creates a seed plan while enforcing immutable source and least-privilege boundaries.
    pub fn new(
        session_id: Uuid,
        source: SourceRevision,
        destination: DestinationRepository,
        source_scope: Option<GitCredentialScope>,
    ) -> Result<Self, ProvisioningError> {
        if source.repository_id != destination.repository_id {
            return Err(ProvisioningError::RepositoryIdMismatch {
                source: source.repository_id,
                destination: destination.repository_id,
            });
        }

        let source_coordinate = parse_clone_url(&source.clone_url)?;
        let destination_coordinate = destination.coordinate();
        if source_coordinate == destination_coordinate {
            return Err(ProvisioningError::SourceEqualsDestination(
                source_coordinate.to_string(),
            ));
        }

        if let Some(scope) = &source_scope {
            let expected = GitCredentialScope::for_source(&source)?;
            if scope != &expected {
                return Err(ProvisioningError::CredentialScopeMismatch {
                    expected: expected.summary(),
                    actual: scope.summary(),
                });
            }
        }

        let destination_scope =
            GitCredentialScope::for_destination(&destination, CredentialAccess::Write);
        let target_ref = format!("refs/heads/{}", destination.default_branch);
        Ok(Self {
            session_id,
            source,
            destination,
            source_scope,
            destination_scope,
            target_ref,
        })
    }

    /// Returns the two shell-neutral Git operations required to seed the destination.
    #[must_use]
    pub fn operations(&self) -> [SeedOperation; 2] {
        [
            SeedOperation::FetchExactCommit {
                clone_url: self.source.clone_url.clone(),
                commit_sha: self.source.commit_sha.clone(),
            },
            SeedOperation::PushExactCommit {
                clone_url: self.destination.clone_url(),
                commit_sha: self.source.commit_sha.clone(),
                ref_name: self.target_ref.clone(),
            },
        ]
    }

    /// Projects this seed into the exact repository revision accepted by the sandbox runner.
    pub fn verification_revision(
        &self,
        head_commit_sha: impl Into<String>,
    ) -> Result<RepositoryRevision, ProvisioningError> {
        let head_commit_sha = head_commit_sha.into().to_ascii_lowercase();
        validate_full_git_object_id(&head_commit_sha)?;
        Ok(RepositoryRevision {
            repository_id: self.source.repository_id.clone(),
            clone_url: self.destination.clone_url(),
            base_commit_sha: self.source.commit_sha.clone(),
            head_commit_sha,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepositoryCoordinate {
    host: String,
    owner: String,
    name: String,
}

impl std::fmt::Display for RepositoryCoordinate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}/{}", self.host, self.owner, self.name)
    }
}

fn parse_clone_url(value: &str) -> Result<RepositoryCoordinate, ProvisioningError> {
    let parsed = Url::parse(value).map_err(|error| ProvisioningError::InvalidCloneUrl {
        url: value.to_string(),
        reason: error.to_string(),
    })?;
    if parsed.scheme() != "https" {
        return Err(ProvisioningError::InvalidCloneUrl {
            url: value.to_string(),
            reason: "only https clone URLs are accepted".to_string(),
        });
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ProvisioningError::InvalidCloneUrl {
            url: value.to_string(),
            reason: "credentials must not be embedded in clone URLs".to_string(),
        });
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ProvisioningError::InvalidCloneUrl {
            url: value.to_string(),
            reason: "query strings and fragments are not repository identity".to_string(),
        });
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| ProvisioningError::InvalidCloneUrl {
            url: value.to_string(),
            reason: "missing host".to_string(),
        })?
        .to_ascii_lowercase();
    let segments = parsed
        .path_segments()
        .ok_or_else(|| ProvisioningError::InvalidCloneUrl {
            url: value.to_string(),
            reason: "missing repository path".to_string(),
        })?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() != 2 {
        return Err(ProvisioningError::InvalidCloneUrl {
            url: value.to_string(),
            reason: "expected exactly /owner/repository".to_string(),
        });
    }
    let owner = segments[0].to_string();
    let name = segments[1]
        .strip_suffix(".git")
        .unwrap_or(segments[1])
        .to_string();
    validate_host(&host)?;
    validate_github_name("owner", &owner, true)?;
    validate_github_name("repository", &name, false)?;
    Ok(RepositoryCoordinate {
        host,
        owner: owner.to_ascii_lowercase(),
        name: name.to_ascii_lowercase(),
    })
}

fn validate_host(value: &str) -> Result<(), ProvisioningError> {
    let candidate = format!("https://{value}/");
    let valid = Url::parse(&candidate)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .is_some_and(|host| host.eq_ignore_ascii_case(value));
    if valid && !value.contains('/') && !value.contains('@') {
        Ok(())
    } else {
        Err(ProvisioningError::InvalidGitHubName {
            field: "host",
            value: value.to_string(),
        })
    }
}

fn validate_github_name(
    field: &'static str,
    value: &str,
    owner: bool,
) -> Result<(), ProvisioningError> {
    let max_len = if owner { 39 } else { 100 };
    let valid_chars = value.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || character == '-'
            || (!owner && matches!(character, '.' | '_'))
    });
    let valid = !value.is_empty()
        && value.len() <= max_len
        && valid_chars
        && !value.starts_with('-')
        && !value.ends_with('-')
        && (!owner || !value.contains("--"))
        && (!owner || !value.contains('.') && !value.contains('_'));
    if valid {
        Ok(())
    } else {
        Err(ProvisioningError::InvalidGitHubName {
            field,
            value: value.to_string(),
        })
    }
}

fn validate_ref_name(value: &str) -> Result<(), ProvisioningError> {
    let valid = !value.is_empty()
        && value.len() <= 255
        && value != "@"
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.ends_with('.')
        && !value.ends_with(".lock")
        && !value.contains("..")
        && !value.contains("@{")
        && !value.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        });
    if valid {
        Ok(())
    } else {
        Err(ProvisioningError::InvalidGitRef(value.to_string()))
    }
}
