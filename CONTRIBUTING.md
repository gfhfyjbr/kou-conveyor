# Contributing

kou-conveyor is a personal project: built for my own daily work first, and
shared in case it is useful to someone else. Fork it and change whatever you
like; that is what it is here for.

## Issues

Questions, bug reports and ideas are welcome as
[issues](https://github.com/gfhfyjbr/kou-conveyor/issues). For a bug, say what
you did, what you expected and what happened instead, with the cockpit
(browser, terminal or iPhone), the system and the model, and the part of the
log that shows it, if there is one.

## Pull requests

Pull requests are welcome too, though they are reviewed as time allows, and
one that changes a lot is best talked over in an issue first. Many changes
need nothing of the core: the browser cockpit is built from plugins, and a
plugin of your own can replace, restyle or extend any part of it, with tools,
skills and slash commands for the agent besides (see
[docs/plugins.md](docs/plugins.md)).

Before you open one, check that:

- `make test check` passes: the tests under the race detector, `go vet` and
  `gofmt`;
- a change in behavior comes with a test, and with the README or the docs
  updated where they describe it;
- no keys, tokens, account files or personal data are in the code, the tests
  or the commits.

By contributing, you agree that your contribution is licensed under the
project's [MIT License](LICENSE).
