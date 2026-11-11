use super::*;
use crate::operation::decode_skill;
use kou_domain::Json;

struct Fixed;
impl Translator for Fixed {
    fn translate(&self, _: &str, _: &mut dyn FnMut(Spec) -> String) -> CallStatus {
        CallStatus::default()
    }
    fn result(&self, _: &str, _: &CallStatus, _: &[Arc<Operation>]) -> Result<ToolResult> {
        Ok(ToolResult::default())
    }
}
fn fixed() -> Arc<dyn Translator> {
    Arc::new(Fixed)
}
fn definition(name: &str) -> Definition {
    Definition {
        tool: Tool {
            name: name.into(),
            ..Tool::default()
        },
        ..Definition::default()
    }
}
fn names(registry: &Registry) -> Vec<String> {
    registry
        .static_definitions()
        .iter()
        .map(|definition| definition.tool.name.clone())
        .collect()
}
fn resolves_to(registry: &Registry, name: &str, want: &Arc<dyn Translator>) -> bool {
    registry
        .resolve(name)
        .is_some_and(|got| Arc::ptr_eq(&got, want))
}
fn no_submission(_: Spec) -> String {
    panic!("the call submitted work")
}
fn review() -> Skill {
    Skill {
        name: "review".into(),
        description: "Review code.".into(),
        path: "/skills/review/SKILL.md".into(),
        manual: false,
    }
}

#[test]
fn registry_has_fixed_definitions_and_injected_translators() {
    let (bash, view_image) = (fixed(), fixed());
    let registry = Registry::new(
        StaticTranslators {
            bash: Some(bash.clone()),
            view_image: Some(view_image.clone()),
            ..StaticTranslators::default()
        },
        [BASH_NAME, VIEW_IMAGE_NAME, SKILL_USE_NAME],
    );
    assert_eq!(
        names(&registry),
        [BASH_NAME, VIEW_IMAGE_NAME, SKILL_USE_NAME]
    );
    assert_eq!(registry.names(), names(&registry));
    assert!(resolves_to(&registry, BASH_NAME, &bash));
    assert!(resolves_to(&registry, VIEW_IMAGE_NAME, &view_image));
    assert!(!resolves_to(&registry, BASH_NAME, &view_image));
    assert!(registry.resolve("unknown").is_none());

    // SkillUse is the registry's own translator: it reads the registry's skills.
    let skill_use = registry.resolve(SKILL_USE_NAME).unwrap();
    let call = ToolCall {
        call_id: "call".into(),
        name: SKILL_USE_NAME.into(),
        arguments: r#"{"name":"review"}"#.into(),
    };
    let status = skill_use.translate_call(&call, &mut no_submission);
    assert_eq!(status.error, "skill \"review\" is not registered");
    registry.register_skill(review()).unwrap();
    let mut specs = Vec::new();
    let status = skill_use.translate_call(&call, &mut |spec| {
        specs.push(spec);
        "operation-1".into()
    });
    assert_eq!(status.waiting(), ["operation-1"]);
    let operation = specs.pop().unwrap().operation("operation-1".into());
    assert_eq!(
        decode_skill(&operation).unwrap().path,
        "/skills/review/SKILL.md"
    );
    assert!(registry.skill_catalog().resolve("review").is_some());
}

#[test]
fn registry_owns_canonical_bash_definition() {
    let registry = Registry::new(StaticTranslators::default(), [BASH_NAME]);
    let definitions = registry.static_definitions();
    assert_eq!(
        serde_json::to_string(&definitions[0].tool).unwrap(),
        concat!(
            r#"{"Type":"function","Name":"Bash","Description":"Execute a shell command in background. Independent commands may be issued as parallel tool calls in one turn. Command child processes are killed when the shell exits.","#,
            r#""Parameters":{"properties":{"command":{"description":"The shell command to execute.","type":"string"},"#,
            r#""max_output_length":{"default":40000,"description":"Maximum characters per output text field. Truncated text keeps its head and tail, around a marker stating how much was omitted, and path to the file with the complete stream. Defaults to 40000.","maximum":1000000,"minimum":1,"type":"integer"},"#,
            r#""timeout":{"description":"Seconds the command may run before it is stopped (SIGTERM, then SIGKILL). Defaults to 600; 0 lets it run; at most 14400.","maximum":14400,"minimum":0,"type":"number"}},"#,
            r#""required":["command"],"type":"object"}}"#
        )
    );
}

#[test]
fn registry_owns_canonical_skill_use_definition() {
    let registry = Registry::new(StaticTranslators::default(), [SKILL_USE_NAME]);
    let definitions = registry.static_definitions();
    assert_eq!(
        serde_json::to_string(&definitions[0].tool).unwrap(),
        concat!(
            r#"{"Type":"function","Name":"SkillUse","Description":"Load the instructions for a registered skill.","#,
            r#""Parameters":{"properties":{"name":{"description":"The exact name of the skill to load.","type":"string"}},"#,
            r#""required":["name"],"type":"object"}}"#
        )
    );
}

#[test]
fn registry_static_definitions_returns_independent_values() {
    let registry = Registry::new(StaticTranslators::default(), [BASH_NAME]);
    let mut changed = Definition::clone(&registry.static_definitions()[0]);
    changed.tool.name = "changed".into();
    changed
        .tool
        .parameters
        .as_mut()
        .unwrap()
        .insert("changed".into(), Json::parse("true").unwrap());
    let definitions = registry.static_definitions();
    assert_eq!(definitions[0].tool.name, BASH_NAME);
    let parameters = definitions[0].tool.parameters.as_ref().unwrap();
    assert!(!parameters.contains_key("changed"));
    // The values are shared and immutable: no call copies a schema.
    assert!(Arc::ptr_eq(
        &definitions[0],
        &registry.static_definitions()[0]
    ));
}

#[test]
fn registry_registers_lists_and_unregisters_skills() {
    let registry = Registry::new(StaticTranslators::default(), [] as [&str; 0]);
    let first = Skill {
        name: "go-review".into(),
        description: "Review Go code".into(),
        path: "/skills/go/SKILL.md".into(),
        manual: false,
    };
    let second = Skill {
        name: "documents".into(),
        description: "Edit documents".into(),
        path: "/skills/docs/SKILL.md".into(),
        manual: false,
    };
    let first_id = registry.register_skill(first.clone()).unwrap();
    let second_id = registry.register_skill(second.clone()).unwrap();
    let listed = |registry: &Registry| -> Vec<Skill> {
        registry
            .skills()
            .iter()
            .map(|skill| Skill::clone(skill))
            .collect()
    };
    assert_eq!(listed(&registry), [first.clone(), second.clone()]);

    let mut copy = listed(&registry);
    copy[0].path = "changed".into();
    assert_eq!(*registry.skills()[0], first);

    registry.unregister_skill(first_id);
    registry.unregister_skill(first_id);
    registry.unregister_skill(RegistrationId::nil());
    assert_eq!(listed(&registry), [second]);
    assert_ne!(first_id, second_id);
}

#[test]
fn registry_rejects_invalid_and_duplicate_skills() {
    let registry = Registry::new(StaticTranslators::default(), [] as [&str; 0]);
    let error = |skill: Skill| registry.register_skill(skill).unwrap_err().to_string();
    assert_eq!(error(Skill::default()), "skill path must be set");
    let mut skill = Skill {
        path: "/skill".into(),
        ..Skill::default()
    };
    assert_eq!(error(skill.clone()), "skill name must be set");
    skill.name = "review".into();
    assert_eq!(error(skill.clone()), "skill description must be set");
    skill.description = "Review code".into();
    registry.register_skill(skill.clone()).unwrap();
    assert_eq!(
        error(skill.clone()),
        "skill path \"/skill\" is already registered"
    );
    skill.description = "Review other code".into();
    skill.path = "/other-skill".into();
    assert_eq!(error(skill), "skill name \"review\" is already registered");
}

#[test]
fn registry_registers_tools_beside_the_built_in_ones() {
    let registry = Registry::new(StaticTranslators::default(), [BASH_NAME]);
    let plugin = Definition {
        tool: Tool {
            kind: TOOL_FUNCTION.into(),
            name: "GitGlance".into(),
            description: "d".into(),
            parameters: None,
        },
        ..Definition::default()
    };
    let registered = fixed();
    registry
        .register_tool(plugin.clone(), Some(registered.clone()))
        .unwrap();
    for (definition, translator, error) in [
        (
            definition(BASH_NAME),
            Some(registered.clone()),
            "tool \"Bash\" is built in",
        ),
        (
            plugin.clone(),
            Some(registered.clone()),
            "tool \"GitGlance\" is already registered",
        ),
        (
            definition(""),
            Some(registered.clone()),
            "tool name must be set",
        ),
        (
            definition("NoTranslator"),
            None,
            "tool \"NoTranslator\" has no translator",
        ),
    ] {
        let refused = registry.register_tool(definition, translator).unwrap_err();
        assert_eq!(refused.to_string(), error);
    }
    // A built-in tool that is not enabled leaves its name free.
    registry
        .register_tool(definition(VIEW_IMAGE_NAME), Some(registered.clone()))
        .unwrap();
    assert_eq!(names(&registry), ["Bash", "GitGlance", "ViewImage"]);
    assert_eq!(*registry.static_definitions()[1], plugin);
    assert!(resolves_to(&registry, "GitGlance", &registered));
    assert!(resolves_to(&registry, VIEW_IMAGE_NAME, &registered));
}

// A gateway shows Claude the tools as mcp__<server>__<word>_<Tool>; a model
// that shortens the name still reaches the tool.
#[test]
fn registry_resolves_names_a_gateway_disguised() {
    let (read, patch) = (fixed(), fixed());
    let registry = Registry::new(
        StaticTranslators {
            read: Some(read.clone()),
            apply_patch: Some(patch.clone()),
            bash: Some(fixed()),
            ..StaticTranslators::default()
        },
        [READ_NAME, APPLY_PATCH_NAME, BASH_NAME],
    );
    for name in [
        "Read",
        "grace_Read",
        "assault_kidney__grace_Read",
        "mcp__assault_kidney__grace_Read",
    ] {
        assert!(resolves_to(&registry, name, &read), "{name}");
    }
    for name in ["apply_patch", "grace_apply_patch", "mcp__x__y_apply_patch"] {
        assert!(resolves_to(&registry, name, &patch), "{name}");
    }
    for name in [
        "read",
        "Readx",
        "_Read",
        "grace_",
        "grace__",
        "patch",
        "grace_patch",
        "",
    ] {
        assert!(registry.resolve(name).is_none(), "{name}");
    }
}

#[test]
fn registry_refuses_a_name_that_may_stand_for_two_tools() {
    let registry = Registry::new(
        StaticTranslators {
            read: Some(fixed()),
            ..StaticTranslators::default()
        },
        [READ_NAME],
    );
    let notes = fixed();
    registry
        .register_tool(definition("notes_Read"), Some(notes.clone()))
        .unwrap();
    assert_eq!(
        aliases("x_notes_Read", &["Read", "notes_Read"]),
        ["Read", "notes_Read"]
    );
    assert!(registry.resolve("x_notes_Read").is_none());
    // Its own name is not a disguise of Read.
    assert!(resolves_to(&registry, "notes_Read", &notes));
    assert_eq!(
        registry.unavailable("x_notes_Read"),
        "tool \"x_notes_Read\" is not available: it may stand for any of Read, notes_Read; call the one you mean by its exact name"
    );
    assert_eq!(
        registry.unavailable("Grep"),
        "tool \"Grep\" is not available; the tools are Read, notes_Read"
    );
    let empty = Registry::new(StaticTranslators::default(), [] as [&str; 0]);
    assert_eq!(empty.unavailable("Grep"), "tool \"Grep\" is not available");
    assert_eq!(
        unavailable("a\n_Read", &["Read"]),
        "tool \"a\\n_Read\" is not available; the tools are Read"
    );
}

#[test]
fn aliases_are_the_ends_after_an_underscore_sorted_by_bytes() {
    let names = ["z", "é_z", "界_é_z", "Read", "", "_", "_Read"];
    assert_eq!(aliases("q_界_é_z", &names), ["z", "é_z", "界_é_z"]);
    // Neither a leading underscore nor one before another starts an end, and an end is never empty.
    for name in [
        "", "_", "__", "_Read", "Read", "Read_", "x_", "x__", "x_Read_",
    ] {
        assert!(aliases(name, &names).is_empty(), "{name}");
    }
    assert_eq!(aliases("x__Read", &names), ["Read"]);
    assert_eq!(aliases("__Read", &names), ["Read"]);
    assert_eq!(aliases("é_Read", &names), ["Read"]);
    assert!(aliases("x_Read", &[] as &[&str]).is_empty());
}

// Kept from Go: a built-in tool left out of the selection is no name at all,
// so a call of it is read like any other unknown name.
#[test]
fn a_left_out_built_in_name_may_stand_for_a_registered_tool() {
    let registry = Registry::new(StaticTranslators::default(), [READ_NAME]);
    let patch = fixed();
    registry
        .register_tool(definition("patch"), Some(patch.clone()))
        .unwrap();
    assert!(resolves_to(&registry, APPLY_PATCH_NAME, &patch));
    assert!(resolves_to(&registry, "x_apply_patch", &patch));
    assert!(registry.resolve(EDIT_NAME).is_none());
    assert_eq!(
        registry.unavailable(EDIT_NAME),
        "tool \"Edit\" is not available; the tools are Read, patch"
    );
}

#[test]
fn registry_keeps_selected_static_tools_across_catalog_changes() {
    let all = [BASH_NAME, VIEW_IMAGE_NAME, SKILL_USE_NAME];
    for selection in 0..1 << all.len() {
        let want: Vec<&str> = (0..all.len())
            .filter(|index| selection & (1 << index) != 0)
            .map(|index| all[index])
            .collect();
        // Named twice each and in reverse: the selection is a set.
        let enabled: Vec<&str> = want.iter().rev().flat_map(|name| [*name, *name]).collect();
        let registry = Registry::new(StaticTranslators::default(), &enabled);
        let assert_selection = || {
            assert_eq!(names(&registry), want, "selection {selection}");
            for name in all {
                assert_eq!(
                    registry.resolve(name).is_some(),
                    want.contains(&name),
                    "{name}"
                );
            }
        };
        assert_selection();
        let id = registry.register_skill(review()).unwrap();
        assert_selection();
        registry.unregister_skill(id);
        assert_selection();
    }
}

#[test]
fn registry_does_not_resolve_unselected_translators() {
    let registry = Registry::new(
        StaticTranslators {
            bash: Some(fixed()),
            view_image: Some(fixed()),
            ..StaticTranslators::default()
        },
        [] as [&str; 0],
    );
    assert!(registry.static_definitions().is_empty());
    for name in [BASH_NAME, VIEW_IMAGE_NAME, SKILL_USE_NAME] {
        assert!(registry.resolve(name).is_none(), "{name}");
    }
}

#[test]
fn an_enabled_tool_without_a_translator_answers_that_it_is_not_configured() {
    let registry = Registry::new(StaticTranslators::default(), STATIC_NAMES);
    assert_eq!(names(&registry), STATIC_NAMES);
    for name in STATIC_NAMES.iter().filter(|name| **name != SKILL_USE_NAME) {
        let translator = registry.resolve(name).unwrap();
        let message = format!("static tool \"{name}\" is not configured");
        let status = translator.translate("{}", &mut no_submission);
        assert_eq!(status.error, message);
        assert!(!status.error_truncated && status.waiting().is_empty());
        // The result is the same text whatever the recorded status says.
        let recorded = CallStatus {
            error: "recorded".into(),
            ..CallStatus::default()
        };
        let result = translator.result("call", &recorded, &[]).unwrap();
        assert_eq!(result, text_result("call", message));
        assert!(!translator.waits());
        assert_eq!(translator.running(&[]), "");
    }
}

#[test]
fn concurrent_registration_of_one_name_succeeds_once() {
    let registry = Registry::new(StaticTranslators::default(), [BASH_NAME]);
    let registered = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..24)
            .map(|_| scope.spawn(|| registry.register_tool(definition("GitGlance"), Some(fixed()))))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|result| result.is_ok())
            .count()
    });
    assert_eq!(registered, 1);
    assert_eq!(names(&registry), ["Bash", "GitGlance"]);
}
