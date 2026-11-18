//! Runs the plugins: each in a world of its own inside one interpreter.

use crate::{
    node::Node,
    source::{self, Found, Source, runnable},
};
use rquickjs::{CatchResultExt, Context, Function, Object, Runtime, context::EvalOptions};
use serde::Deserialize;
use serde_json::Value;
use std::{
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

/// What every plugin's script finds in its world.
const KERNEL: &str = include_str!("kernel.js");

/// How much memory all the plugins may hold together.
const MEMORY: usize = 96 << 20;
/// How deep a script's calls may go.
const STACK: usize = 512 << 10;
/// How long a script may take to load, and to draw or answer once.
const LOAD: Duration = Duration::from_millis(1500);
const CALL: Duration = Duration::from_millis(250);

/// Something wrong with a plugin: its manifest, its script, a page it drew.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub plugin: String,
    pub message: String,
}

/// A page a plugin adds.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PageInfo {
    /// The plugin whose page it is.
    #[serde(default)]
    pub plugin: String,
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    /// A page of its own in the app (a row of the rail shows it), rather
    /// than a view the app draws where it has a place for it.
    #[serde(default = "yes")]
    pub page: bool,
}

fn yes() -> bool {
    true
}

/// Something a plugin asks the app to do: its name and what it is given.
pub type Act = (String, Value);

/// A page as last drawn: from what (its input is kept as a hash: a view's
/// may be megabytes of a file), and what came of it.
struct Drawn {
    input: u64,
    state: u64,
    tree: Rc<Node>,
}

fn hash_of(text: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// A plugin that runs.
struct Loaded {
    found: Found,
    context: Context,
    pages: Vec<PageInfo>,
    /// What was last drawn, by page and instance.
    drawn: HashMap<(String, String), Drawn>,
    /// The drawings whose script failed, by page and instance, from what
    /// (input and state), for a drawing that must not be the one before
    /// (see [`Host::render_strict`]).
    failed: HashMap<(String, String), (u64, u64, Problem)>,
}

/// How many drawings a plugin's views are remembered for: past this the
/// lot is let go and drawn anew when next asked for.
const DRAWN: usize = 512;

/// What a plugin's page remembers, kept apart from its script so that it
/// outlives the script being read again.
#[derive(Default)]
struct Kept {
    json: String,
    /// Goes up with each change.
    version: u64,
}

/// Finds, runs and reloads plugins. See the [crate](crate).
pub struct Host {
    sources: Vec<Source>,
    runtime: Runtime,
    /// When the script that runs now is to be stopped.
    deadline: Rc<Cell<Option<Instant>>>,
    plugins: Vec<Loaded>,
    /// What each page keeps, by `plugin/page`.
    kept: HashMap<String, Kept>,
    /// What is wrong now, at most one thing for each plugin.
    problems: Vec<Problem>,
    /// How many times a script ran to draw.
    runs: u64,
}

impl Host {
    /// A host for the plugins of `sources`, all loaded.
    pub fn new(sources: Vec<Source>) -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|error| error.to_string())?;
        runtime.set_memory_limit(MEMORY);
        runtime.set_max_stack_size(STACK);
        let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let until = deadline.clone();
        // A script that runs past its time is stopped where it is.
        runtime.set_interrupt_handler(Some(Box::new(move || {
            until.get().is_some_and(|until| Instant::now() > until)
        })));
        let mut host = Self {
            sources,
            runtime,
            deadline,
            plugins: Vec::new(),
            kept: HashMap::new(),
            problems: Vec::new(),
            runs: 0,
        };
        host.reload();
        Ok(host)
    }

    /// Looks at the sources again and makes what runs match them: a plugin
    /// that is new is loaded, one whose script changed is loaded anew, one
    /// that is gone is let go. A plugin whose new script fails keeps running
    /// as it was, and the failure is a [`Problem`]. Whether anything
    /// changed, the problems included.
    pub fn reload(&mut self) -> bool {
        let (found, mut problems) = source::find(&self.sources);
        let before = self.problems.clone();
        let mut changed = false;
        // Gone: no source has them any more.
        let gone = self.plugins.len();
        self.plugins
            .retain(|loaded| found.iter().any(|plugin| plugin.name == loaded.found.name));
        changed |= gone != self.plugins.len();
        for plugin in found {
            let at = self
                .plugins
                .iter()
                .position(|loaded| loaded.found.name == plugin.name);
            if at.is_some_and(|at| self.plugins[at].found == plugin) {
                // As it runs. (A problem of its last drawing stays said.)
                if let Some(problem) = before.iter().find(|problem| {
                    problem.plugin == plugin.name && problem.message.starts_with("page ")
                }) {
                    problems.push(problem.clone());
                }
                continue;
            }
            match self.load(&plugin) {
                Ok(loaded) => {
                    changed = true;
                    match at {
                        Some(at) => self.plugins[at] = loaded,
                        None => self.plugins.push(loaded),
                    }
                }
                Err(message) => problems.push(Problem {
                    plugin: plugin.name.clone(),
                    message,
                }),
            }
        }
        self.runtime.run_gc();
        changed |= problems != before;
        self.problems = problems;
        changed
    }

    /// Runs a plugin's script in a world of its own.
    fn load(&self, plugin: &Found) -> Result<Loaded, String> {
        let context = Context::full(&self.runtime).map_err(|error| error.to_string())?;
        let pages = self.within(LOAD, || {
            context.with(|ctx| {
                let run = |name: &str, text: &str| {
                    let mut options = EvalOptions::default();
                    options.filename = Some(name.to_owned());
                    ctx.eval_with_options::<(), _>(text, options)
                        .catch(&ctx)
                        .map_err(|error| error.to_string())
                };
                run("kou/kernel.js", KERNEL)?;
                for (name, text) in &plugin.scripts {
                    run(name, &runnable(name, text)?)?;
                }
                let listed: String = kernel(&ctx)?
                    .get::<_, Function>("pages")
                    .and_then(|pages| pages.call(()))
                    .catch(&ctx)
                    .map_err(|error| error.to_string())?;
                serde_json::from_str::<Vec<PageInfo>>(&listed).map_err(|error| error.to_string())
            })
        })?;
        Ok(Loaded {
            pages: pages
                .into_iter()
                .map(|page| PageInfo {
                    plugin: plugin.name.clone(),
                    ..page
                })
                .collect(),
            found: plugin.clone(),
            context,
            drawn: HashMap::new(),
            failed: HashMap::new(),
        })
    }

    /// Runs `call` with `budget` of time: a script still at it then is
    /// stopped, and that is what the error says.
    fn within<T>(
        &self,
        budget: Duration,
        call: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let until = Instant::now() + budget;
        self.deadline.set(Some(until));
        let result = call();
        self.deadline.set(None);
        match result {
            Err(_) if Instant::now() > until => Err(format!(
                "it ran for longer than {} ms and was stopped",
                budget.as_millis()
            )),
            result => result,
        }
    }

    /// The pages plugins add, each id once: a later plugin's page takes the
    /// place of an earlier one's of the same id. (Their views are not pages:
    /// the app draws those where it has a place for them.)
    pub fn pages(&self) -> Vec<PageInfo> {
        let mut pages: Vec<PageInfo> = Vec::new();
        for page in self.plugins.iter().flat_map(|loaded| &loaded.pages) {
            pages.retain(|known| known.id != page.id);
            pages.push(page.clone());
        }
        pages.retain(|page| page.page);
        pages
    }

    /// Whether some plugin has a page or a view called `id`.
    pub fn has(&self, id: &str) -> bool {
        self.owner(id).is_some()
    }

    /// How many times a script has run to draw (not a drawing remembered
    /// from the same input): for a test of how often the scripts run.
    pub fn runs(&self) -> u64 {
        self.runs
    }

    /// What is wrong with the plugins now.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The names of the plugins that run, in the order they were found.
    pub fn running(&self) -> Vec<&str> {
        self.plugins
            .iter()
            .map(|loaded| loaded.found.name.as_str())
            .collect()
    }

    /// The plugin that has page `id`: the last one that added it.
    fn owner(&self, id: &str) -> Option<usize> {
        self.plugins
            .iter()
            .rposition(|loaded| loaded.pages.iter().any(|page| page.id == id))
    }

    /// Draws page `id` for `input` (`{ "theme": ..., "data": ... }`): the
    /// tree its plugin builds. Drawn again from the same input and the same
    /// remembered state, it is the tree built before, and no script runs.
    /// When the script fails, the page as it was last drawn is what comes
    /// back, if it was ever drawn, and the failure is a [`Problem`].
    pub fn render(&mut self, id: &str, input: &Value) -> Result<Rc<Node>, Problem> {
        self.render_as(id, "", input)
    }

    /// The same for a view that is drawn for several things: `instance`
    /// names the one meant, which is drawn from its own input and keeps a
    /// state of its own.
    pub fn render_as(
        &mut self,
        id: &str,
        instance: &str,
        input: &Value,
    ) -> Result<Rc<Node>, Problem> {
        self.draw(id, instance, input, false)
    }

    /// The same for a part of the app's own window, which the app draws
    /// itself when the plugin cannot: a script that fails gives the
    /// failure, not what it drew before, and is not run again for the same
    /// input and state until it is read again.
    pub fn render_strict(
        &mut self,
        id: &str,
        instance: &str,
        input: &Value,
    ) -> Result<Rc<Node>, Problem> {
        self.draw(id, instance, input, true)
    }

    fn draw(
        &mut self,
        id: &str,
        instance: &str,
        input: &Value,
        strict: bool,
    ) -> Result<Rc<Node>, Problem> {
        let Some(at) = self.owner(id) else {
            return Err(Problem {
                plugin: String::new(),
                message: format!("no plugin has a page \"{id}\""),
            });
        };
        let name = self.plugins[at].found.name.clone();
        let key = kept_key(&name, id, instance);
        let kept = self.kept.entry(key).or_default();
        let (state, version) = (
            if kept.json.is_empty() {
                "null".to_owned()
            } else {
                kept.json.clone()
            },
            kept.version,
        );
        let input = input.to_string();
        let given = hash_of(&input);
        let slot = (id.to_owned(), instance.to_owned());
        if strict
            && let Some((input, state, problem)) = self.plugins[at].failed.get(&slot)
            && *input == given
            && *state == version
        {
            return Err(problem.clone());
        }
        if let Some(drawn) = self.plugins[at].drawn.get(&slot)
            && drawn.input == given
            && drawn.state == version
        {
            return Ok(drawn.tree.clone());
        }
        self.runs += 1;
        let built = self.within(CALL, || {
            self.plugins[at].context.with(|ctx| {
                let tree: String = kernel(&ctx)?
                    .get::<_, Function>("render")
                    .and_then(|render| render.call((id, input.as_str(), state.as_str(), instance)))
                    .catch(&ctx)
                    .map_err(|error| error.to_string())?;
                serde_json::from_str::<Node>(&tree).map_err(|error| error.to_string())
            })
        });
        self.problems
            .retain(|problem| !(problem.plugin == name && problem.message.starts_with("page ")));
        match built {
            Ok(tree) => {
                let tree = Rc::new(tree);
                self.plugins[at].failed.remove(&slot);
                if self.plugins[at].drawn.len() >= DRAWN {
                    self.plugins[at].drawn.clear();
                }
                self.plugins[at].drawn.insert(
                    slot,
                    Drawn {
                        input: given,
                        state: version,
                        tree: tree.clone(),
                    },
                );
                Ok(tree)
            }
            Err(message) => {
                let problem = Problem {
                    plugin: name,
                    message: format!("page \"{id}\": {}", message.trim()),
                };
                self.problems.push(problem.clone());
                if strict {
                    let failed = &mut self.plugins[at].failed;
                    if failed.len() >= DRAWN {
                        failed.clear();
                    }
                    failed.insert(slot, (given, version, problem.clone()));
                    return Err(problem);
                }
                match self.plugins[at].drawn.get(&slot) {
                    Some(drawn) => Ok(drawn.tree.clone()),
                    None => Err(problem),
                }
            }
        }
    }

    /// Page `id` as it was last drawn for `instance`, if it was: its
    /// handlers are the ones that answer now.
    pub fn drawn(&self, id: &str, instance: &str) -> Option<Rc<Node>> {
        let at = self.owner(id)?;
        self.plugins[at]
            .drawn
            .get(&(id.to_owned(), instance.to_owned()))
            .map(|drawn| drawn.tree.clone())
    }

    /// Calls the handler `handler` of page `id` as it was last drawn, with
    /// `event`: what the page asks the app to do, in order. What the
    /// handler keeps is remembered for the page's next drawing.
    pub fn dispatch(&mut self, id: &str, handler: u64, event: &Value) -> Vec<Act> {
        self.dispatch_as(id, "", handler, event)
    }

    /// The same for one of the things a view is drawn for.
    pub fn dispatch_as(
        &mut self,
        id: &str,
        instance: &str,
        handler: u64,
        event: &Value,
    ) -> Vec<Act> {
        #[derive(Deserialize)]
        struct Told {
            acts: Vec<Act>,
            state: Option<Value>,
        }
        let Some(at) = self.owner(id) else {
            return Vec::new();
        };
        let name = self.plugins[at].found.name.clone();
        let key = kept_key(&name, id, instance);
        let state = self
            .kept
            .get(&key)
            .map(|kept| kept.json.clone())
            .filter(|json| !json.is_empty())
            .unwrap_or_else(|| "null".to_owned());
        let event = event.to_string();
        let told = self.within(CALL, || {
            self.plugins[at].context.with(|ctx| {
                let told: String = kernel(&ctx)?
                    .get::<_, Function>("dispatch")
                    .and_then(|dispatch| {
                        dispatch.call((
                            id,
                            handler as f64,
                            event.as_str(),
                            state.as_str(),
                            instance,
                        ))
                    })
                    .catch(&ctx)
                    .map_err(|error| error.to_string())?;
                serde_json::from_str::<Told>(&told).map_err(|error| error.to_string())
            })
        });
        match told {
            Ok(told) => {
                if let Some(state) = told.state {
                    let kept = self.kept.entry(key).or_default();
                    kept.json = state.to_string();
                    kept.version += 1;
                }
                told.acts
            }
            Err(message) => {
                self.problems.retain(|problem| {
                    !(problem.plugin == name && problem.message.starts_with("page "))
                });
                self.problems.push(Problem {
                    plugin: name,
                    message: format!("page \"{id}\": {}", message.trim()),
                });
                Vec::new()
            }
        }
    }
}

/// What the state of a page, or of one of the things a view is drawn for,
/// is kept under.
fn kept_key(plugin: &str, id: &str, instance: &str) -> String {
    if instance.is_empty() {
        format!("{plugin}/{id}")
    } else {
        format!("{plugin}/{id}#{instance}")
    }
}

/// The kernel's side of the app's talk with a plugin's world.
fn kernel<'js>(ctx: &rquickjs::Ctx<'js>) -> Result<Object<'js>, String> {
    ctx.globals()
        .get::<_, Object>("__kou")
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn plugin(folder: &Path, name: &str, script: &str) {
        let plugin = folder.join(name);
        std::fs::create_dir_all(plugin.join("desktop")).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            json!({"name": name, "desktop": {"script": "desktop/main.js"}}).to_string(),
        )
        .unwrap();
        std::fs::write(plugin.join("desktop/main.js"), script).unwrap();
    }

    const COUNTER: &str = r#"
        kou.page({
          id: "counter",
          title: "Counter",
          icon: "flame",
          render(ctx) {
            const count = (ctx.state && ctx.state.count) || 0;
            return ui.col({ gap: 8, bg: ui.alpha("accent", 0.5) }, [
              "Clicked " + count + " times for " + ctx.data.who,
              null,
              [ui.button("More", { id: "more", onClick: (api) => {
                  api.setState({ count: ((api.state && api.state.count) || 0) + 1 });
                  api.act("say", { count: 1 });
              } })],
            ]);
          },
        });
    "#;

    fn input(who: &str) -> Value {
        json!({"theme": {"accent": "#ff5b1f"}, "data": {"who": who}})
    }

    #[test]
    fn a_page_is_drawn_answers_a_click_and_remembers_through_a_reload() {
        let folder = tempfile::tempdir().unwrap();
        plugin(folder.path(), "counter", COUNTER);
        let mut host = Host::new(vec![Source::Folder(folder.path().into())]).unwrap();
        assert_eq!(host.problems(), []);
        assert_eq!(
            host.pages(),
            [PageInfo {
                plugin: "counter".into(),
                id: "counter".into(),
                title: "Counter".into(),
                icon: Some("flame".into()),
                page: true,
            }]
        );
        // Text is an element, nothing stands for `null`, nested lists are
        // one list, and the theme's colours are the script's to work with.
        let tree = host.render("counter", &input("you")).unwrap();
        assert_eq!(tree.kind, "box");
        assert_eq!(tree.text("dir"), Some("col"));
        assert_eq!(tree.text("bg"), Some("#ff5b1f80"));
        assert_eq!(tree.children.len(), 2);
        assert_eq!(tree.said(), "Clicked 0 times for you More");
        // The same input: the tree built before, no script run.
        assert!(Rc::ptr_eq(
            &tree,
            &host.render("counter", &input("you")).unwrap()
        ));
        // A click runs the script's function: it asks the app for something
        // and keeps a count, with which the page is drawn again.
        let more = tree.find("more").unwrap().handler("onClick").unwrap();
        assert_eq!(
            host.dispatch("counter", more, &json!({})),
            [("say".to_owned(), json!({"count": 1}))]
        );
        let tree = host.render("counter", &input("you")).unwrap();
        assert_eq!(tree.said(), "Clicked 1 times for you More");
        // The script changes on disk: its new version draws, with what the
        // old one's page had kept.
        plugin(
            folder.path(),
            "counter",
            &COUNTER.replace("Clicked ", "Pressed "),
        );
        assert!(host.reload());
        assert!(!host.reload(), "nothing changed since");
        let tree = host.render("counter", &input("me")).unwrap();
        assert_eq!(tree.said(), "Pressed 1 times for me More");
    }

    #[test]
    fn a_broken_script_leaves_the_plugin_as_it_ran_and_says_where_it_broke() {
        let folder = tempfile::tempdir().unwrap();
        plugin(folder.path(), "counter", COUNTER);
        let mut host = Host::new(vec![Source::Folder(folder.path().into())]).unwrap();
        host.render("counter", &input("you")).unwrap();
        // It no longer parses: the page is still the old script's.
        plugin(
            folder.path(),
            "counter",
            "kou.page({ id: \"counter\",\n  render( {",
        );
        assert!(host.reload());
        let problems = host.problems().to_vec();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].plugin, "counter");
        assert!(
            problems[0].message.contains("counter/desktop/main.js:2"),
            "{}",
            problems[0].message
        );
        assert_eq!(
            host.render("counter", &input("you")).unwrap().said(),
            "Clicked 0 times for you More"
        );
        // It throws while drawing: what was drawn last is what shows, and
        // the problem names the page.
        plugin(
            folder.path(),
            "counter",
            "kou.page({ id: \"counter\", render(ctx) { return ctx.data.nobody.home; } });",
        );
        assert!(host.reload());
        assert_eq!(host.problems(), []);
        let failed = host.render("counter", &input("you")).unwrap_err();
        assert!(
            failed.message.starts_with("page \"counter\": "),
            "{failed:?}"
        );
        assert_eq!(host.problems(), [failed]);
        // Mended, it draws and nothing is wrong.
        plugin(folder.path(), "counter", COUNTER);
        assert!(host.reload());
        assert_eq!(host.problems(), []);
        assert!(host.render("counter", &input("you")).is_ok());
        // Gone from disk, it is gone from the app.
        std::fs::remove_dir_all(folder.path().join("counter")).unwrap();
        assert!(host.reload());
        assert_eq!(host.pages(), []);
        assert!(host.render("counter", &input("you")).is_err());
    }

    #[test]
    fn a_later_source_s_plugin_takes_the_place_of_an_earlier_one_of_its_name() {
        const BUILT_IN: &[crate::File] = &[
            (
                "usage",
                "plugin.json",
                r#"{"name": "usage", "desktop": {"script": "desktop/usage.js"}}"#,
            ),
            (
                "usage",
                "desktop/usage.js",
                r#"kou.page({ id: "usage", title: "Usage", render: () => ui.text("the app's own") });"#,
            ),
            // One with nothing for the desktop app is no plugin of its.
            (
                "web-only",
                "plugin.json",
                r#"{"name": "web-only", "web": {"script": "x.js"}}"#,
            ),
        ];
        let folder = tempfile::tempdir().unwrap();
        let mut host = Host::new(vec![
            Source::Embedded(BUILT_IN),
            Source::Folder(folder.path().into()),
        ])
        .unwrap();
        assert_eq!(host.running(), ["usage"]);
        assert_eq!(
            host.render("usage", &json!({})).unwrap().said(),
            "the app's own"
        );
        // The user's `usage` replaces it; taken away, the app's own is back.
        plugin(
            folder.path(),
            "usage",
            r#"kou.page({ id: "usage", title: "Mine", render: () => ui.text("the user's") });"#,
        );
        assert!(host.reload());
        assert_eq!(host.running(), ["usage"]);
        assert_eq!(host.pages()[0].title, "Mine");
        assert_eq!(
            host.render("usage", &json!({})).unwrap().said(),
            "the user's"
        );
        std::fs::remove_dir_all(folder.path().join("usage")).unwrap();
        assert!(host.reload());
        assert_eq!(
            host.render("usage", &json!({})).unwrap().said(),
            "the app's own"
        );
        // A manifest that cannot be read is a problem, and stops nothing.
        std::fs::create_dir_all(folder.path().join("bad")).unwrap();
        std::fs::write(folder.path().join("bad/plugin.json"), "{ not json").unwrap();
        assert!(host.reload());
        assert_eq!(host.problems().len(), 1);
        assert_eq!(host.problems()[0].plugin, "bad");
        assert_eq!(host.running(), ["usage"]);
    }

    #[test]
    fn a_script_that_never_ends_or_takes_all_the_memory_is_stopped() {
        let folder = tempfile::tempdir().unwrap();
        plugin(folder.path(), "counter", COUNTER);
        plugin(
            folder.path(),
            "spin",
            r#"kou.page({ id: "spin", render() { for (;;) {} } });
               kou.page({ id: "hog", render() {
                 const kept = []; for (;;) kept.push(new Array(1 << 20).fill(7));
               } });"#,
        );
        let mut host = Host::new(vec![Source::Folder(folder.path().into())]).unwrap();
        let began = Instant::now();
        let stopped = host.render("spin", &json!({})).unwrap_err();
        assert!(
            stopped.message.contains("ran for longer than"),
            "{stopped:?}"
        );
        assert!(began.elapsed() < Duration::from_secs(5));
        assert!(host.render("hog", &json!({})).is_err());
        // The others go on as if nothing had happened.
        assert_eq!(
            host.render("counter", &input("you")).unwrap().said(),
            "Clicked 0 times for you More"
        );
        // One that never finishes loading is not loaded at all.
        plugin(folder.path(), "loop", "for (;;) {}");
        let began = Instant::now();
        assert!(host.reload());
        assert!(began.elapsed() < Duration::from_secs(10));
        assert!(!host.running().contains(&"loop"));
        assert!(
            host.problems()
                .iter()
                .any(|problem| problem.plugin == "loop" && problem.message.contains("stopped"))
        );
    }

    #[test]
    fn a_plugin_s_scripts_are_one_program_and_a_view_is_drawn_for_each_thing_apart() {
        let folder = tempfile::tempdir().unwrap();
        let plugin = folder.path().join("cards");
        std::fs::create_dir_all(plugin.join("shared")).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            json!({"name": "cards", "desktop": {"scripts": ["shared/data.js", "main.js"]}})
                .to_string(),
        )
        .unwrap();
        // The first is written as a module, as the browser cockpit's are.
        std::fs::write(
            plugin.join("shared/data.js"),
            "export const twice = (x) => x * 2;\n",
        )
        .unwrap();
        std::fs::write(
            plugin.join("main.js"),
            r#"kou.view({ id: "card", render: (ctx) => ui.col({}, [
                 ui.text(ctx.data.name + ": " + twice((ctx.state && ctx.state.n) || 1)),
                 ui.button("More", { id: "more", onClick: (api, event) =>
                   api.setState({ n: ((api.state && api.state.n) || 1) + event.by }) }),
               ]) });"#,
        )
        .unwrap();
        let mut host = Host::new(vec![Source::Folder(folder.path().into())]).unwrap();
        assert_eq!(host.problems(), []);
        // A view is no page of the app's, and is there to be drawn.
        assert_eq!(host.pages(), []);
        assert!(host.has("card") && !host.has("nothing"));
        let draw = |host: &mut Host, instance: &str, name: &str| {
            host.render_as("card", instance, &json!({"data": {"name": name}}))
                .unwrap()
        };
        assert_eq!(draw(&mut host, "a", "first").said(), "first: 2 More");
        assert_eq!(draw(&mut host, "b", "second").said(), "second: 2 More");
        // What one of them keeps is its own, and its handlers are too.
        let more = draw(&mut host, "b", "second")
            .find("more")
            .unwrap()
            .handler("onClick")
            .unwrap();
        host.dispatch_as("card", "b", more, &json!({"by": 4}));
        assert_eq!(draw(&mut host, "a", "first").said(), "first: 2 More");
        assert_eq!(draw(&mut host, "b", "second").said(), "second: 10 More");
        // A script that imports is refused, and says which file to list.
        std::fs::write(
            plugin.join("main.js"),
            "import { twice } from './shared/data.js';\n",
        )
        .unwrap();
        assert!(host.reload());
        assert!(
            host.problems()[0]
                .message
                .contains("cards/main.js:1: a plugin's script cannot import"),
            "{:?}",
            host.problems()
        );
        // The plugin goes on as it ran.
        assert_eq!(draw(&mut host, "b", "second").said(), "second: 10 More");
    }

    #[test]
    fn a_part_of_the_app_s_own_is_not_drawn_from_a_failed_script_nor_tried_again_for_the_same_input()
     {
        let folder = tempfile::tempdir().unwrap();
        plugin(
            folder.path(),
            "chrome",
            r#"let runs = 0;
               kou.view({ id: "bar", render(ctx) {
                 runs += 1;
                 if (ctx.data.fail) throw new Error("no " + runs);
                 return ui.text("runs " + runs);
               } });"#,
        );
        let mut host = Host::new(vec![Source::Folder(folder.path().into())]).unwrap();
        let input = |fail: bool, n: u32| json!({"data": {"fail": fail, "n": n}});
        let draw =
            |host: &mut Host, fail: bool, n: u32| host.render_strict("bar", "", &input(fail, n));
        assert_eq!(draw(&mut host, false, 1).unwrap().said(), "runs 1");
        // It fails: what comes back is the failure, where a page would be
        // given what it drew before.
        let failed = draw(&mut host, true, 1).unwrap_err();
        assert!(failed.message.contains("no 2"), "{failed:?}");
        assert_eq!(host.problems(), std::slice::from_ref(&failed));
        // Asked again for the same, the script is not run again.
        assert_eq!(draw(&mut host, true, 1).unwrap_err(), failed);
        // For something new it is, and once it draws nothing is wrong.
        assert!(
            draw(&mut host, true, 2)
                .unwrap_err()
                .message
                .contains("no 3")
        );
        assert_eq!(draw(&mut host, false, 2).unwrap().said(), "runs 4");
        assert_eq!(host.problems(), []);
    }

    #[test]
    fn a_script_reaches_nothing_outside_its_world() {
        let folder = tempfile::tempdir().unwrap();
        plugin(
            folder.path(),
            "probe",
            r#"kou.page({ id: "probe", render: () => ui.text([
                 typeof require, typeof process, typeof fetch, typeof std, typeof os,
                 typeof XMLHttpRequest, typeof Deno,
               ].join(",")) });"#,
        );
        // A script in a plugin's manifest may not name a file outside it.
        std::fs::write(
            folder.path().join("secret.js"),
            "kou.page({id:'x',render(){}})",
        )
        .unwrap();
        std::fs::create_dir_all(folder.path().join("out")).unwrap();
        std::fs::write(
            folder.path().join("out/plugin.json"),
            r#"{"name": "out", "desktop": {"script": "../secret.js"}}"#,
        )
        .unwrap();
        let mut host = Host::new(vec![Source::Folder(folder.path().into())]).unwrap();
        assert_eq!(
            host.render("probe", &json!({})).unwrap().said(),
            ["undefined"; 7].join(",")
        );
        assert_eq!(host.running(), ["probe"]);
        assert!(
            host.problems()
                .iter()
                .any(|problem| problem.plugin == "out")
        );
    }

    #[test]
    fn a_change_in_a_plugins_folder_is_told() {
        let folder = tempfile::tempdir().unwrap();
        plugin(folder.path(), "counter", COUNTER);
        let told = crate::watch(vec![folder.path().into()], Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(120));
        assert!(told.try_recv().is_err(), "nothing changed yet");
        plugin(
            folder.path(),
            "counter",
            &COUNTER.replace("Clicked", "Pressed again"),
        );
        let heard = (0..200).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            told.try_recv().is_ok()
        });
        assert!(heard, "the change was not told");
    }
}
