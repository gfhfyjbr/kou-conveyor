# scratchpad

An example [plugin](../../../docs/plugins.md) that adds a view of its own to
the browser cockpit:

- a **Notes** tab in the rail, with the notes listed there, and a page in
  the stage in place of the session (`layout.view`);
- an address, `#/notes`, a command, `/note [text]`, and a palette item;
- a **hook** on every prompt's way to the agent (`run.request`): a note
  pinned to prompts goes with each;
- a manifest command, `/notes-review`.

Its state outlives its own versions (`cockpit.hot.data`): edit
`web/scratchpad.js` or `web/scratchpad.css` while the cockpit is open, and
the cockpit takes the new version up at once, with the note being edited
still there.

```sh
cp -R path/to/examples/plugins/scratchpad ~/.config/kou-conveyor/plugins/
```

(on macOS, `~/Library/Application Support/kou-conveyor/plugins/`). No reload
is needed: the page shows the Notes tab as soon as the plugin is there.
