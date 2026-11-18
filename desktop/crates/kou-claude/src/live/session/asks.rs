//! What Claude Code asks the user, beside leave to run a call (`calls`):
//! the questions of its `AskUserQuestion` tool and an MCP server's form
//! (`elicitation`); and, for every kind, how a request is answered, taken
//! back by Claude Code (`control_cancel_request`), or answered for the user
//! when the turn ends.

use super::{
    super::ask::{self, Field},
    Asked, OpenPermission, Session,
    wire::{self, text_of},
};
use kou_thread::{
    Level, PermissionKind, PermissionOption, PermissionRequest, Question, ToolCall, ToolContent,
    ToolKind, ToolPatch, ToolStatus, Update,
};
use serde_json::{Value, json};

/// What a call the user refused is told.
const REFUSED: &str = "User refused permission to run tool";
/// What `AskUserQuestion` is told when the user leaves its questions.
const SKIPPED: &str = "The user chose not to answer.";
/// The option that leaves the questions of `AskUserQuestion` unanswered.
const SKIP: &str = "skip";
/// The options of an MCP server's form: to fill it in (where it has no
/// fields, only this), and to refuse it.
const ACCEPT: &str = "accept";
const DECLINE: &str = "decline";
/// Claude Code says it takes the mark of a question left unanswered.
const UNANSWERED_MARKS: &str = "ask_user_question_unanswered_v1";

fn option(id: &str, label: &str, kind: PermissionKind) -> PermissionOption {
    PermissionOption {
        id: id.into(),
        label: label.into(),
        kind,
    }
}

/// MCP's `ElicitResult` of a form not filled in: refused by the user
/// (`decline`), or never answered (`cancel`).
fn unfilled(action: &str) -> Value {
    json!({ "action": action })
}

impl Session {
    /// The user said stop: nothing is asked any more.
    pub(super) fn stopped(&self) -> bool {
        self.draining || self.turn.as_ref().is_some_and(|turn| turn.cancelled)
    }

    /// Tell the interface of a request and keep it until it is answered.
    pub(super) fn open(
        &mut self,
        request_id: String,
        asked: Asked,
        tool: ToolCall,
        options: Vec<PermissionOption>,
        questions: Vec<Question>,
    ) {
        let request = PermissionRequest {
            id: self.next_permission,
            tool,
            options,
            questions,
        };
        self.next_permission += 1;
        self.permissions.push(OpenPermission {
            request_id,
            asked,
            request: request.clone(),
        });
        self.send(Update::Permission(request));
    }

    /// `AskUserQuestion` asks leave to run: its questions are asked of the
    /// user, on its own call.
    pub(super) fn ask_questions(
        &mut self,
        id: String,
        tool_use: String,
        input: Value,
        tool: ToolCall,
    ) {
        let questions = ask::tool_questions(&input);
        if questions.is_empty() {
            // As the adapter answers it.
            let why = "AskUserQuestion called with no valid questions.";
            self.link
                .write(&wire::success(&id, wire::deny(&tool_use, why)));
            return;
        }
        let options = vec![option(SKIP, "Skip", PermissionKind::RejectOnce)];
        self.open(
            id,
            Asked::Questions { tool_use, input },
            tool,
            options,
            questions,
        );
    }

    /// An MCP server asks the user to fill in a form (`elicitation`): shown
    /// on a call of its own, a question a field. A page to open (its `url`
    /// mode) cannot be shown here and is declined.
    pub(super) fn on_elicitation(&mut self, id: String, request: &Value) {
        let server = text_of(request, "mcp_server_name")
            .unwrap_or("An MCP server")
            .to_owned();
        if self.stopped() {
            self.link.write(&wire::success(&id, unfilled("cancel")));
            return;
        }
        if text_of(request, "mode").is_some_and(|mode| mode != "form") {
            let what = match text_of(request, "url") {
                Some(url) => format!("{server} asked to open {url}"),
                None => format!("{server} asked for something"),
            };
            self.notice(
                Level::Warning,
                format!("{what}, which this app cannot show, so it was declined."),
            );
            self.link.write(&wire::success(&id, unfilled(DECLINE)));
            return;
        }
        // Asked with no turn open: it opens one of Claude Code's own.
        self.own_turn_starts();
        self.last_chunk = None;
        let schema = request.get("requested_schema").unwrap_or(&Value::Null);
        let (fields, questions): (Vec<Field>, Vec<Question>) =
            ask::form(schema, request.get("property_order"))
                .into_iter()
                .unzip();
        let name = text_of(request, "display_name").unwrap_or(&server);
        let call = format!("elicitation-{}", self.next_permission);
        let tool = ToolCall {
            id: call.clone(),
            title: format!("{name} asks"),
            kind: ToolKind::Other,
            status: ToolStatus::Pending,
            content: text_of(request, "message")
                .map(|message| ToolContent::Text(message.to_owned()))
                .into_iter()
                .collect(),
            raw_input: Some(request.clone()),
            ..ToolCall::default()
        };
        let decline = option(DECLINE, "Decline", PermissionKind::RejectOnce);
        let options = if questions.is_empty() {
            // Nothing to fill in: it asks only for a yes.
            vec![option(ACCEPT, "Accept", PermissionKind::AllowOnce), decline]
        } else {
            vec![decline]
        };
        self.open(id, Asked::Form { call, fields }, tool, options, questions);
    }

    /// The form shown as call `call` is done with: filled in (its answers
    /// in words added to what it showed), or not.
    fn settle_form(&mut self, call: &str, shown: &[ToolContent], answered: Option<String>) {
        let status = if answered.is_some() {
            ToolStatus::Done
        } else {
            ToolStatus::Cancelled
        };
        let content = answered
            .filter(|summary| !summary.is_empty())
            .map(|summary| {
                let mut content = shown.to_vec();
                content.push(ToolContent::Text(summary));
                content
            });
        self.send(Update::ToolPatch(ToolPatch {
            id: call.to_owned(),
            status: Some(status),
            content,
            ..ToolPatch::default()
        }));
    }

    /// The open request of the interface's id `request`, by its place.
    fn find_open(&self, request: u64) -> Option<usize> {
        let found = self
            .permissions
            .iter()
            .position(|open| open.request.id == request);
        if found.is_none() {
            log::debug!("an answer to request {request}, which is not open");
        }
        found
    }

    /// The request at `index` is still open: the interface, which took it
    /// for answered, is told again.
    fn ask_again(&mut self, index: usize) {
        let again = self.permissions[index].request.clone();
        self.send(Update::Permission(again));
    }

    /// The user chose option `option` of request `request`.
    pub(super) fn permit(&mut self, request: u64, option: &str) {
        let Some(index) = self.find_open(request) else {
            return;
        };
        let offered = self.permissions[index]
            .request
            .options
            .iter()
            .any(|offered| offered.id == option);
        if !offered {
            // Never pass on an answer Claude Code was not offered.
            log::warn!("request {request} has no option {option:?}");
            return self.ask_again(index);
        }
        let open = self.permissions.remove(index);
        let answer = match &open.asked {
            Asked::Leave {
                tool_use,
                input,
                suggestions,
            } => match option {
                wire::ALLOW => wire::allow(input, tool_use, None),
                wire::REJECT => wire::deny(tool_use, REFUSED),
                always => {
                    let rule = wire::suggestion_of(always).and_then(|at| suggestions.get(at));
                    wire::allow(input, tool_use, rule)
                }
            },
            Asked::Questions { tool_use, .. } => wire::deny(tool_use, SKIPPED),
            Asked::Form { call, .. } => {
                let accepted = option == ACCEPT;
                self.settle_form(call, &open.request.tool.content, accepted.then(String::new));
                if accepted {
                    json!({"action": "accept", "content": {}})
                } else {
                    unfilled(DECLINE)
                }
            }
        };
        self.link.write(&wire::success(&open.request_id, answer));
    }

    /// The user answered the questions of request `request`.
    pub(super) fn answer(&mut self, request: u64, answers: Vec<Vec<String>>) {
        let Some(index) = self.find_open(request) else {
            return;
        };
        let open = &self.permissions[index];
        let reply = match &open.asked {
            Asked::Leave { .. } => {
                log::warn!("request {request} asks no questions to answer");
                return self.ask_again(index);
            }
            Asked::Questions { tool_use, input } => {
                let marks = self
                    .capabilities
                    .iter()
                    .any(|known| known == UNANSWERED_MARKS);
                let input = ask::tool_answers(input, &open.request.questions, &answers, marks);
                wire::allow(&input, tool_use, None)
            }
            Asked::Form { fields, .. } => match ask::form_content(fields, &answers) {
                Ok(content) => json!({"action": "accept", "content": content}),
                Err(why) => {
                    // Asked again, for an answer that fits.
                    self.notice(Level::Warning, why);
                    return self.ask_again(index);
                }
            },
        };
        let open = self.permissions.remove(index);
        if let Asked::Form { call, fields } = &open.asked {
            let summary = ask::form_summary(fields, &answers);
            self.settle_form(call, &open.request.tool.content, Some(summary));
        }
        self.link.write(&wire::success(&open.request_id, reply));
    }

    /// Claude Code takes back a request of its own (`control_cancel_request`):
    /// it was answered elsewhere, or what it was for stopped. It wants no
    /// answer any more.
    pub(super) fn withdraw(&mut self, request_id: &str) {
        let Some(index) = self
            .permissions
            .iter()
            .position(|open| open.request_id == request_id)
        else {
            return;
        };
        let open = self.permissions.remove(index);
        self.send(Update::Withdrawn(open.request.id));
        if let Asked::Form { call, .. } = &open.asked {
            self.settle_form(call, &open.request.tool.content, None);
        }
    }

    /// Answer every open request for the user, who can no longer: a call
    /// is refused with `message` as why, a form is left unfilled.
    pub(super) fn answer_open(&mut self, message: &str) {
        for open in std::mem::take(&mut self.permissions) {
            let answer = match &open.asked {
                Asked::Leave { tool_use, .. } | Asked::Questions { tool_use, .. } => {
                    wire::deny(tool_use, message)
                }
                Asked::Form { call, .. } => {
                    self.settle_form(call, &open.request.tool.content, None);
                    unfilled("cancel")
                }
            };
            self.link.write(&wire::success(&open.request_id, answer));
        }
    }
}
