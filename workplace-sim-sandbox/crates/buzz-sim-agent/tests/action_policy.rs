use std::collections::BTreeSet;

use buzz_sim_agent::{
    ActionPolicy, ConversationSurface, NpcActionDraft, NpcCapability, NpcReplyDraft,
    PersonaDirectory, PersonaPack, PolicyViolation,
};

const PERSONA: &str = r#"
version: 1
personas:
  - id: minseo
    display_name: 문민서
    presentation: woman
    role: staff_backend_engineer
    team: checkout
    public_traits: [무뚝뚝함]
    private_traits: [과거 장애에 대한 죄책감]
    speech_style: [짧은 문장]
    goals:
      - id: preserve_v1
        description: API v1을 보호한다
        priority: 90
    capabilities:
      - send_message
      - request_review
      - create_branch
      - open_pull_request
      - review_pull_request
      - run_verification
      - escalate
      - schedule_meeting
    channels: [checkout-team, project-coupon]
    repository_access:
      legacy-cart: maintain
      mobile-contracts: read
    workload: 40
    availability: available
    knowledge:
      - id: public_policy
        statement: 보호 브랜치에는 직접 푸시할 수 없다
        stance: fact
        disclosure: public
      - id: mobile_v1
        statement: 모바일 앱은 API v1을 사용한다
        stance: fact
        disclosure: team
      - id: risky_option
        statement: 운영 우회 배포를 사용할 수 있다
        stance: belief
        disclosure: discretionary
      - id: manual_patch
        statement: 과거 운영 DB를 수동 수정했다
        stance: fact
        disclosure: never
  - id: eugene
    display_name: 최유진
    presentation: woman
    role: qa_engineer
    team: checkout
    public_traits: [밝음]
    private_traits: [버그 발견을 즐김]
    speech_style: [정확한 재현 절차]
    goals:
      - id: prevent_regression
        description: 회귀를 막는다
        priority: 95
    capabilities: [send_message, run_verification]
    channels: [checkout-team]
    repository_access:
      legacy-cart: read
      mobile-contracts: read
    workload: 55
    availability: available
    knowledge: []
"#;

fn directory() -> PersonaDirectory {
    PersonaDirectory::new(PersonaPack::from_yaml(PERSONA).unwrap()).unwrap()
}

#[test]
fn repository_writes_require_both_capability_and_write_access() {
    let directory = directory();
    let policy = ActionPolicy::new(&directory);
    let minseo = directory.resolve("minseo").unwrap();

    policy
        .validate_action(
            minseo,
            &NpcActionDraft::CreateBranch {
                repository_id: "legacy-cart".to_string(),
                branch_name: "sim/minseo/coupon-fix".to_string(),
                purpose: "쿠폰 계산 수정".to_string(),
            },
        )
        .unwrap();

    let error = policy
        .validate_action(
            minseo,
            &NpcActionDraft::CreateBranch {
                repository_id: "mobile-contracts".to_string(),
                branch_name: "sim/minseo/contract-change".to_string(),
                purpose: "계약 수정".to_string(),
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        PolicyViolation::RepositoryWriteDenied { repository_id }
            if repository_id == "mobile-contracts"
    ));

    let eugene = directory.resolve("eugene").unwrap();
    let error = policy
        .validate_action(
            eugene,
            &NpcActionDraft::CreateBranch {
                repository_id: "legacy-cart".to_string(),
                branch_name: "sim/eugene/test".to_string(),
                purpose: "테스트 수정".to_string(),
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        PolicyViolation::CapabilityMissing {
            capability: NpcCapability::CreateBranch
        }
    ));
}

#[test]
fn message_targets_must_be_subscribed_channels() {
    let directory = directory();
    let policy = ActionPolicy::new(&directory);
    let minseo = directory.resolve("minseo").unwrap();

    let error = policy
        .validate_action(
            minseo,
            &NpcActionDraft::SendMessage {
                channel_id: "executive-private".to_string(),
                body: "진행 상황 공유".to_string(),
                fact_ids: BTreeSet::new(),
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        PolicyViolation::ChannelNotSubscribed { channel_id }
            if channel_id == "executive-private"
    ));
}

#[test]
fn confidential_fact_references_and_literal_leaks_are_blocked() {
    let directory = directory();
    let policy = ActionPolicy::new(&directory);
    let minseo = directory.resolve("minseo").unwrap();

    let cited = NpcReplyDraft {
        body: "과거 수정에 대해 말할 게 있어요.".to_string(),
        fact_ids: BTreeSet::from(["manual_patch".to_string()]),
    };
    assert!(matches!(
        policy.validate_reply(minseo, &ConversationSurface::DirectMessage, &cited),
        Err(PolicyViolation::FactDisclosureDenied { fact_id }) if fact_id == "manual_patch"
    ));

    let leaked = NpcReplyDraft {
        body: "과거 운영 DB를 수동 수정했다".to_string(),
        fact_ids: BTreeSet::new(),
    };
    assert!(matches!(
        policy.validate_reply(minseo, &ConversationSurface::DirectMessage, &leaked),
        Err(PolicyViolation::ConfidentialTextLeak { fact_id }) if fact_id == "manual_patch"
    ));
}

#[test]
fn discretionary_facts_are_allowed_in_dm_but_not_in_team_channels() {
    let directory = directory();
    let policy = ActionPolicy::new(&directory);
    let minseo = directory.resolve("minseo").unwrap();
    let reply = NpcReplyDraft {
        body: "운영 우회 배포라는 선택지도 있기는 합니다.".to_string(),
        fact_ids: BTreeSet::from(["risky_option".to_string()]),
    };

    policy
        .validate_reply(minseo, &ConversationSurface::DirectMessage, &reply)
        .unwrap();
    assert!(matches!(
        policy.validate_reply(
            minseo,
            &ConversationSurface::Channel {
                channel_id: "checkout-team".to_string()
            },
            &reply,
        ),
        Err(PolicyViolation::FactDisclosureDenied { fact_id }) if fact_id == "risky_option"
    ));
}

#[test]
fn verification_requires_full_commit_and_manifest_digests() {
    let directory = directory();
    let policy = ActionPolicy::new(&directory);
    let minseo = directory.resolve("minseo").unwrap();

    let error = policy
        .validate_action(
            minseo,
            &NpcActionDraft::RunVerification {
                repository_id: "legacy-cart".to_string(),
                commit_sha: "abc123".to_string(),
                manifest_digest: "f".repeat(64),
            },
        )
        .unwrap_err();
    assert!(matches!(error, PolicyViolation::InvalidCommitSha));
}
