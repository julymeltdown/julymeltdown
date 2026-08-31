use std::{fmt, io::Write, path::Path};

use async_trait::async_trait;
use tempfile::{Builder, TempDir};
use tokio::process::Command;

use crate::{GitCredentialScope, GitHubApiError, RepositorySeeder, SeedPlan, SeededRepository};

/// Lifecycle phase of one concrete Git command used while seeding a session repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitCommandPhase {
    /// Create the temporary local repository.
    Initialize,
    /// Register the immutable source repository.
    AddSourceRemote,
    /// Fetch one exact source commit.
    FetchExactCommit,
    /// Resolve and verify the fetched commit object.
    ResolveFetchedCommit,
    /// Register the session-owned destination repository.
    AddDestinationRemote,
    /// Push the exact commit into the protected session branch.
    PushExactCommit,
    /// Non-mutating command used by health checks and tests.
    Probe,
}

impl GitCommandPhase {
    const fn operation(self) -> &'static str {
        match self {
            Self::Initialize => "git_initialize",
            Self::AddSourceRemote => "git_add_source_remote",
            Self::FetchExactCommit => "git_fetch_exact_commit",
            Self::ResolveFetchedCommit => "git_resolve_fetched_commit",
            Self::AddDestinationRemote => "git_add_destination_remote",
            Self::PushExactCommit => "git_push_exact_commit",
            Self::Probe => "git_probe",
        }
    }
}

/// One shell-free Git invocation. Arguments never contain credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommand {
    /// Stable phase used for diagnostics and tests.
    pub phase: GitCommandPhase,
    /// Arguments passed directly to the `git` executable.
    pub arguments: Vec<String>,
}

impl GitCommand {
    /// Creates a command from an iterator without invoking a shell.
    #[must_use]
    pub fn new<I, S>(phase: GitCommandPhase, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            phase,
            arguments: arguments.into_iter().map(Into::into).collect(),
        }
    }
}

/// Captured successful Git process output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommandOutput {
    /// UTF-8-lossy standard output.
    pub stdout: String,
    /// UTF-8-lossy standard error.
    pub stderr: String,
}

/// Short-lived username and secret used only by one exact repository operation.
pub struct GitCredentialLease {
    username: String,
    secret: String,
}

impl GitCredentialLease {
    /// Creates a credential lease while rejecting values unsafe for environment transport.
    pub fn new(
        username: impl Into<String>,
        secret: impl Into<String>,
    ) -> Result<Self, GitHubApiError> {
        let username = username.into();
        let secret = secret.into();
        let valid = |value: &str| {
            !value.is_empty()
                && !value.contains('\0')
                && !value.contains('\n')
                && !value.contains('\r')
        };
        if !valid(&username) || !valid(&secret) {
            return Err(GitHubApiError::InvalidResponse {
                operation: "git_credential",
                reason: "credential values must be non-empty single-line strings".to_string(),
            });
        }
        Ok(Self { username, secret })
    }

    /// Username supplied through the askpass boundary.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }

    fn secret(&self) -> &str {
        &self.secret
    }
}

impl fmt::Debug for GitCredentialLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitCredentialLease")
            .field("username", &self.username)
            .field("secret", &"REDACTED")
            .finish()
    }
}

/// Issues a short-lived credential for one already-validated repository scope.
#[async_trait]
pub trait GitCredentialProvider: Send + Sync {
    /// Resolves the credential for the exact host, owner, repository, and access mode.
    async fn credential_for(
        &self,
        scope: &GitCredentialScope,
    ) -> Result<GitCredentialLease, GitHubApiError>;
}

/// Executes shell-free Git commands with an optional askpass credential.
#[async_trait]
pub trait GitCommandExecutor: Send + Sync {
    /// Runs one command in the supplied temporary repository.
    async fn execute(
        &self,
        working_directory: &Path,
        command: &GitCommand,
        credential: Option<&GitCredentialLease>,
    ) -> Result<GitCommandOutput, GitHubApiError>;
}

/// Tokio process implementation of [`GitCommandExecutor`].
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioGitCommandExecutor;

#[async_trait]
impl GitCommandExecutor for TokioGitCommandExecutor {
    async fn execute(
        &self,
        working_directory: &Path,
        command: &GitCommand,
        credential: Option<&GitCredentialLease>,
    ) -> Result<GitCommandOutput, GitHubApiError> {
        let mut process = Command::new("git");
        process
            .args(&command.arguments)
            .current_dir(working_directory)
            .env("GIT_TERMINAL_PROMPT", "0");

        let askpass = match credential {
            Some(credential) => {
                let askpass = create_askpass(working_directory)?;
                process
                    .env("GIT_ASKPASS", askpass.path())
                    .env("GIT_ASKPASS_REQUIRE", "force")
                    .env("BUZZ_GIT_USERNAME", credential.username())
                    .env("BUZZ_GIT_PASSWORD", credential.secret());
                Some(askpass)
            }
            None => None,
        };

        let output = process
            .output()
            .await
            .map_err(|error| GitHubApiError::Transport(error.to_string()))?;
        drop(askpass);

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !output.status.success() {
            return Err(GitHubApiError::InvalidResponse {
                operation: command.phase.operation(),
                reason: bounded_process_failure(output.status.code(), &stderr),
            });
        }

        Ok(GitCommandOutput { stdout, stderr })
    }
}

fn create_askpass(working_directory: &Path) -> Result<tempfile::NamedTempFile, GitHubApiError> {
    let suffix = if cfg!(windows) { ".cmd" } else { ".sh" };
    let mut file = Builder::new()
        .prefix(".buzz-git-askpass-")
        .suffix(suffix)
        .tempfile_in(working_directory)
        .map_err(|error| GitHubApiError::Transport(error.to_string()))?;

    #[cfg(windows)]
    file.write_all(b"@echo off\r\necho %BUZZ_GIT_PASSWORD%\r\n")
        .map_err(|error| GitHubApiError::Transport(error.to_string()))?;

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        file.write_all(
            b"#!/bin/sh\ncase \"$1\" in\n  *Username*) printf '%s\\n' \"$BUZZ_GIT_USERNAME\" ;;\n  *) printf '%s\\n' \"$BUZZ_GIT_PASSWORD\" ;;\nesac\n",
        )
        .map_err(|error| GitHubApiError::Transport(error.to_string()))?;
        let mut permissions = file
            .as_file()
            .metadata()
            .map_err(|error| GitHubApiError::Transport(error.to_string()))?
            .permissions();
        permissions.set_mode(0o700);
        file.as_file()
            .set_permissions(permissions)
            .map_err(|error| GitHubApiError::Transport(error.to_string()))?;
    }

    file.flush()
        .map_err(|error| GitHubApiError::Transport(error.to_string()))?;
    Ok(file)
}

fn bounded_process_failure(code: Option<i32>, stderr: &str) -> String {
    const MAX_BYTES: usize = 4 * 1024;
    let mut bytes = stderr.as_bytes();
    if bytes.len() > MAX_BYTES {
        bytes = &bytes[..MAX_BYTES];
    }
    format!(
        "git exited with status {:?}: {}",
        code,
        String::from_utf8_lossy(bytes).trim()
    )
}

/// Git-based implementation that copies one exact commit into a session repository.
#[derive(Debug, Clone)]
pub struct GitCliSeeder<P, E> {
    credentials: P,
    executor: E,
}

impl<P, E> GitCliSeeder<P, E> {
    /// Creates a seeder from independent credential and process boundaries.
    #[must_use]
    pub const fn new(credentials: P, executor: E) -> Self {
        Self {
            credentials,
            executor,
        }
    }
}

#[async_trait]
impl<P, E> RepositorySeeder for GitCliSeeder<P, E>
where
    P: GitCredentialProvider,
    E: GitCommandExecutor,
{
    async fn seed_repository(&self, plan: &SeedPlan) -> Result<SeededRepository, GitHubApiError> {
        let workspace =
            TempDir::new().map_err(|error| GitHubApiError::Transport(error.to_string()))?;

        self.run(
            workspace.path(),
            GitCommand::new(GitCommandPhase::Initialize, ["init", "--quiet"]),
            None,
        )
        .await?;
        self.run(
            workspace.path(),
            GitCommand::new(
                GitCommandPhase::AddSourceRemote,
                ["remote", "add", "source", plan.source.clone_url.as_str()],
            ),
            None,
        )
        .await?;

        let source_credential = match &plan.source_scope {
            Some(scope) => Some(self.credentials.credential_for(scope).await?),
            None => None,
        };
        self.run(
            workspace.path(),
            GitCommand::new(
                GitCommandPhase::FetchExactCommit,
                [
                    "fetch",
                    "--no-tags",
                    "--depth=1",
                    "source",
                    plan.source.commit_sha.as_str(),
                ],
            ),
            source_credential.as_ref(),
        )
        .await?;

        let resolved = self
            .run(
                workspace.path(),
                GitCommand::new(
                    GitCommandPhase::ResolveFetchedCommit,
                    ["rev-parse", "--verify", "FETCH_HEAD^{commit}"],
                ),
                None,
            )
            .await?
            .stdout
            .trim()
            .to_ascii_lowercase();
        if resolved != plan.source.commit_sha {
            return Err(GitHubApiError::InvalidResponse {
                operation: "seed_repository",
                reason: format!(
                    "fetched commit {resolved:?} does not match requested commit {:?}",
                    plan.source.commit_sha
                ),
            });
        }

        let destination_credential = self
            .credentials
            .credential_for(&plan.destination_scope)
            .await?;
        self.run(
            workspace.path(),
            GitCommand::new(
                GitCommandPhase::AddDestinationRemote,
                [
                    "remote",
                    "add",
                    "destination",
                    plan.destination.clone_url().as_str(),
                ],
            ),
            None,
        )
        .await?;
        let force_with_lease = format!("--force-with-lease={}:", plan.target_ref);
        let refspec = format!("{}:{}", plan.source.commit_sha, plan.target_ref);
        self.run(
            workspace.path(),
            GitCommand::new(
                GitCommandPhase::PushExactCommit,
                [
                    "push",
                    force_with_lease.as_str(),
                    "destination",
                    refspec.as_str(),
                ],
            ),
            Some(&destination_credential),
        )
        .await?;

        Ok(SeededRepository {
            repository_id: plan.source.repository_id.clone(),
            head_commit_sha: resolved,
        })
    }
}

impl<P, E> GitCliSeeder<P, E>
where
    E: GitCommandExecutor,
{
    async fn run(
        &self,
        directory: &Path,
        command: GitCommand,
        credential: Option<&GitCredentialLease>,
    ) -> Result<GitCommandOutput, GitHubApiError> {
        self.executor.execute(directory, &command, credential).await
    }
}
