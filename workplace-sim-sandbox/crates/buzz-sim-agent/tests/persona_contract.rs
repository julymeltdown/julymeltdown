use buzz_sim_agent::{
    CharacterPresentation, KnowledgeDisclosure, KnowledgeStance, NpcAvailability, NpcCapability,
    PersonaDirectory, PersonaPack, PERSONA_PACK_VERSION,
};
use buzz_sim_github::RepositoryAccess;

const VALID_PACK: &str = r#"
version: 1
personas:
  - id: minseo
    display_name: 문민서
    presentation: woman
    role: staff_backend_engineer
    team: checkout
    public_traits: [무뚝뚝함, 레거시 전문가]
    private_traits: [과거 장애에 대한 죄책감]
    speech_style: [짧은 문장, 불확실한 사실은 단정하지 않음]
    goals:
      - id: preserve_api_v1
        description: 모바일 API v1 호환성을 유지한다
        priority: 95
    capabilities:
      - send_message
      - request_review
      - create_branch
      - open_pull_request
      - review_pull_request
      - run_verification
      - escalate
    channels: [checkout-team, project-coupon]
    repository_access:
      legacy-cart: maintain
      mobile-contracts: read
    workload: 78
    availability: available
    knowledge:
      - id: mobile_v1_consumer
        statement: 모바일 앱 세 버전이 아직 API v1을 사용한다
        stance: fact
        disclosure: team
      - id: growth_api_stable
        statement: Growth API는 이번 분기에는 안정적일 것이다
        stance: belief
        disclosure: discretionary
      - id: incident_manual_patch
        statement: 2024년 장애 때 운영 DB를 수동으로 수정했다
        stance: fact
        disclosure: never
"#;

#[test]
fn valid_persona_pack_round_trips_and_preserves_work_authority() {
    let pack = PersonaPack::from_yaml(VALID_PACK).unwrap();
    assert_eq!(pack.version, PERSONA_PACK_VERSION);

    let directory = PersonaDirectory::new(pack).unwrap();
    let minseo = directory.resolve("minseo").unwrap();

    assert_eq!(minseo.presentation, CharacterPresentation::Woman);
    assert_eq!(minseo.availability, NpcAvailability::Available);
    assert!(minseo.capabilities.contains(&NpcCapability::CreateBranch));
    assert_eq!(
        minseo.repository_access.get("legacy-cart"),
        Some(&RepositoryAccess::Maintain)
    );
    assert_eq!(minseo.knowledge[0].stance, KnowledgeStance::Fact);
    assert_eq!(
        minseo.knowledge[2].disclosure,
        KnowledgeDisclosure::Never
    );
}

#[test]
fn presentation_other_than_woman_is_rejected_by_the_schema() {
    let invalid = VALID_PACK.replace("presentation: woman", "presentation: man");
    let error = PersonaPack::from_yaml(&invalid).unwrap_err();

    assert!(error.to_string().contains("presentation"));
}

#[test]
fn unknown_yaml_fields_are_rejected() {
    let invalid = VALID_PACK.replace("    workload: 78", "    romance_route: true\n    workload: 78");
    let error = PersonaPack::from_yaml(&invalid).unwrap_err();

    assert!(error.to_string().contains("romance_route"));
}

#[test]
fn duplicate_persona_and_knowledge_ids_are_rejected() {
    let duplicate_fact = VALID_PACK.replace(
        "      - id: growth_api_stable",
        "      - id: mobile_v1_consumer",
    );
    assert!(PersonaDirectory::new(PersonaPack::from_yaml(&duplicate_fact).unwrap()).is_err());

    let pack = PersonaPack::from_yaml(VALID_PACK).unwrap();
    let duplicate_persona = PersonaPack {
        version: pack.version,
        personas: vec![pack.personas[0].clone(), pack.personas[0].clone()],
    };
    assert!(PersonaDirectory::new(duplicate_persona).is_err());
}

#[test]
fn workload_above_one_hundred_is_rejected() {
    let invalid = VALID_PACK.replace("workload: 78", "workload: 101");
    let error = PersonaDirectory::new(PersonaPack::from_yaml(&invalid).unwrap()).unwrap_err();

    assert!(error.to_string().contains("workload"));
}
