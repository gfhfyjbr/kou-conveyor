//! Theme state, validation, persistence and native text editing for the JS UI.
use crate::shell::Shell;
use gpui::{
    App, ClipboardItem, Context, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use kou_canvas::field::{Field, Outcome};
use kou_ui::{
    Theme,
    themes::{MAX_THEMES, Preset, SavedTheme, Selection, ThemeDocument, parse_hex},
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Page {
    #[default]
    Appearance,
    General,
    Accounts,
    Shortcuts,
}
impl Page {
    pub const ALL: [Self; 4] = [
        Self::Appearance,
        Self::General,
        Self::Accounts,
        Self::Shortcuts,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::General => "general",
            Self::Accounts => "accounts",
            Self::Shortcuts => "shortcuts",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Input {
    Name,
    Color,
    Link,
}
struct TextEdit {
    kind: Input,
    field: Field,
}
pub(crate) struct Draft {
    pub document: ThemeDocument,
    pub id: Option<String>,
}
pub(crate) struct Appearance {
    pub page: Page,
    pub draft: Option<Draft>,
    focus: FocusHandle,
    input: Option<TextEdit>,
    token: &'static str,
    pub message: Option<String>,
    problem: Option<String>,
    pub importing: bool,
}
impl Appearance {
    pub fn new(cx: &mut App) -> Self {
        Self {
            page: Page::default(),
            draft: None,
            focus: cx.focus_handle(),
            input: None,
            token: "accent",
            message: None,
            problem: None,
            importing: false,
        }
    }
}

impl Shell {
    pub(crate) fn appearance_theme(&self, system_dark: bool) -> Theme {
        match &self.store.theme {
            Selection::System => Theme::new(system_dark).accented(self.store.accent_rgb()),
            Selection::Preset(preset) => {
                preset.document().theme().accented(self.store.accent_rgb())
            }
            Selection::Custom(id) => self
                .store
                .themes
                .iter()
                .find(|saved| &saved.id == id && saved.theme.validate().is_ok())
                .map(|saved| saved.theme.theme().accented(self.store.accent_rgb()))
                .unwrap_or_else(|| Preset::Charcoal.document().theme()),
        }
    }
    fn active_document(&self, theme: &Theme) -> ThemeDocument {
        if let Selection::Custom(id) = &self.store.theme
            && let Some(saved) = self.store.themes.iter().find(|saved| &saved.id == id)
        {
            let mut doc = saved.theme.clone();
            doc.colors = theme.palette;
            return doc;
        }
        let base = match self.store.theme {
            Selection::Preset(preset) => preset,
            _ if !theme.dark => Preset::White,
            _ => Preset::AmoledBlack,
        };
        let mut doc = base.document();
        doc.colors = theme.palette;
        doc
    }
    fn apply_appearance(&mut self, window: &Window, cx: &mut Context<Self>) {
        let dark = Theme::for_appearance(window.appearance()).dark;
        cx.set_global(self.themed(dark));
        self.retheme(cx);
        cx.refresh_windows();
        self.save(cx);
    }
    pub(crate) fn choose_theme(
        &mut self,
        selection: Selection,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.store.theme = selection;
        self.store.accent = None;
        self.appearance.draft = None;
        self.appearance.input = None;
        self.appearance.problem = None;
        self.appearance.message = None;
        self.appearance.importing = false;
        self.apply_appearance(window, cx);
    }
    fn edit_theme(&mut self, copy: bool, cx: &mut Context<Self>) {
        let mut document = self.active_document(kou_ui::theme(cx));
        let id = match &self.store.theme {
            Selection::Custom(id) if !copy => Some(id.clone()),
            _ => None,
        };
        if id.is_none() {
            document.name = format!("{} custom", document.name);
        }
        self.appearance.draft = Some(Draft { document, id });
        self.appearance.input = None;
        self.appearance.problem = None;
        self.appearance.message = None;
        cx.notify();
    }
    fn cancel_theme(&mut self, cx: &mut Context<Self>) {
        self.appearance.draft = None;
        self.appearance.input = None;
        self.appearance.problem = None;
        self.appearance.importing = false;
        cx.notify();
    }
    fn save_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.commit_theme_input(cx) {
            return;
        }
        let Some(draft) = &self.appearance.draft else {
            return;
        };
        if let Err(error) = draft.document.validate() {
            self.appearance.problem = Some(error);
            cx.notify();
            return;
        }
        if draft.id.is_none() && self.store.themes.len() >= MAX_THEMES {
            self.appearance.problem =
                Some("Your library is full. Delete a saved theme before adding another.".into());
            cx.notify();
            return;
        }
        let document = draft.document.clone();
        let id = draft.id.clone().unwrap_or_else(|| {
            let mut n = 1;
            while self
                .store
                .themes
                .iter()
                .any(|saved| saved.id == format!("theme-{n}"))
            {
                n += 1;
            }
            format!("theme-{n}")
        });
        if let Some(saved) = self.store.themes.iter_mut().find(|saved| saved.id == id) {
            saved.theme = document;
        } else {
            self.store.themes.push(SavedTheme {
                id: id.clone(),
                theme: document,
            });
        }
        self.choose_theme(Selection::Custom(id), window, cx);
        self.appearance.message = Some("Theme saved and applied.".into());
    }
    fn share_theme(&mut self, cx: &mut Context<Self>) {
        let result = self.active_document(kou_ui::theme(cx)).link();
        match result {
            Ok(link) => {
                cx.write_to_clipboard(ClipboardItem::new_string(link));
                self.appearance.message = Some("Theme link copied. Send it to someone with kou, or import it on another computer.".into());
            }
            Err(error) => self.appearance.problem = Some(error),
        }
        cx.notify();
    }
    pub(crate) fn import_theme_link(&mut self, link: &str, cx: &mut Context<Self>) {
        self.show_settings(cx);
        self.appearance.page = Page::Appearance;
        match ThemeDocument::from_link(link) {
            Ok(document) => {
                self.appearance.draft = Some(Draft { document, id: None });
                self.appearance.importing = false;
                self.appearance.input = None;
                self.appearance.problem = None;
                self.appearance.message =
                    Some("Shared theme preview. Save & apply to add it to your library.".into());
            }
            Err(error) => {
                self.appearance.problem = Some(error);
            }
        }
        cx.notify();
    }
    fn start_theme_input(&mut self, kind: Input, window: &mut Window, cx: &mut Context<Self>) {
        if !self.commit_theme_input(cx) {
            return;
        }
        let text = match kind {
            Input::Name => self
                .appearance
                .draft
                .as_ref()
                .map(|d| d.document.name.clone())
                .unwrap_or_default(),
            Input::Color => self
                .appearance
                .draft
                .as_ref()
                .map(|d| {
                    format!(
                        "#{:06X}",
                        d.document.colors.get(self.appearance.token).unwrap()
                    )
                })
                .unwrap_or_default(),
            Input::Link => String::new(),
        };
        self.appearance.input = Some(TextEdit {
            kind,
            field: Field::new(&text, false),
        });
        window.focus(&self.appearance.focus, cx);
        cx.notify();
    }
    fn commit_theme_input(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(edit) = &self.appearance.input else {
            return true;
        };
        let value = edit.field.text().trim().to_owned();
        match edit.kind {
            Input::Name => {
                if value.is_empty()
                    || value.chars().count() > 64
                    || value.chars().any(char::is_control)
                {
                    self.appearance.problem = Some("Use a theme name of 1–64 characters.".into());
                    cx.notify();
                    return false;
                }
                if let Some(draft) = &mut self.appearance.draft {
                    draft.document.name = value;
                }
            }
            Input::Color => {
                let Some(color) = parse_hex(&value) else {
                    self.appearance.problem =
                        Some("Enter six hex digits, for example #8090A0.".into());
                    cx.notify();
                    return false;
                };
                if let Some(draft) = &mut self.appearance.draft {
                    draft.document.colors.set(self.appearance.token, color);
                }
            }
            Input::Link => {
                if value.is_empty() {
                    self.appearance.problem = Some("Paste a theme link first.".into());
                    cx.notify();
                    return false;
                }
                self.import_theme_link(&value, cx);
                return self.appearance.problem.is_none();
            }
        }
        self.appearance.input = None;
        self.appearance.problem = None;
        true
    }
    fn theme_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(edit) = &mut self.appearance.input else {
            return;
        };
        match edit.field.key(&event.keystroke, clipboard) {
            Outcome::Commit => {
                self.commit_theme_input(cx);
            }
            Outcome::Cancel => {
                self.appearance.input = None;
                self.appearance.problem = None;
                window.focus(&self.focus, cx);
            }
            Outcome::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Outcome::Edited => {
                self.appearance.problem = None;
                if edit.kind == Input::Color
                    && let Some(color) = parse_hex(edit.field.text())
                    && let Some(draft) = &mut self.appearance.draft
                {
                    draft.document.colors.set(self.appearance.token, color);
                }
            }
            Outcome::Moved => {}
            Outcome::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    fn theme_field(
        &self,
        kind: Input,
        value: String,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let key = match kind {
            Input::Name => "theme-name",
            Input::Color => "theme-color",
            Input::Link => "theme-link",
        };
        let edit = self
            .appearance
            .input
            .as_ref()
            .filter(|edit| edit.kind == kind);
        let base = div()
            .id(key)
            .debug_selector(move || key.into())
            .size_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .tab_index(0)
            .on_click(
                cx.listener(move |this, _, window, cx| this.start_theme_input(kind, window, cx)),
            );
        match edit {
            Some(edit) => base
                .track_focus(&self.appearance.focus)
                .on_key_down(cx.listener(Self::theme_key))
                .child(kou_canvas::field_lines(key, &edit.field, theme, px(18.)))
                .into_any_element(),
            None => base
                .child(div().truncate().child(SharedString::from(value)))
                .into_any_element(),
        }
    }
    pub(crate) fn appearance_settings(
        &self,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        use serde_json::json;
        let selected = &self.store.theme;
        let draft = self.appearance.draft.as_ref();
        let data = json!({
            "system": *selected == Selection::System,
            "presets": Preset::ALL.map(|preset| {
                let mut doc = serde_json::to_value(preset.document()).unwrap();
                doc["selected"] = json!(*selected == Selection::Preset(preset));
                doc
            }),
            "saved": self.store.themes.iter().map(|saved| json!({
                "id": saved.id, "theme": saved.theme,
                "selected": *selected == Selection::Custom(saved.id.clone()),
            })).collect::<Vec<_>>(),
            "draft": draft.map(|d| json!({ "document": d.document, "id": d.id })),
            "token": self.appearance.token,
            "editing": self.appearance.input.as_ref().map(|edit| match edit.kind {
                Input::Name => "name", Input::Color => "color", Input::Link => "link",
            }),
            "importing": self.appearance.importing,
            "message": self.appearance.message, "problem": self.appearance.problem,
            "lowContrast": draft.is_some_and(|draft| {
                let p = draft.document.colors;
                kou_ui::contrast(p.fg, p.pane) < 4.5 || kou_ui::contrast(p.fg, p.raised) < 4.5
                    || kou_ui::contrast(p.primary_ink, p.primary) < 4.5
            }),
        });
        self.core_chrome(
            "appearance",
            data,
            || {
                if let Some(draft) = draft {
                    vec![
                        (
                            "name",
                            self.theme_field(Input::Name, draft.document.name.clone(), theme, cx),
                        ),
                        (
                            "color",
                            self.theme_field(
                                Input::Color,
                                format!(
                                    "#{:06X}",
                                    draft.document.colors.get(self.appearance.token).unwrap()
                                ),
                                theme,
                                cx,
                            ),
                        ),
                    ]
                } else if self.appearance.importing {
                    vec![(
                        "link",
                        self.theme_field(Input::Link, "Paste theme link".into(), theme, cx),
                    )]
                } else {
                    Vec::new()
                }
            },
            theme,
            cx,
        )
    }

    pub(crate) fn appearance_act(
        &mut self,
        name: &str,
        given: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let id = given.get("id").and_then(serde_json::Value::as_str);
        match name {
            "settings.page" => {
                if let Some(page) = Page::ALL.into_iter().find(|page| {
                    Some(page.id()) == given.get("page").and_then(serde_json::Value::as_str)
                }) {
                    if self.commit_theme_input(cx) {
                        self.appearance.page = page;
                    }
                }
            }
            "theme.select" => {
                let selection =
                    if given.get("system").and_then(serde_json::Value::as_bool) == Some(true) {
                        Some(Selection::System)
                    } else if let Some(id) =
                        id.filter(|id| self.store.themes.iter().any(|saved| saved.id == *id))
                    {
                        Some(Selection::Custom(id.into()))
                    } else {
                        given
                            .get("preset")
                            .and_then(|p| serde_json::from_value::<Preset>(p.clone()).ok())
                            .map(Selection::Preset)
                    };
                if let Some(selection) = selection {
                    self.choose_theme(selection, window, cx);
                }
            }
            "theme.create" => self.edit_theme(true, cx),
            "theme.edit" => {
                if let Some(saved) = self
                    .store
                    .themes
                    .iter()
                    .find(|saved| Some(saved.id.as_str()) == id)
                {
                    self.appearance.draft = Some(Draft {
                        document: saved.theme.clone(),
                        id: Some(saved.id.clone()),
                    });
                    self.appearance.input = None;
                    self.appearance.problem = None;
                }
            }
            "theme.delete" => {
                if let Some(id) = id {
                    self.store.themes.retain(|saved| saved.id != id);
                    if self
                        .appearance
                        .draft
                        .as_ref()
                        .is_some_and(|d| d.id.as_deref() == Some(id))
                    {
                        self.cancel_theme(cx);
                    }
                    if self.store.theme == Selection::Custom(id.into()) {
                        self.choose_theme(Selection::Preset(Preset::Charcoal), window, cx);
                    } else {
                        self.save(cx);
                    }
                }
            }
            "theme.share" => self.share_theme(cx),
            "theme.import" => {
                self.appearance.importing = true;
                self.start_theme_input(Input::Link, window, cx);
            }
            "theme.preview" => {
                self.commit_theme_input(cx);
            }
            "theme.token" => {
                if let Some(token) = kou_ui::Palette::TOKENS.iter().copied().find(|token| {
                    Some(*token) == given.get("token").and_then(serde_json::Value::as_str)
                }) {
                    if self.commit_theme_input(cx) {
                        self.appearance.token = token;
                    }
                }
            }
            "theme.color" => {
                if let Some(color) = given
                    .get("color")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|c| *c <= 0xffffff)
                {
                    self.appearance.input = None;
                    self.appearance.problem = None;
                    if let Some(draft) = &mut self.appearance.draft {
                        draft
                            .document
                            .colors
                            .set(self.appearance.token, color as u32);
                    }
                }
            }
            "theme.save" => self.save_theme(window, cx),
            "theme.cancel" => self.cancel_theme(cx),
            "theme.reset" => {
                if let Some(draft) = &mut self.appearance.draft {
                    draft.document.colors = draft.document.base.palette();
                }
                self.appearance.input = None;
                self.appearance.problem = None;
            }
            _ => return false,
        }
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Store;
    use gpui::TestAppContext;

    #[test]
    fn old_settings_keep_the_system_appearance_and_accent() {
        let store: Store = serde_json::from_value(serde_json::json!({
            "spaces": [], "sessions": [], "panes": [], "selected": null,
            "rail_hidden": false, "accent": "#4d9fe0"
        }))
        .unwrap();
        assert_eq!(store.theme, Selection::System);
        assert_eq!(store.accent_rgb(), Some(0x4d9fe0));
        assert!(store.themes.is_empty());
    }

    #[gpui::test]
    fn shared_themes_are_previewed_saved_and_restored_without_losing_sessions(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| cx.set_global(Theme::new(true)));
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("state.json");
        let mut store = Store::default();
        let space = store.add_space("/fixture/theme-test".into());
        store.add_session(&space).unwrap();
        let (shell, cx) = cx.add_window_view(|_, cx| Shell::new(store, Some(path.clone()), cx));
        let mut doc = Preset::White.document();
        doc.name = "Моя тема".into();
        doc.colors.accent = 0x123456;
        shell.update(cx, |shell, cx| {
            let before = shell.store.theme.clone();
            shell.import_theme_link(&doc.link().unwrap(), cx);
            assert_eq!(shell.store.theme, before, "import is a preview");
            assert!(shell.store.themes.is_empty());
        });
        shell.update_in(cx, |shell, window, cx| shell.save_theme(window, cx));
        let saved = Store::load(&path).unwrap();
        assert_eq!(saved.themes.len(), 1);
        assert_eq!(saved.themes[0].theme, doc);
        assert_eq!(saved.sessions.len(), 1);
        assert_eq!(saved.spaces.len(), 1);
        shell.update(cx, |shell, cx| {
            assert_eq!(
                shell.themed(true).palette,
                doc.colors,
                "explicit theme ignores OS darkness"
            );
            shell.share_theme(cx);
            let link = cx.read_from_clipboard().unwrap().text().unwrap();
            assert_eq!(ThemeDocument::from_link(&link).unwrap(), doc);
            shell.edit_theme(false, cx);
            shell
                .appearance
                .draft
                .as_mut()
                .unwrap()
                .document
                .colors
                .pane = 0x123456;
            shell.cancel_theme(cx);
            assert_eq!(
                shell.themed(false).palette,
                doc.colors,
                "cancel discards edits"
            );
            shell.import_theme_link("kou-theme://v1/broken", cx);
            assert!(shell.appearance.problem.is_some());
            assert_eq!(shell.themed(true).palette, doc.colors);
        });
    }

    #[gpui::test]
    fn invalid_color_cannot_be_saved_and_editing_keeps_the_theme_id(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::new(true)));
        let (shell, cx) = cx.add_window_view(|_, cx| Shell::new(Store::default(), None, cx));
        shell.update_in(cx, |shell, window, cx| {
            shell.edit_theme(true, cx);
            shell.appearance.input = Some(TextEdit {
                kind: Input::Color,
                field: Field::new("#xx", false),
            });
            shell.save_theme(window, cx);
            assert!(shell.store.themes.is_empty());
            assert!(shell.appearance.problem.is_some());
            shell.appearance.input = Some(TextEdit {
                kind: Input::Color,
                field: Field::new("#123abc", false),
            });
            shell.save_theme(window, cx);
            assert_eq!(shell.store.themes.len(), 1);
            let id = shell.store.themes[0].id.clone();
            shell.edit_theme(false, cx);
            shell.appearance.draft.as_mut().unwrap().document.name = "Renamed".into();
            shell.save_theme(window, cx);
            assert_eq!(shell.store.themes.len(), 1);
            assert_eq!(shell.store.themes[0].id, id);
            assert_eq!(shell.store.themes[0].theme.name, "Renamed");
            assert_eq!(shell.store.themes[0].theme.colors.accent, 0x123abc);
        });
    }
    #[gpui::test]
    fn a_theme_can_be_created_with_the_actual_fields_and_buttons(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::new(true));
            crate::shell::bind_keys(cx);
        });
        let mut store = Store::default();
        let space = store.add_space("/fixture/theme-editor".into());
        let session = store.add_session(&space).unwrap();
        store.selected = Some(session);
        let (shell, cx) = cx.add_window_view(|_, cx| Shell::new(store, None, cx));
        cx.simulate_resize(gpui::size(px(1280.), px(1100.)));
        shell.update(cx, |shell, cx| shell.show_settings(cx));
        let draw = |cx: &mut gpui::VisualTestContext| {
            for _ in 0..2 {
                cx.update(|window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                });
            }
        };
        draw(cx);
        let create = cx.debug_bounds("theme-create").unwrap();
        cx.simulate_click(create.center(), gpui::Modifiers::none());
        draw(cx);
        let name = cx.debug_bounds("theme-name").unwrap();
        cx.simulate_click(name.center(), gpui::Modifiers::none());
        cx.simulate_keystrokes("M y space T h e m e enter");
        draw(cx);
        let color = cx.debug_bounds("theme-color").unwrap();
        cx.simulate_click(color.center(), gpui::Modifiers::none());
        cx.simulate_keystrokes("1 2 3 a b c enter");
        draw(cx);
        let save = cx.debug_bounds("theme-save").unwrap();
        cx.simulate_click(save.center(), gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.store.themes.len(), 1);
            assert_eq!(shell.store.themes[0].theme.name, "My Theme");
            assert_eq!(shell.store.themes[0].theme.colors.accent, 0x123abc);
            assert!(shell.appearance.draft.is_none());
        });
    }
}
