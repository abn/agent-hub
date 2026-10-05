# Contributing

Thanks for your interest in Agent Hub.

## Making a change

Read the [contributor guide](docs/contribution/guide.md) for the workflow,
conventions, and the verification gate. In short:

```
make setup
make check
```

Changes are scoped, land on a conventional branch, follow Conventional
Commits with no trailers, and are not done until `make check` passes and the
wiki reflects the new behaviour.

`AGENTS.md` is the operational contract and applies to agent sessions as much
as to people.

## Public-ready by default

Everything committed, and documentation in particular, must be safe to
publish: no internal codenames, hostnames, absolute paths, tokens, or task
identifiers. Working notes and the internal progress log live in the
gitignored scratch area under `.agents/brain/` and are never committed.

## Licence

Contributions are made under the MIT licence, matching [LICENSE](LICENSE).
