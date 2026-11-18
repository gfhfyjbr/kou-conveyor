//! What the user is asked, as the interface asks it ([`Question`]), and the
//! answers as Claude Code takes them back. Two things ask:
//!
//! - The `AskUserQuestion` tool (`AskUserQuestionInput` of `sdk-tools.d.ts`).
//!   Claude Code asks leave to run it like any tool; the answers go back in
//!   the call's input (`answers`, from each question's text to its answer),
//!   with the call allowed, which is how the SDK's docs and the ACP adapter
//!   (`applyAskElicitationResponse` of its `elicitation.js`) answer it.
//! - An MCP server's form (`elicitation`, `SDKControlElicitationRequest`): a
//!   JSON schema of flat fields, one question each, answered with the
//!   content typed as the schema says (MCP's `ElicitResult`).

pub(crate) use kou_thread::elicitation::{Field, form, form_content, form_summary};
use kou_thread::{Question, QuestionOption};
use serde_json::{Map, Value, json};

/// The questions of an `AskUserQuestion` call's input. One without its text
/// is left out, as the adapter leaves it out (`extractAskUserQuestions`).
pub(crate) fn tool_questions(input: &Value) -> Vec<Question> {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    };
    input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|question| {
            let options = question
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    Some(QuestionOption {
                        label: text(option, "label")?,
                        description: text(option, "description"),
                    })
                })
                .collect();
            Some(Question {
                header: text(question, "header"),
                text: text(question, "question")?,
                options,
                multiple: question.get("multiSelect").and_then(Value::as_bool) == Some(true),
                // Claude Code's own dialog always has a box for the user's
                // own words ("Other").
                free: true,
            })
        })
        .collect()
}

/// The `AskUserQuestion` call's input with the user's answers to
/// `questions` (as [`tool_questions`] made them from it): `answers` maps each
/// question's text to its answer. Several choices, and the user's own words
/// beside them, are one answer joined with ", " (an item that holds ", " or
/// a quote is quoted, as Claude Code's own dialog does, so that the tool
/// splits them back right). Words beside the one choice of a single-choice
/// question go as its `notes`. A question left unanswered is left out, and
/// marked `skipped` where Claude Code says it takes that mark
/// (`ask_user_question_unanswered_v1`).
pub(crate) fn tool_answers(
    input: &Value,
    questions: &[Question],
    answers: &[Vec<String>],
    mark_unanswered: bool,
) -> Value {
    let mut answered = Map::new();
    let mut notes = Map::new();
    for (at, question) in questions.iter().enumerate() {
        let items: Vec<&str> = answers
            .get(at)
            .into_iter()
            .flatten()
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .collect();
        let is_label = |item: &&str| question.options.iter().any(|option| option.label == *item);
        let answer = if question.multiple {
            join_choices(&items)
        } else {
            let picked: Vec<&str> = items.iter().copied().filter(is_label).collect();
            let words: Vec<&str> = items
                .iter()
                .copied()
                .filter(|item| !is_label(item))
                .collect();
            let words = words.join(", ");
            if picked.is_empty() {
                words
            } else {
                if !words.is_empty() {
                    notes.insert(question.text.clone(), json!({"notes": words}));
                }
                picked.join(", ")
            }
        };
        if !answer.is_empty() {
            answered.insert(question.text.clone(), json!(answer));
        } else if mark_unanswered {
            notes.insert(question.text.clone(), json!({"unanswered": "skipped"}));
        }
    }
    let mut input = input.clone();
    if let Some(fields) = input.as_object_mut() {
        fields.insert("answers".into(), Value::Object(answered));
        if !notes.is_empty() {
            fields.insert("annotations".into(), Value::Object(notes));
        }
    }
    input
}

/// Several choices as one answer, as Claude Code's own dialog joins them.
fn join_choices(items: &[&str]) -> String {
    items
        .iter()
        .map(|item| {
            if item.contains(", ") || item.contains('"') {
                Value::from(*item).to_string()
            } else {
                (*item).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Value {
        json!({"questions": [
            {"question": "Which library?", "header": "Library", "multiSelect": false, "options": [
                {"label": "chrono", "description": "Full-featured"},
                {"label": "time", "description": "Small"},
            ]},
            {"question": "Which features?", "header": "Features", "multiSelect": true, "options": [
                {"label": "serde", "description": "Serialization"},
                {"label": "tz", "description": "Time zones"},
            ]},
            {"question": "Anything else?", "header": "More", "multiSelect": false, "options": [
                {"label": "No", "description": "Nothing"},
                {"label": "Yes", "description": "Something"},
            ]},
            {"header": "no question"},
        ]})
    }

    #[test]
    fn the_tool_s_questions_are_asked_with_its_options_and_room_for_the_user_s_words() {
        let questions = tool_questions(&input());
        assert_eq!(questions.len(), 3);
        assert_eq!(
            questions[0],
            Question {
                header: Some("Library".into()),
                text: "Which library?".into(),
                options: vec![
                    QuestionOption {
                        label: "chrono".into(),
                        description: Some("Full-featured".into()),
                    },
                    QuestionOption {
                        label: "time".into(),
                        description: Some("Small".into()),
                    },
                ],
                multiple: false,
                free: true,
            }
        );
        assert!(questions[1].multiple);
    }

    #[test]
    fn answers_go_into_the_tool_s_input_by_question_as_claude_code_s_dialog_writes_them() {
        let input = input();
        let questions = tool_questions(&input);
        let answers = vec![
            vec!["time".to_owned(), "but only for now".to_owned()],
            vec![
                "serde".to_owned(),
                "tz".to_owned(),
                "Redis, not Memcached".to_owned(),
            ],
            Vec::new(),
        ];
        let answered = tool_answers(&input, &questions, &answers, true);
        assert_eq!(answered["questions"], input["questions"]);
        assert_eq!(
            answered["answers"],
            json!({
                "Which library?": "time",
                "Which features?": r#"serde, tz, "Redis, not Memcached""#,
            })
        );
        assert_eq!(
            answered["annotations"],
            json!({
                "Which library?": {"notes": "but only for now"},
                "Anything else?": {"unanswered": "skipped"},
            })
        );
        // Words alone answer a single choice; nothing is marked where Claude
        // Code does not take the mark.
        let answers = vec![vec!["neither".to_owned()]];
        let answered = tool_answers(&input, &questions, &answers, false);
        assert_eq!(answered["answers"], json!({"Which library?": "neither"}));
        assert!(answered.get("annotations").is_none());
    }
}
