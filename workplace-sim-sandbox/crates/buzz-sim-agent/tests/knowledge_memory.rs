use std::collections::BTreeMap;

use buzz_sim_agent::{
    ConversationSurface, MemoryAudience, MemoryLedger, MemoryRecord, MemoryRecordOutcome,
    NpcContextBuilder, NpcTurnRequest, PersonaDirectory, PersonaPack, WorldSnapshot,
};
use uuid::Uuid;

const PERSONA: &str = r#"
version: 1
personas:
  - id: minseo
    display_name: 문민서
    presentation: woman
    role: staff_backend_engineer
    team: checkout
    public_traits: [무뚝뚝함]
    private_traits: [과거 장애를 숨기고 있음]
    speech_style: [짧은 문장]
    goals:
      - id: preserve_v1
        description: API v1을 보호한다
        priority: 90
    capabilities: [send_message, create_branch, run_verification]
    channels: [checkout-team]
    repository_access:
      legacy-cart: maintain
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
      - id: manual_patch
        statement: 과거 운영 DB를 수동 수정했다
        stance: fact
        disclosure: never
"#;

fn record(
    id: &str,
    session_id: Uuid,
    sequence: u64,
    audience: MemoryAudience,
    summary: &str,
) -> MemoryRecord {
    MemoryRecord::new(
        Uuid::parse_str(id).unwrap(),
        session_id,
        sequence,
        "minseo",
        audience,
        summary,
        Vec::<String>::new(),
    )
    .unwrap()
}

#[test]
fn memory_recording_is_idempotent_but_conflicting_reuse_is_rejected() {
    let session_id = Uuid::parse_str("00000000-0000-4000-8000-000000000101").unwrap();
    let mut ledger = MemoryLedger::default();
    let first = record(
        "00000000-0000-4000-8000-000000000201",
        session_id,
        1,
        MemoryAudience::ActorOnly {
            actor_id: "minseo".to_string(),
        },
        "플레이어가 API 계약부터 확인했다",
    );

    assert_eq!(
        ledger.record(first.clone()).unwrap(),
        MemoryRecordOutcome::Inserted
    );
    assert_eq!(
        ledger.record(first.clone()).unwrap(),
        MemoryRecordOutcome::Duplicate
    );

    let conflicting = MemoryRecord::new(
        first.event_id,
        session_id,
        1,
        "minseo",
        MemoryAudience::ActorOnly {
            actor_id: "minseo".to_string(),
        },
        "같은 ID지만 다른 내용",
        Vec::<String>::new(),
    )
    .unwrap();
    let error = ledger.record(conflicting).unwrap_err();
    assert!(error.to_string().contains("memory event"));
}

#[test]
fn context_contains_only_memories_visible_to_the_npc_in_stable_order() {
    let directory = PersonaDirectory::new(PersonaPack::from_yaml(PERSONA).unwrap()).unwrap();
    let session_id = Uuid::parse_str("00000000-0000-4000-8000-000000000102").unwrap();
    let mut ledger = MemoryLedger::default();

    let records = [
        record(
            "00000000-0000-4000-8000-000000000205",
            session_id,
            5,
            MemoryAudience::Public,
            "전사 릴리스 프리즈가 공지됐다",
        ),
        record(
            "00000000-0000-4000-8000-000000000202",
            session_id,
            2,
            MemoryAudience::ActorOnly {
                actor_id: "nari".to_string(),
            },
            "나리만 아는 우회 배포 절차",
        ),
        record(
            "00000000-0000-4000-8000-000000000204",
            session_id,
            4,
            MemoryAudience::Team {
                team_id: "growth".to_string(),
            },
            "Growth 팀의 비공개 일정",
        ),
        record(
            "00000000-0000-4000-8000-000000000203",
            session_id,
            3,
            MemoryAudience::Team {
                team_id: "checkout".to_string(),
            },
            "Checkout 회고에서 계약 테스트를 추가하기로 했다",
        ),
        record(
            "00000000-0000-4000-8000-000000000201",
            session_id,
            1,
            MemoryAudience::ActorOnly {
                actor_id: "minseo".to_string(),
            },
            "민서는 플레이어의 첫 PR을 검토했다",
        ),
    ];
    for item in records {
        ledger.record(item).unwrap();
    }

    let request = NpcTurnRequest {
        session_id,
        turn_id: Uuid::parse_str("00000000-0000-4000-8000-000000000301").unwrap(),
        actor_id: "minseo".to_string(),
        player_input: "이번 변경에서 뭘 먼저 확인해야 해?".to_string(),
        surface: ConversationSurface::DirectMessage,
        world: WorldSnapshot {
            week: 3,
            sprint: 2,
            work_block: 91,
            active_incident: None,
            visible_facts: BTreeMap::from([(
                "ticket".to_string(),
                "쿠폰 금액 표시 오류".to_string(),
            )]),
        },
    };

    let context = NpcContextBuilder::new(&directory, &ledger)
        .build(&request, 16)
        .unwrap();

    assert_eq!(
        context
            .memories
            .iter()
            .map(|memory| memory.sequence)
            .collect::<Vec<_>>(),
        vec![1, 3, 5]
    );
    assert!(context
        .knowledge
        .iter()
        .any(|entry| entry.id == "manual_patch"));
    assert_eq!(context.request.player_input, request.player_input);

    let same_context = NpcContextBuilder::new(&directory, &ledger)
        .build(&request, 16)
        .unwrap();
    assert_eq!(context.digest().unwrap(), same_context.digest().unwrap());
}
