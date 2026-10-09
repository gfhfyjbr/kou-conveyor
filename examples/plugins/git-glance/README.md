# git-glance

An example [plugin](../../../docs/plugins.md) that uses every part:

- **Tool** `GitGlance` ([glance.sh](glance.sh)): the branch, the changed files
  and the latest commits as JSON; the arguments arrive on standard input.
- **Command** `/review [focus]`: asks the agent to review the uncommitted
  changes.
- **Skill** `commit-message` and **instructions** ([prompt.md](prompt.md)) for
  the system prompt.
- **Web** ([web/plugin.js](web/plugin.js), [web/plugin.css](web/plugin.css)):
  draws `GitGlance` results as a card in the timeline, adds a Git panel to
  the inspector and a review item to the command palette.

Try it in a git repository:

```sh
mkdir -p .harness/plugins && cp -R path/to/examples/plugins/git-glance .harness/plugins/
```

then trust the workspace (the Plugins panel of the browser cockpit, or
`/plugins trust`), or copy it into your user plugins directory instead, where
it runs everywhere.
