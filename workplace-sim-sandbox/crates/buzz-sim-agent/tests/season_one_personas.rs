use std::collections::BTreeSet;

use buzz_sim_agent::{
    CharacterPresentation, NpcAvailability, NpcCapability, PersonaDirectory, PersonaPack,
};

const SEASON_ONE_PERSONAS: &str = include_str!("../../../personas/momo-commerce-season-1.yaml");

#[test]
fn season_one_pack_loads_eight_distinct_women_with_work_authority() {
    let directory =
        PersonaDirectory::new(PersonaPack::from_yaml(SEASON_ONE_PERSONAS).unwrap()).unwrap();

    assert_eq!(directory.len(), 8);
    let ids = directory
        .personas()
        .map(|persona| persona.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids,
        BTreeSet::from([
            "chaewon", "eunbi", "eugene", "harin", "jisoo", "minseo", "nari", "seoyun",
        ])
    );

    for persona in directory.personas() {
        assert_eq!(persona.presentation, CharacterPresentation::Woman);
        assert_eq!(persona.availability, NpcAvailability::Available);
        assert!(!persona.goals.is_empty());
        assert!(persona.capabilities.contains(&NpcCapability::SendMessage));
        assert!(!persona.channels.is_empty());
    }
}

#[test]
fn season_one_roles_cover_cross_functional_company_work() {
    let directory =
        PersonaDirectory::new(PersonaPack::from_yaml(SEASON_ONE_PERSONAS).unwrap()).unwrap();
    let roles = directory
        .personas()
        .map(|persona| persona.role.as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        roles,
        BTreeSet::from([
            "engineering_manager",
            "frontend_engineer",
            "product_designer",
            "product_manager",
            "qa_engineer",
            "security_engineer",
            "sre",
            "staff_backend_engineer",
        ])
    );

    assert!(directory
        .resolve("minseo")
        .unwrap()
        .capabilities
        .contains(&NpcCapability::CreateBranch));
    assert!(directory
        .resolve("chaewon")
        .unwrap()
        .capabilities
        .contains(&NpcCapability::ScheduleMeeting));
    assert!(directory
        .resolve("jisoo")
        .unwrap()
        .capabilities
        .contains(&NpcCapability::RunVerification));
}
