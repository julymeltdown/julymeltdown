//! Trusted scenario manifest schema, validation, and lookup.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};

use buzz_sim_protocol::{
    manifest_digest, CheckPhase, EvidenceVisibility, VerificationRequest,
    VERIFICATION_PROTOCOL_VERSION,
};
use globset::{Glob, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::RunnerError;

/// Maximum repositories referenced by one scenario.
pub const MAX_REPOSITORIES: usize = 8;
/// Maximum command checks in one scenario.
pub const MAX_COMMANDS: usize = 32;
/// Maximum concurrently described services.
pub const MAX_SERVICES: usize = 8;
/// Maximum host-side HTTP probes.
pub const MAX_PROBES: usize = 32;
/// Maximum commands, services, and probes combined.
pub const MAX_TOTAL_CHECKS: usize = 64;
/// Maximum run wall-clock budget.
pub const MAX_WALL_CLOCK_SECONDS: u64 = 1_800;
/// Maximum individual step budget.
pub const MAX_STEP_SECONDS: u64 = 600;
/// Maximum memory assigned to a workload.
pub const MAX_MEMORY_MB: u64 = 8_192;
/// Maximum CPU allocation in thousandths of one CPU.
pub const MAX_CPUS_MILLIS: u64 = 8_000;
/// Maximum number of workload processes.
pub const MAX_PIDS: u64 = 1_024;
/// Maximum captured stdout or stderr bytes per process.
pub const MAX_OUTPUT_BYTES: u64 = 8 * 1_048_576;

/// Immutable toolchain image used for all workload containers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainSpec {
    /// OCI image reference; production requires an `@sha256:` digest.
    pub image: String,
}

/// Hard resource limits applied to one verification run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    /// Overall run budget.
    pub wall_clock_seconds: u64,
    /// Per-command, service, or probe ceiling.
    pub step_seconds: u64,
    /// Container memory limit.
    pub memory_mb: u64,
    /// CPU quota in thousandths of one CPU.
    pub cpus_millis: u64,
    /// Process-count ceiling.
    pub pids: u64,
    /// Captured output ceiling per stream.
    pub output_bytes: u64,
}

/// Repository expected by a trusted scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestRepository {
    /// Stable scenario-local repository identifier.
    pub id: String,
    /// Whether player or NPC commits may change the repository.
    pub writable: bool,
    /// Relative files that must exist at the submitted head revision.
    #[serde(default)]
    pub required_paths: Vec<String>,
}

/// A player-visible command check executed without a shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandCheckSpec {
    /// Stable check identifier.
    pub id: String,
    /// Repository used as the working directory.
    pub repository_id: String,
    /// Objective check phase.
    pub phase: CheckPhase,
    /// Executable and arguments passed directly to the container runtime.
    pub argv: Vec<String>,
    /// Optional environment variables containing no secrets.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Step timeout bounded by resource limits.
    pub timeout_seconds: u64,
    /// Version 1 permits only player-visible commands.
    pub visibility: EvidenceVisibility,
}

/// Trusted service readiness contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessSpec {
    /// Relative HTTP path requested on loopback.
    pub path: String,
    /// Expected readiness status.
    pub expected_status: u16,
    /// Readiness deadline bounded by resource limits.
    pub timeout_seconds: u64,
}

/// Long-running service started for integration verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceSpec {
    /// Stable service and network-alias identifier.
    pub id: String,
    /// Repository used as the working directory.
    pub repository_id: String,
    /// Executable and arguments passed directly to the container runtime.
    pub argv: Vec<String>,
    /// TCP port exposed by the service inside its container.
    pub container_port: u16,
    /// Services that must become ready first.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Trusted readiness contract.
    pub readiness: ReadinessSpec,
}

/// HTTP method supported by version 1 probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeMethod {
    /// HTTP GET.
    #[serde(rename = "GET")]
    Get,
    /// HTTP POST.
    #[serde(rename = "POST")]
    Post,
}

/// JSON Pointer equality assertion for a host-side probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonAssertionSpec {
    /// RFC 6901 JSON Pointer, or an empty string for the root document.
    pub pointer: String,
    /// Exact expected JSON value.
    pub expected: Value,
}

/// Trusted host-side HTTP behavior probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpProbeSpec {
    /// Stable check identifier.
    pub id: String,
    /// Service receiving the loopback request.
    pub service_id: String,
    /// Supported HTTP method.
    pub method: ProbeMethod,
    /// Relative HTTP path; absolute URLs are forbidden.
    pub path: String,
    /// Expected status code.
    pub expected_status: u16,
    /// Optional trusted JSON request body.
    pub body: Option<Value>,
    /// JSON Pointer assertions.
    #[serde(default)]
    pub assertions: Vec<JsonAssertionSpec>,
    /// Probe timeout bounded by resource limits and 30 seconds.
    pub timeout_seconds: u64,
    /// Player-visible or evaluator-only evidence.
    pub visibility: EvidenceVisibility,
}

/// Source-level policy evaluated before any workload executes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySpec {
    /// Glob patterns that reject matching changed paths.
    #[serde(default)]
    pub forbidden_paths: Vec<String>,
    /// Maximum number of changed files across repositories.
    pub max_changed_files: usize,
    /// Maximum added and deleted lines across repositories.
    pub max_changed_lines: u64,
    /// Whether Git submodules are forbidden.
    pub forbid_submodules: bool,
    /// Whether symbolic links are forbidden.
    pub forbid_symlinks: bool,
}

/// Complete trusted verification manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationManifest {
    /// Wire protocol version understood by this manifest.
    pub version: u16,
    /// Stable scenario identifier.
    pub scenario_id: String,
    /// Immutable scenario version.
    pub scenario_version: String,
    /// Immutable workload toolchain.
    pub toolchain: ToolchainSpec,
    /// Hard resource limits.
    pub limits: ResourceLimits,
    /// Expected repository set.
    pub repositories: Vec<ManifestRepository>,
    /// Player-visible command checks.
    #[serde(default)]
    pub commands: Vec<CommandCheckSpec>,
    /// Services used by integration probes.
    #[serde(default)]
    pub services: Vec<ServiceSpec>,
    /// Player-visible and evaluator-only host-side probes.
    #[serde(default)]
    pub probes: Vec<HttpProbeSpec>,
    /// Source-level policy.
    pub policy: PolicySpec,
}

/// Manifest plus trusted origin and precomputed dependency order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedManifest {
    /// Parsed and validated manifest.
    pub manifest: VerificationManifest,
    /// SHA-256 of exact trusted manifest bytes.
    pub digest: String,
    /// Canonical trusted manifest path.
    pub source_path: PathBuf,
    /// Topological service startup order.
    pub service_order: Vec<String>,
}

impl VerificationManifest {
    /// Parses a YAML manifest while rejecting unknown fields.
    pub fn from_yaml(yaml: &str) -> Result<Self, RunnerError> {
        Ok(serde_yaml::from_str(yaml)?)
    }

    /// Validates hard limits, references, IDs, image identity, and service topology.
    pub fn validate(&self, require_immutable_images: bool) -> Result<Vec<String>, RunnerError> {
        if self.version != VERIFICATION_PROTOCOL_VERSION {
            return Err(manifest_error(format!(
                "unsupported manifest version {}",
                self.version
            )));
        }
        validate_slug("scenario_id", &self.scenario_id)?;
        validate_version("scenario_version", &self.scenario_version)?;
        validate_limits(&self.limits)?;
        if require_immutable_images && !is_immutable_image(&self.toolchain.image) {
            return Err(manifest_error(
                "toolchain image must use an immutable @sha256 digest",
            ));
        }
        if self.repositories.is_empty() || self.repositories.len() > MAX_REPOSITORIES {
            return Err(manifest_error(format!(
                "repositories must contain 1..={MAX_REPOSITORIES} entries"
            )));
        }
        if self.commands.len() > MAX_COMMANDS {
            return Err(manifest_error(format!(
                "commands exceed maximum {MAX_COMMANDS}"
            )));
        }
        if self.services.len() > MAX_SERVICES {
            return Err(manifest_error(format!(
                "services exceed maximum {MAX_SERVICES}"
            )));
        }
        if self.probes.len() > MAX_PROBES {
            return Err(manifest_error(format!(
                "probes exceed maximum {MAX_PROBES}"
            )));
        }
        let total = self.commands.len() + self.services.len() + self.probes.len();
        if total > MAX_TOTAL_CHECKS {
            return Err(manifest_error(format!(
                "total checks exceed maximum {MAX_TOTAL_CHECKS}"
            )));
        }

        let repository_ids = validate_repositories(&self.repositories)?;
        let mut check_ids = BTreeSet::new();
        validate_commands(self, &repository_ids, &mut check_ids)?;
        let service_ids = validate_services(self, &repository_ids, &mut check_ids)?;
        validate_probes(self, &service_ids, &mut check_ids)?;
        validate_policy(&self.policy)?;
        topological_service_order(&self.services)
    }
}

/// Immutable registry rooted at trusted operator-owned scenario files.
#[derive(Debug, Clone)]
pub struct ScenarioRegistry {
    root: PathBuf,
    require_immutable_images: bool,
}

impl ScenarioRegistry {
    /// Opens a canonical existing scenario root.
    pub fn new(
        root: impl AsRef<Path>,
        require_immutable_images: bool,
    ) -> Result<Self, RunnerError> {
        let root = fs::canonicalize(root.as_ref())
            .map_err(|error| RunnerError::io("canonicalizing scenario root", error))?;
        if !root.is_dir() {
            return Err(RunnerError::Config(
                "scenario root must be a directory".into(),
            ));
        }
        Ok(Self {
            root,
            require_immutable_images,
        })
    }

    /// Loads `<root>/<scenario>/<version>/verification.yaml` and verifies exact bytes.
    pub fn load(&self, request: &VerificationRequest) -> Result<ValidatedManifest, RunnerError> {
        validate_slug("scenario_id", &request.scenario_id)?;
        validate_version("scenario_version", &request.scenario_version)?;
        let path = self
            .root
            .join(&request.scenario_id)
            .join(&request.scenario_version)
            .join("verification.yaml");
        let path = fs::canonicalize(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                RunnerError::NotFound(format!(
                    "scenario {}/{}",
                    request.scenario_id, request.scenario_version
                ))
            } else {
                RunnerError::io("canonicalizing scenario manifest", error)
            }
        })?;
        if !path.starts_with(&self.root) {
            return Err(manifest_error("scenario manifest escaped trusted root"));
        }
        let bytes =
            fs::read(&path).map_err(|error| RunnerError::io("reading scenario manifest", error))?;
        let actual = manifest_digest(&bytes);
        if actual != request.expected_manifest_digest {
            return Err(RunnerError::ManifestDigestMismatch {
                expected: request.expected_manifest_digest.clone(),
                actual,
            });
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| manifest_error("scenario manifest must be UTF-8"))?;
        let manifest = VerificationManifest::from_yaml(text)?;
        if manifest.scenario_id != request.scenario_id
            || manifest.scenario_version != request.scenario_version
        {
            return Err(manifest_error(
                "manifest identity does not match the verification request",
            ));
        }
        let service_order = manifest.validate(self.require_immutable_images)?;
        Ok(ValidatedManifest {
            manifest,
            digest: request.expected_manifest_digest.clone(),
            source_path: path,
            service_order,
        })
    }

    /// Returns the canonical trusted scenario root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn manifest_error(message: impl Into<String>) -> RunnerError {
    RunnerError::Manifest(message.into())
}

fn validate_limits(limits: &ResourceLimits) -> Result<(), RunnerError> {
    validate_limit(
        "wall_clock_seconds",
        limits.wall_clock_seconds,
        MAX_WALL_CLOCK_SECONDS,
    )?;
    validate_limit("step_seconds", limits.step_seconds, MAX_STEP_SECONDS)?;
    if limits.step_seconds > limits.wall_clock_seconds {
        return Err(manifest_error(
            "step_seconds cannot exceed wall_clock_seconds",
        ));
    }
    validate_limit("memory_mb", limits.memory_mb, MAX_MEMORY_MB)?;
    validate_limit("cpus_millis", limits.cpus_millis, MAX_CPUS_MILLIS)?;
    validate_limit("pids", limits.pids, MAX_PIDS)?;
    validate_limit("output_bytes", limits.output_bytes, MAX_OUTPUT_BYTES)
}

fn validate_limit(name: &str, value: u64, maximum: u64) -> Result<(), RunnerError> {
    if value == 0 || value > maximum {
        return Err(manifest_error(format!(
            "{name} must be within 1..={maximum}"
        )));
    }
    Ok(())
}

fn validate_repositories(
    repositories: &[ManifestRepository],
) -> Result<BTreeSet<String>, RunnerError> {
    let mut ids = BTreeSet::new();
    for repository in repositories {
        validate_slug("repository id", &repository.id)?;
        if !ids.insert(repository.id.clone()) {
            return Err(manifest_error(format!(
                "duplicate repository id {}",
                repository.id
            )));
        }
        for path in &repository.required_paths {
            validate_relative_path("required path", path)?;
        }
    }
    Ok(ids)
}

fn validate_commands(
    manifest: &VerificationManifest,
    repository_ids: &BTreeSet<String>,
    check_ids: &mut BTreeSet<String>,
) -> Result<(), RunnerError> {
    for command in &manifest.commands {
        validate_check_id(&command.id, check_ids)?;
        require_repository(&command.repository_id, repository_ids)?;
        validate_argv(&command.argv)?;
        validate_step_timeout(command.timeout_seconds, &manifest.limits)?;
        if command.visibility != EvidenceVisibility::Player {
            return Err(manifest_error(
                "hidden checks must be host-side probes in protocol version 1",
            ));
        }
        for (key, value) in &command.env {
            if key.is_empty()
                || !key.chars().all(|character| {
                    character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
                })
                || value.contains('\0')
            {
                return Err(manifest_error(format!(
                    "invalid command environment entry {key:?}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_services(
    manifest: &VerificationManifest,
    repository_ids: &BTreeSet<String>,
    check_ids: &mut BTreeSet<String>,
) -> Result<BTreeSet<String>, RunnerError> {
    let mut ids = BTreeSet::new();
    let mut ports = BTreeSet::new();
    for service in &manifest.services {
        validate_slug("service id", &service.id)?;
        validate_check_id(&service.id, check_ids)?;
        if !ids.insert(service.id.clone()) {
            return Err(manifest_error(format!(
                "duplicate service id {}",
                service.id
            )));
        }
        require_repository(&service.repository_id, repository_ids)?;
        validate_argv(&service.argv)?;
        if service.container_port == 0 || !ports.insert(service.container_port) {
            return Err(manifest_error(format!(
                "service port {} is zero or duplicated",
                service.container_port
            )));
        }
        validate_http_path("readiness path", &service.readiness.path)?;
        validate_status(service.readiness.expected_status)?;
        validate_step_timeout(service.readiness.timeout_seconds, &manifest.limits)?;
    }
    for service in &manifest.services {
        for dependency in &service.depends_on {
            if dependency == &service.id || !ids.contains(dependency) {
                return Err(manifest_error(format!(
                    "service {} has invalid dependency {}",
                    service.id, dependency
                )));
            }
        }
    }
    Ok(ids)
}

fn validate_probes(
    manifest: &VerificationManifest,
    service_ids: &BTreeSet<String>,
    check_ids: &mut BTreeSet<String>,
) -> Result<(), RunnerError> {
    for probe in &manifest.probes {
        validate_check_id(&probe.id, check_ids)?;
        if !service_ids.contains(&probe.service_id) {
            return Err(manifest_error(format!(
                "probe {} references unknown service {}",
                probe.id, probe.service_id
            )));
        }
        validate_http_path("probe path", &probe.path)?;
        validate_status(probe.expected_status)?;
        validate_step_timeout(probe.timeout_seconds, &manifest.limits)?;
        for assertion in &probe.assertions {
            if !assertion.pointer.is_empty() && !assertion.pointer.starts_with('/') {
                return Err(manifest_error(format!(
                    "probe {} has invalid JSON Pointer {}",
                    probe.id, assertion.pointer
                )));
            }
        }
    }
    Ok(())
}

fn validate_policy(policy: &PolicySpec) -> Result<(), RunnerError> {
    if policy.max_changed_files == 0 || policy.max_changed_files > 100_000 {
        return Err(manifest_error(
            "max_changed_files must be within 1..=100000",
        ));
    }
    if policy.max_changed_lines == 0 || policy.max_changed_lines > 10_000_000 {
        return Err(manifest_error(
            "max_changed_lines must be within 1..=10000000",
        ));
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in &policy.forbidden_paths {
        if pattern.starts_with('/') || pattern.contains("..") {
            return Err(manifest_error(format!(
                "forbidden path glob must be relative: {pattern}"
            )));
        }
        builder
            .add(Glob::new(pattern).map_err(|error| {
                manifest_error(format!("invalid forbidden path glob: {error}"))
            })?);
    }
    builder
        .build()
        .map_err(|error| manifest_error(format!("invalid forbidden path glob set: {error}")))?;
    Ok(())
}

fn topological_service_order(services: &[ServiceSpec]) -> Result<Vec<String>, RunnerError> {
    let mut incoming = BTreeMap::<String, usize>::new();
    let mut dependants = BTreeMap::<String, Vec<String>>::new();
    for service in services {
        incoming.insert(service.id.clone(), service.depends_on.len());
        for dependency in &service.depends_on {
            dependants
                .entry(dependency.clone())
                .or_default()
                .push(service.id.clone());
        }
    }
    let mut ready = incoming
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(services.len());
    while let Some(id) = ready.pop_front() {
        order.push(id.clone());
        if let Some(next_services) = dependants.get(&id) {
            let mut next_services = next_services.clone();
            next_services.sort();
            for next in next_services {
                if let Some(count) = incoming.get_mut(&next) {
                    *count -= 1;
                    if *count == 0 {
                        ready.push_back(next);
                    }
                }
            }
        }
    }
    if order.len() != services.len() {
        return Err(manifest_error("service dependency graph contains a cycle"));
    }
    Ok(order)
}

fn validate_check_id(id: &str, ids: &mut BTreeSet<String>) -> Result<(), RunnerError> {
    validate_slug("check id", id)?;
    if !ids.insert(id.to_owned()) {
        return Err(manifest_error(format!("duplicate check id {id}")));
    }
    Ok(())
}

fn require_repository(id: &str, ids: &BTreeSet<String>) -> Result<(), RunnerError> {
    if !ids.contains(id) {
        return Err(manifest_error(format!("unknown repository {id}")));
    }
    Ok(())
}

fn validate_argv(argv: &[String]) -> Result<(), RunnerError> {
    if argv.is_empty()
        || argv
            .iter()
            .any(|argument| argument.is_empty() || argument.contains('\0'))
    {
        return Err(manifest_error(
            "command argv must contain non-empty NUL-free arguments",
        ));
    }
    Ok(())
}

fn validate_step_timeout(timeout: u64, limits: &ResourceLimits) -> Result<(), RunnerError> {
    if timeout == 0 || timeout > limits.step_seconds || timeout > MAX_STEP_SECONDS {
        return Err(manifest_error(format!(
            "step timeout must be within 1..={} seconds",
            limits.step_seconds
        )));
    }
    Ok(())
}

fn validate_http_path(field: &str, path: &str) -> Result<(), RunnerError> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains("://")
        || path.contains('#')
        || path.chars().any(char::is_control)
    {
        return Err(manifest_error(format!(
            "{field} must be a safe relative HTTP path"
        )));
    }
    Ok(())
}

fn validate_status(status: u16) -> Result<(), RunnerError> {
    if !(100..=599).contains(&status) {
        return Err(manifest_error(format!(
            "HTTP status {status} is outside 100..=599"
        )));
    }
    Ok(())
}

fn validate_relative_path(field: &str, raw: &str) -> Result<(), RunnerError> {
    let path = Path::new(raw);
    if raw.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(manifest_error(format!(
            "{field} must be a clean relative path: {raw:?}"
        )));
    }
    Ok(())
}

fn validate_slug(field: &str, value: &str) -> Result<(), RunnerError> {
    let mut chars = value.chars();
    let valid_first = chars
        .next()
        .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit());
    if value.len() > 64
        || !valid_first
        || !chars.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '_'
        })
    {
        return Err(manifest_error(format!(
            "{field} must match [a-z0-9][a-z0-9_-]{{0,63}}"
        )));
    }
    Ok(())
}

fn validate_version(field: &str, value: &str) -> Result<(), RunnerError> {
    if value.is_empty()
        || value.len() > 64
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character == '.'
                || character == '-'
                || character == '_'
        })
    {
        return Err(manifest_error(format!(
            "{field} contains unsupported characters"
        )));
    }
    Ok(())
}

fn is_immutable_image(image: &str) -> bool {
    image.rsplit_once("@sha256:").is_some_and(|(name, digest)| {
        !name.is_empty()
            && digest.len() == 64
            && digest
                .chars()
                .all(|character| character.is_ascii_hexdigit())
    })
}
