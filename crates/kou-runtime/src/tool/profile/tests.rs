use super::*;

#[test]
fn profile_for_model() {
    for (model, want) in [
        ("claude-opus-5", Profile::Edit),
        ("anthropic/claude-sonnet-4.6", Profile::Edit),
        ("gpt-6-astra", Profile::Patch),
        ("openai/gpt-5.1-codex", Profile::Patch),
        ("o3-pro", Profile::Patch),
        ("codex-mini", Profile::Patch),
        ("gemini-3", Profile::Edit),
        ("", Profile::Edit),
        ("llama-4", Profile::Edit),
        ("ollama/gpt-oss", Profile::Patch),
        // Only what follows the last slash names the model, trimmed and in any case.
        (" OpenAI/ChatGPT-4o\n", Profile::Patch),
        ("gpt/claude", Profile::Edit),
        ("o", Profile::Edit),
        ("o0", Profile::Edit),
        ("O9", Profile::Patch),
        ("opus", Profile::Edit),
    ] {
        assert_eq!(Profile::for_model(model), want, "{model:?}");
    }
    assert_eq!(Profile::Auto.resolve("gpt-6"), Profile::Patch);
    assert_eq!(Profile::Code.resolve("gpt-6"), Profile::Code);
    // An empty name is the automatic profile.
    let unnamed = Profile::parse("").unwrap();
    assert_eq!(unnamed.resolve("claude-opus-5"), Profile::Edit);
}

#[test]
fn parse_profile() {
    assert_eq!(Profile::parse(" Code ").unwrap(), Profile::Code);
    assert_eq!(Profile::parse("").unwrap(), Profile::Auto);
    assert_eq!(
        Profile::parse("bogus").unwrap_err().to_string(),
        "unknown tool profile \"bogus\"; the profiles are auto, edit, patch, code, shell"
    );
    assert!(Profile::Auto.tools().is_empty());
    assert!(Profile::Edit.tools().contains(&EDIT_NAME));
    assert!(Profile::Patch.tools().contains(&APPLY_PATCH_NAME));
    assert!(!Profile::Patch.tools().contains(&EDIT_NAME));
    assert_eq!(Profile::Code.tools(), [CODE_NAME]);
    let all = all_profile_tools();
    for name in STATIC_NAMES {
        assert!(
            all.contains(&name),
            "all profile tools lack {name}: {all:?}"
        );
    }
    assert_eq!(all.len(), STATIC_NAMES.len());
}

#[test]
fn profiles_keep_their_names_order_and_descriptions() {
    assert_eq!(PROFILES[0], Profile::default());
    assert_eq!(profile_names(), "auto, edit, patch, code, shell");
    for profile in PROFILES {
        assert_eq!(Profile::parse(profile.name()).unwrap(), profile);
        assert_eq!(profile.to_string().parse::<Profile>().unwrap(), profile);
        assert!(!profile.description().is_empty());
        // Every tool a profile names is a built-in one, and SkillUse comes last.
        assert!(
            profile
                .tools()
                .iter()
                .all(|name| STATIC_NAMES.contains(name))
        );
        let skill_use = profile
            .tools()
            .iter()
            .position(|name| *name == SKILL_USE_NAME);
        assert!(skill_use.is_none_or(|at| at + 1 == profile.tools().len()));
    }
    assert_eq!(
        all_profile_tools(),
        [
            "Bash",
            "Read",
            "Edit",
            "Write",
            "ViewImage",
            "TranscriptSearch",
            "SkillUse",
            "apply_patch",
            "Code"
        ]
    );
}

// Go lower-cases one scalar to one scalar and trims Unicode space; the name
// it could not read is quoted back as Go quotes it.
#[test]
fn profile_names_are_read_as_go_reads_them() {
    assert_eq!(
        Profile::parse("\u{a0}\u{2003}SHELL\u{3000}").unwrap(),
        Profile::Shell
    );
    assert_eq!(Profile::parse("ED\u{130}T").unwrap(), Profile::Edit);
    for (name, quoted) in [
        ("\u{391}\u{3a3}", "\"\u{3b1}\u{3c3}\""),
        ("\u{200b}shell", "\"\\u200bshell\""),
        ("a\nb", "\"a\\nb\""),
        ("CODE\u{307}", "\"code\u{307}\""),
    ] {
        assert_eq!(
            Profile::parse(name).unwrap_err().to_string(),
            format!(
                "unknown tool profile {quoted}; the profiles are auto, edit, patch, code, shell"
            )
        );
    }
}
