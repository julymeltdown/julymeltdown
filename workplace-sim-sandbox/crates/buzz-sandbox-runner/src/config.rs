//! Process-level configuration and secret handling.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::RunnerError;

/// Secret bearer token used by trusted runner clients.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiToken(String);

impl ApiToken {
    /// Constructs and validates a 32-512 byte token.
    pub fn new(value: String) -> Result<Self, RunnerError> {
        if !(32..=512).contains(&value.len()) || value.chars().any(char::is_control) {
            return Err(RunnerError::Config(
                "BUZZ_SANDBOX_API_TOKEN must contain 32-512 non-control bytes".into(),
            ));
        }
        Ok(Self(value))
    }

    /// Returns token bytes for constant-time comparison.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for ApiToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiToken([REDACTED])")
    }
}

/// Validated process-level runner configuration.
#[derive(Clone)]
pub struct RunnerConfig {
    /// Internal API bearer token.
    pub api_token: ApiToken,
    /// Canonical trusted scenario root.
    pub scenario_root: PathBuf,
    /// Canonical root for disposable runs and durable evidence.
    pub run_root: PathBuf,
    /// HTTP bind address.
    pub bind_addr: SocketAddr,
    /// Docker-compatible CLI executable.
    pub docker_bin: PathBuf,
    /// Git executable.
    pub git_bin: PathBuf,
    /// Maximum runs executing concurrently.
    pub max_concurrent_runs: usize,
    /// Allowlisted lowercase HTTPS Git hosts.
    pub allowed_git_hosts: BTreeSet<String>,
    /// Whether operator-controlled file URLs are accepted.
    pub allow_file_urls: bool,
    /// Canonical root containing permitted local fixture repositories.
    pub file_source_root: Option<PathBuf>,
    /// Whether toolchain references require immutable digests.
    pub require_immutable_images: bool,
    /// Whether a non-loopback HTTP bind is explicitly allowed.
    pub allow_remote_bind: bool,
}

impl fmt::Debug for RunnerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunnerConfig")
            .field("api_token", &self.api_token)
            .field("scenario_root", &self.scenario_root)
            .field("run_root", &self.run_root)
            .field("bind_addr", &self.bind_addr)
            .field("docker_bin", &self.docker_bin)
            .field("git_bin", &self.git_bin)
            .field("max_concurrent_runs", &self.max_concurrent_runs)
            .field("allowed_git_hosts", &self.allowed_git_hosts)
            .field("allow_file_urls", &self.allow_file_urls)
            .field("file_source_root", &self.file_source_root)
            .field("require_immutable_images", &self.require_immutable_images)
            .field("allow_remote_bind", &self.allow_remote_bind)
            .finish()
    }
}

impl RunnerConfig {
    /// Parses configuration from the current process environment.
    pub fn from_env() -> Result<Self, RunnerError> {
        Self::from_map(std::env::vars().collect())
    }

    /// Parses configuration from a deterministic key-value map.
    pub fn from_map(values: BTreeMap<String, String>) -> Result<Self, RunnerError> {
        let api_token = ApiToken::new(required(&values, "BUZZ_SANDBOX_API_TOKEN")?.to_owned())?;
        let scenario_root = canonical_existing_directory(
            Path::new(required(&values, "BUZZ_SANDBOX_SCENARIO_ROOT")?),
            "scenario root",
        )?;
        let run_root_raw = PathBuf::from(required(&values, "BUZZ_SANDBOX_RUN_ROOT")?);
        fs::create_dir_all(&run_root_raw)
            .map_err(|error| RunnerError::io("creating run root", error))?;
        let run_root = canonical_existing_directory(&run_root_raw, "run root")?;

        let bind_addr = value_or(&values, "BUZZ_SANDBOX_BIND_ADDR", "127.0.0.1:8787")
            .parse::<SocketAddr>()
            .map_err(|error| RunnerError::Config(format!("invalid bind address: {error}")))?;
        let allow_remote_bind = parse_bool(
            value_or(&values, "BUZZ_SANDBOX_ALLOW_REMOTE_BIND", "false"),
            "BUZZ_SANDBOX_ALLOW_REMOTE_BIND",
        )?;
        if !bind_addr.ip().is_loopback() && !allow_remote_bind {
            return Err(RunnerError::Config(
                "non-loopback bind requires BUZZ_SANDBOX_ALLOW_REMOTE_BIND=true".into(),
            ));
        }

        let max_concurrent_runs = value_or(&values, "BUZZ_SANDBOX_MAX_CONCURRENT_RUNS", "2")
            .parse::<usize>()
            .map_err(|error| RunnerError::Config(format!("invalid concurrency: {error}")))?;
        if !(1..=64).contains(&max_concurrent_runs) {
            return Err(RunnerError::Config(
                "BUZZ_SANDBOX_MAX_CONCURRENT_RUNS must be within 1..=64".into(),
            ));
        }

        let allowed_git_hosts = value_or(&values, "BUZZ_SANDBOX_ALLOWED_GIT_HOSTS", "github.com")
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<BTreeSet<_>>();
        if allowed_git_hosts.is_empty()
            || allowed_git_hosts.iter().any(|host| {
                host.contains('/') || host.contains(':') || host.chars().any(char::is_whitespace)
            })
        {
            return Err(RunnerError::Config(
                "BUZZ_SANDBOX_ALLOWED_GIT_HOSTS must list plain hostnames".into(),
            ));
        }

        let allow_file_urls = parse_bool(
            value_or(&values, "BUZZ_SANDBOX_ALLOW_FILE_URLS", "false"),
            "BUZZ_SANDBOX_ALLOW_FILE_URLS",
        )?;
        let file_source_root = match values.get("BUZZ_SANDBOX_FILE_SOURCE_ROOT") {
            Some(value) if !value.is_empty() => Some(canonical_existing_directory(
                Path::new(value),
                "file source root",
            )?),
            _ => None,
        };
        if allow_file_urls && file_source_root.is_none() {
            return Err(RunnerError::Config(
                "file URLs require BUZZ_SANDBOX_FILE_SOURCE_ROOT".into(),
            ));
        }

        Ok(Self {
            api_token,
            scenario_root,
            run_root,
            bind_addr,
            docker_bin: PathBuf::from(value_or(&values, "BUZZ_SANDBOX_DOCKER_BIN", "docker")),
            git_bin: PathBuf::from(value_or(&values, "BUZZ_SANDBOX_GIT_BIN", "git")),
            max_concurrent_runs,
            allowed_git_hosts,
            allow_file_urls,
            file_source_root,
            require_immutable_images: parse_bool(
                value_or(&values, "BUZZ_SANDBOX_REQUIRE_IMMUTABLE_IMAGES", "true"),
                "BUZZ_SANDBOX_REQUIRE_IMMUTABLE_IMAGES",
            )?,
            allow_remote_bind,
        })
    }
}

fn required<'a>(values: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str, RunnerError> {
    values
        .get(key)
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .ok_or_else(|| RunnerError::Config(format!("missing required environment variable {key}")))
}

fn value_or<'a>(values: &'a BTreeMap<String, String>, key: &str, default: &'a str) -> &'a str {
    values.get(key).map(String::as_str).unwrap_or(default)
}

fn parse_bool(value: &str, key: &str) -> Result<bool, RunnerError> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(RunnerError::Config(format!(
            "{key} must be true, false, 1, or 0"
        ))),
    }
}

fn canonical_existing_directory(path: &Path, label: &str) -> Result<PathBuf, RunnerError> {
    let path = fs::canonicalize(path)
        .map_err(|error| RunnerError::io(format!("canonicalizing {label}"), error))?;
    if !path.is_dir() {
        return Err(RunnerError::Config(format!("{label} must be a directory")));
    }
    Ok(path)
}
