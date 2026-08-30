use std::collections::BTreeMap;
use std::fs;

use buzz_sandbox_runner::{
    RunnerConfig, RunnerError, ScenarioRegistry, VerificationManifest, MAX_TOTAL_CHECKS,
};
use buzz_sim_protocol::{manifest_digest, RepositoryRevision, VerificationRequest};
use tempfile::tempdir;
use uuid::Uuid;

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn valid_yaml() -> String {
    format!(
        r#"version: 1
scenario_id: coupon-tax-inclusive
scenario_version: 1.0.0
toolchain:
  image: ghcr.io/example/sandbox-toolchain@sha256:{DIGEST}
limits:
  wall_clock_seconds: 300
  step_seconds: 60
  memory_mb: 512
  cpus_millis: 1000
  pids: 128
  output_bytes: 1048576
repositories:
  - id: pricing-api
    writable: true
    required_paths:
      - Cargo.lock
  - id: checkout-api
    writable: false
    required_paths:
      - go.sum
commands:
  - id: build-pricing
    repository_id: pricing-api
    phase: build
    argv: [cargo, build, --locked]
    env:
      RUST_BACKTRACE: "1"
    timeout_seconds: 60
    visibility: player
services:
  - id: pricing
    repository_id: pricing-api
    argv: [./target/debug/pricing-api]
    container_port: 8080
    depends_on: []
    readiness:
      path: /health
      expected_status: 200
      timeout_seconds: 20
  - id: checkout
    repository_id: checkout-api
    argv: [./checkout-api]
    container_port: 8081
    depends_on: [pricing]
    readiness:
      path: /health
      expected_status: 200
      timeout_seconds: 20
probes:
  - id: public-coupon
    service_id: checkout
    method: POST
    path: /v1/coupon
    expected_status: 200
    body:
      country: KR
    assertions:
      - pointer: /discount_total
        expected: 1000
    timeout_seconds: 10
    visibility: player
  - id: hidden-tax-contract
    service_id: checkout
    method: GET
    path: /v1/internal/tax-check
    expected_status: 200
    body: null
    assertions:
      - pointer: /compatible
        expected: true
    timeout_seconds: 10
    visibility: evaluator_only
policy:
  forbidden_paths:
    - .github/**
    - secrets/**
  max_changed_files: 100
  max_changed_lines: 5000
  forbid_submodules: true
  forbid_symlinks: true
"#
    )
}

fn parsed() -> VerificationManifest {
    VerificationManifest::from_yaml(&valid_yaml()).unwrap()
}

#[test]
fn valid_manifest_produces_deterministic_service_order() {
    let manifest = parsed();
    let order = manifest.validate(true).unwrap();
    assert_eq!(order, vec!["pricing", "checkout"]);
}

#[test]
fn unknown_yaml_key_is_rejected() {
    let yaml = valid_yaml().replace("version: 1", "version: 1\nunknown: true");
    let error = VerificationManifest::from_yaml(&yaml).unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn hidden_command_checks_are_rejected_in_v1() {
    let yaml = valid_yaml().replacen("visibility: player", "visibility: evaluator_only", 1);
    let error = VerificationManifest::from_yaml(&yaml)
        .unwrap()
        .validate(true)
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("hidden checks must be host-side probes"));
}

#[test]
fn mutable_image_is_rejected_when_required() {
    let yaml = valid_yaml().replace(
        &format!("ghcr.io/example/sandbox-toolchain@sha256:{DIGEST}"),
        "ghcr.io/example/sandbox-toolchain:latest",
    );
    let error = VerificationManifest::from_yaml(&yaml)
        .unwrap()
        .validate(true)
        .unwrap_err();
    assert!(error.to_string().contains("immutable @sha256"));
}

#[test]
fn invalid_repository_id_is_rejected() {
    let yaml = valid_yaml().replace("id: pricing-api", "id: ../pricing-api");
    let error = VerificationManifest::from_yaml(&yaml)
        .unwrap()
        .validate(true)
        .unwrap_err();
    assert!(error.to_string().contains("repository id"));
}

#[test]
fn duplicate_check_ids_are_rejected_across_check_types() {
    let yaml = valid_yaml().replace("id: public-coupon", "id: build-pricing");
    let error = VerificationManifest::from_yaml(&yaml)
        .unwrap()
        .validate(true)
        .unwrap_err();
    assert!(error.to_string().contains("duplicate check id"));
}

#[test]
fn missing_repository_reference_is_rejected() {
    let yaml = valid_yaml().replace(
        "repository_id: pricing-api\n    phase: build",
        "repository_id: missing-api\n    phase: build",
    );
    let error = VerificationManifest::from_yaml(&yaml)
        .unwrap()
        .validate(true)
        .unwrap_err();
    assert!(error.to_string().contains("unknown repository"));
}

#[test]
fn service_dependency_cycles_are_rejected() {
    let yaml = valid_yaml().replace("depends_on: []", "depends_on: [checkout]");
    let error = VerificationManifest::from_yaml(&yaml)
        .unwrap()
        .validate(true)
        .unwrap_err();
    assert!(error.to_string().contains("contains a cycle"));
}

#[test]
fn duplicate_service_ports_are_rejected() {
    let yaml = valid_yaml().replace("container_port: 8081", "container_port: 8080");
    let error = VerificationManifest::from_yaml(&yaml)
        .unwrap()
        .validate(true)
        .unwrap_err();
    assert!(error.to_string().contains("duplicated"));
}

#[test]
fn absolute_probe_urls_are_rejected() {
    let yaml = valid_yaml().replace("path: /v1/coupon", "path: https://example.com/v1/coupon");
    let error = VerificationManifest::from_yaml(&yaml)
        .unwrap()
        .validate(true)
        .unwrap_err();
    assert!(error.to_string().contains("safe relative HTTP path"));
}

#[test]
fn unsupported_probe_methods_are_rejected_by_deserialization() {
    let yaml = valid_yaml().replace("method: POST", "method: DELETE");
    let error = VerificationManifest::from_yaml(&yaml).unwrap_err();
    assert!(error.to_string().contains("unknown variant"));
}

#[test]
fn zero_and_oversized_limits_are_rejected() {
    let zero = valid_yaml().replace("memory_mb: 512", "memory_mb: 0");
    assert!(VerificationManifest::from_yaml(&zero)
        .unwrap()
        .validate(true)
        .unwrap_err()
        .to_string()
        .contains("memory_mb"));

    let large = valid_yaml().replace("pids: 128", "pids: 1025");
    assert!(VerificationManifest::from_yaml(&large)
        .unwrap()
        .validate(true)
        .unwrap_err()
        .to_string()
        .contains("pids"));
}

#[test]
fn more_than_maximum_total_checks_are_rejected() {
    let mut manifest = parsed();
    let command_template = manifest.commands[0].clone();
    while manifest.commands.len() < 32 {
        let mut command = command_template.clone();
        command.id = format!("build-{}", manifest.commands.len());
        manifest.commands.push(command);
    }
    let probe_template = manifest.probes[0].clone();
    while manifest.commands.len() + manifest.services.len() + manifest.probes.len()
        <= MAX_TOTAL_CHECKS
    {
        let mut probe = probe_template.clone();
        probe.id = format!("probe-{}", manifest.probes.len());
        manifest.probes.push(probe);
    }
    assert!(manifest.probes.len() <= 32);
    let error = manifest.validate(true).unwrap_err();
    assert!(error.to_string().contains("total checks exceed"));
}

#[test]
fn registry_requires_the_exact_trusted_manifest_digest() {
    let root = tempdir().unwrap();
    let path = root.path().join("coupon-tax-inclusive").join("1.0.0");
    fs::create_dir_all(&path).unwrap();
    let bytes = valid_yaml().into_bytes();
    fs::write(path.join("verification.yaml"), &bytes).unwrap();
    let request = VerificationRequest {
        version: 1,
        run_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        scenario_id: "coupon-tax-inclusive".into(),
        scenario_version: "1.0.0".into(),
        expected_manifest_digest: manifest_digest(&bytes),
        repositories: vec![
            RepositoryRevision {
                repository_id: "pricing-api".into(),
                clone_url: "https://github.com/example/pricing-api.git".into(),
                base_commit_sha: "a".repeat(40),
                head_commit_sha: "b".repeat(40),
            },
            RepositoryRevision {
                repository_id: "checkout-api".into(),
                clone_url: "https://github.com/example/checkout-api.git".into(),
                base_commit_sha: "c".repeat(40),
                head_commit_sha: "d".repeat(40),
            },
        ],
    };
    let registry = ScenarioRegistry::new(root.path(), true).unwrap();
    let loaded = registry.load(&request).unwrap();
    assert_eq!(loaded.digest, request.expected_manifest_digest);
    assert_eq!(loaded.service_order, vec!["pricing", "checkout"]);

    let mut mismatch = request;
    mismatch.expected_manifest_digest = "f".repeat(64);
    let error = registry.load(&mismatch).unwrap_err();
    assert!(matches!(error, RunnerError::ManifestDigestMismatch { .. }));
}

fn config_map(scenario_root: &str, run_root: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("BUZZ_SANDBOX_API_TOKEN".into(), "x".repeat(32)),
        ("BUZZ_SANDBOX_SCENARIO_ROOT".into(), scenario_root.into()),
        ("BUZZ_SANDBOX_RUN_ROOT".into(), run_root.into()),
    ])
}

#[test]
fn runner_config_defaults_to_loopback_and_redacts_token() {
    let scenario_root = tempdir().unwrap();
    let run_parent = tempdir().unwrap();
    let run_root = run_parent.path().join("runs");
    let config = RunnerConfig::from_map(config_map(
        scenario_root.path().to_str().unwrap(),
        run_root.to_str().unwrap(),
    ))
    .unwrap();
    assert!(config.bind_addr.ip().is_loopback());
    assert_eq!(config.max_concurrent_runs, 2);
    let debug = format!("{config:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(&"x".repeat(32)));
}

#[test]
fn runner_config_rejects_remote_bind_without_explicit_override() {
    let scenario_root = tempdir().unwrap();
    let run_root = tempdir().unwrap();
    let mut values = config_map(
        scenario_root.path().to_str().unwrap(),
        run_root.path().to_str().unwrap(),
    );
    values.insert("BUZZ_SANDBOX_BIND_ADDR".into(), "0.0.0.0:8787".into());
    let error = RunnerConfig::from_map(values).unwrap_err();
    assert!(error.to_string().contains("non-loopback"));
}

#[test]
fn file_urls_require_a_canonical_source_root() {
    let scenario_root = tempdir().unwrap();
    let run_root = tempdir().unwrap();
    let mut values = config_map(
        scenario_root.path().to_str().unwrap(),
        run_root.path().to_str().unwrap(),
    );
    values.insert("BUZZ_SANDBOX_ALLOW_FILE_URLS".into(), "true".into());
    let error = RunnerConfig::from_map(values).unwrap_err();
    assert!(error.to_string().contains("FILE_SOURCE_ROOT"));
}
