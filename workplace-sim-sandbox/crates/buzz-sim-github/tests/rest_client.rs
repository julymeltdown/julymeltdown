use std::sync::{Arc, Mutex};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, post, put},
    Json, Router,
};
use buzz_sim_github::{
    ActorKind, DestinationRepository, GitHubApiError, GitHubRepositoryApi, GitHubRestClient,
    GrantOutcome, RepositoryAccess, RepositoryGrant,
};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedRequest {
    method: String,
    path: String,
    authorization: String,
    accept: String,
    api_version: String,
    body: Option<Value>,
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<ObservedRequest>>>);

impl Recorder {
    fn push(&self, request: ObservedRequest) {
        self.0.lock().unwrap().push(request);
    }

    fn snapshot(&self) -> Vec<ObservedRequest> {
        self.0.lock().unwrap().clone()
    }
}

fn observed(
    method: &str,
    path: String,
    headers: &HeaderMap,
    body: Option<Value>,
) -> ObservedRequest {
    ObservedRequest {
        method: method.to_string(),
        path,
        authorization: headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
        accept: headers
            .get("accept")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
        api_version: headers
            .get("x-github-api-version")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
        body,
    }
}

async fn create_repository(
    Path(owner): Path<String>,
    State(recorder): State<Recorder>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    recorder.push(observed(
        "POST",
        format!("/orgs/{owner}/repos"),
        &headers,
        Some(body.clone()),
    ));
    (
        StatusCode::CREATED,
        Json(json!({
            "id": 42,
            "name": body["name"],
            "clone_url": format!("https://github.com/{owner}/{}.git", body["name"].as_str().unwrap()),
            "private": body["private"],
            "default_branch": "main"
        })),
    )
}

async fn grant_repository_access(
    Path((owner, repository, username)): Path<(String, String, String)>,
    State(recorder): State<Recorder>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> StatusCode {
    recorder.push(observed(
        "PUT",
        format!("/repos/{owner}/{repository}/collaborators/{username}"),
        &headers,
        Some(body),
    ));
    StatusCode::NO_CONTENT
}

async fn delete_repository(
    Path((owner, repository)): Path<(String, String)>,
    State(recorder): State<Recorder>,
    headers: HeaderMap,
) -> StatusCode {
    recorder.push(observed(
        "DELETE",
        format!("/repos/{owner}/{repository}"),
        &headers,
        None,
    ));
    StatusCode::NO_CONTENT
}

async fn forbidden_create() -> (StatusCode, Json<Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"message": "organization policy denied repository creation"})),
    )
}

async fn spawn_api(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), task)
}

fn destination() -> DestinationRepository {
    DestinationRepository::new(
        "legacy-cart",
        "acme-sim",
        "session-legacy-cart",
        "main",
        true,
    )
    .unwrap()
}

fn grant() -> RepositoryGrant {
    RepositoryGrant {
        actor_id: "player:developer".to_string(),
        github_login: "player-dev".to_string(),
        actor_kind: ActorKind::Player,
        repository_id: "legacy-cart".to_string(),
        destination_host: "github.com".to_string(),
        destination_owner: "acme-sim".to_string(),
        destination_repository: "session-legacy-cart".to_string(),
        access: RepositoryAccess::Write,
    }
}

#[tokio::test]
async fn rest_client_uses_current_headers_and_expected_repository_endpoints() {
    let recorder = Recorder::default();
    let router = Router::new()
        .route("/orgs/{owner}/repos", post(create_repository))
        .route(
            "/repos/{owner}/{repository}/collaborators/{username}",
            put(grant_repository_access),
        )
        .route(
            "/repos/{owner}/{repository}",
            delete(delete_repository),
        )
        .with_state(recorder.clone());
    let (base_url, server) = spawn_api(router).await;
    let client = GitHubRestClient::new(&base_url, "test-installation-token").unwrap();

    let created = client.create_repository(&destination()).await.unwrap();
    let outcome = client.grant_repository_access(&grant()).await.unwrap();
    client.delete_repository(&destination()).await.unwrap();

    server.abort();
    assert_eq!(created.id, 42);
    assert_eq!(created.name, "session-legacy-cart");
    assert_eq!(outcome, GrantOutcome::AccessUpdated);

    let requests = recorder.snapshot();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/orgs/acme-sim/repos");
    assert_eq!(
        requests[0].body,
        Some(json!({
            "name": "session-legacy-cart",
            "private": true,
            "auto_init": false
        }))
    );
    assert_eq!(requests[1].method, "PUT");
    assert_eq!(
        requests[1].path,
        "/repos/acme-sim/session-legacy-cart/collaborators/player-dev"
    );
    assert_eq!(requests[1].body, Some(json!({"permission": "push"})));
    assert_eq!(requests[2].method, "DELETE");
    assert_eq!(
        requests[2].path,
        "/repos/acme-sim/session-legacy-cart"
    );
    for request in requests {
        assert_eq!(request.authorization, "Bearer test-installation-token");
        assert_eq!(request.accept, "application/vnd.github+json");
        assert_eq!(request.api_version, "2026-03-10");
    }
}

#[tokio::test]
async fn rest_client_preserves_status_and_bounded_error_body() {
    let (base_url, server) = spawn_api(
        Router::new().route("/orgs/{owner}/repos", post(forbidden_create)),
    )
    .await;
    let client = GitHubRestClient::new(&base_url, "test-installation-token").unwrap();

    let error = client.create_repository(&destination()).await.unwrap_err();
    server.abort();

    assert!(matches!(
        error,
        GitHubApiError::HttpStatus {
            operation: "create_repository",
            status: 403,
            body,
        } if body.contains("organization policy denied")
    ));
}

#[test]
fn rest_client_rejects_plain_http_for_non_loopback_hosts() {
    assert!(matches!(
        GitHubRestClient::new("http://example.com", "test-installation-token"),
        Err(GitHubApiError::InvalidBaseUrl(_))
    ));
}
