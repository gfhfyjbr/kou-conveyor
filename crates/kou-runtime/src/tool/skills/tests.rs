use super::*;

fn entry(name: &str, path: &str) -> Skill {
    Skill {
        name: name.into(),
        path: path.into(),
        description: "owned metadata".into(),
        manual: false,
    }
}

#[test]
fn registration_validates_exact_metadata_and_keeps_order_without_loading_files() {
    let catalog = SkillCatalog::default();
    for (skill, error) in [
        (Skill::default(), "skill path must be set"),
        (entry("review", " \n\t"), "skill path must be set"),
        (entry(" \n\t", "/inert/skill"), "skill name must be set"),
        (
            Skill {
                description: " ".into(),
                ..entry("review", "/inert/skill")
            },
            "skill description must be set",
        ),
    ] {
        assert_eq!(catalog.register(skill).unwrap_err().to_string(), error);
        assert!(catalog.skills().is_empty());
    }
    let first = catalog
        .register(entry(" review ", "/inert/a//SKILL.md"))
        .unwrap();
    let second = catalog
        .register(Skill {
            manual: true,
            ..entry("Review", "/inert/b/SKILL.md")
        })
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(first.get_version_num(), 4);
    let skills = catalog.skills();
    assert_eq!(
        skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        [" review ", "Review"]
    );
    assert_eq!(skills[0].path, "/inert/a//SKILL.md");
    assert!(!skills[0].manual && skills[1].manual);
    assert!(catalog.resolve("review").is_none());
    assert!(Arc::ptr_eq(
        &skills[0],
        &catalog.resolve(" review ").unwrap()
    ));
    assert_eq!(
        catalog
            .register(entry("other", "/inert/a//SKILL.md"))
            .unwrap_err()
            .to_string(),
        "skill path \"/inert/a//SKILL.md\" is already registered"
    );
    assert_eq!(
        catalog
            .register(entry("Review", "/inert/other"))
            .unwrap_err()
            .to_string(),
        "skill name \"Review\" is already registered"
    );
    catalog.unregister(RegistrationId::nil());
    catalog.unregister(RegistrationId::new_v4());
    catalog.unregister(first);
    catalog.unregister(first);
    assert!(catalog.resolve(" review ").is_none());
    assert_eq!(skills[0].name, " review ");
    catalog
        .register(entry(" review ", "/inert/a//SKILL.md"))
        .unwrap();
    assert_eq!(
        catalog
            .skills()
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        ["Review", " review "]
    );
}

#[test]
fn concurrent_registration_is_unique_and_retained_metadata_survives_removal() {
    let catalog = SkillCatalog::default();
    let mut registered = Vec::new();
    thread_scope(&catalog, &mut registered);
    assert_eq!(registered.len(), 1);
    let id = registered[0];
    let retained = catalog.resolve("review").unwrap();
    let weak = Arc::downgrade(&retained);
    catalog.unregister(id);
    assert!(catalog.skills().is_empty());
    assert_eq!(retained.description, "owned metadata");
    drop(retained);
    assert!(weak.upgrade().is_none());
}
fn thread_scope(catalog: &SkillCatalog, registered: &mut Vec<RegistrationId>) {
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..24)
            .map(|_| scope.spawn(|| catalog.register(entry("review", "/inert/skill"))))
            .collect();
        for handle in handles {
            if let Ok(id) = handle.join().unwrap() {
                registered.push(id);
            }
        }
    });
}

#[test]
fn skill_metadata_nulls_and_diagnostic_quotes_keep_the_go_contract() {
    assert_eq!(
        serde_json::from_str::<Skill>(
            r#"{"Name":null,"Description":null,"Path":null,"Manual":null}"#
        )
        .unwrap(),
        Skill::default()
    );
    assert_eq!(
        quoted("a\0\x01\x07\x08\x0b\x0c\n\r\t\"\\'界"),
        "\"a\\x00\\x01\\a\\b\\v\\f\\n\\r\\t\\\"\\\\'界\""
    );
    assert_eq!(
        quoted("\u{200b}\u{2028}\u{feff}\u{e0001}"),
        "\"\\u200b\\u2028\\ufeff\\U000e0001\""
    );
}
