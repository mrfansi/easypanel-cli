# Contributing

Thanks for looking. This is a small, opinionated tool; the bar is correctness, not
feature count.

## Getting started

```bash
cargo build
cargo test
cargo run          # opens the TUI against your default server
```

You need at least one EasyPanel host to use it:

```bash
cargo run -- server add prod --url https://panel.example.com --token <TOKEN>
```

The API is tRPC-style: `POST {url}/api/rpc/{group}/{op}`, `Authorization: Bearer
<token>`, body **always** `{"json": <input>}` (`null` when there are no parameters — an
empty body returns 400). Responses wrap payloads in `json`. The full spec is in
`easypanel-api.json`; 374 endpoints, of which this tool uses a fraction.

## Definition of done

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

All three must be clean. CI runs exactly these.

## What this project has learned the hard way

Please read these before changing behaviour. Each one cost real debugging.

### A test that encodes a wrong assumption is worse than no test

The client discarded *every* server error message for the project's entire life —
`Branch not found` surfaced as `[400] Bad Request` — and the test covering it passed,
because the mock asserted a response shape the server never sends. The test converted a
bug into evidence of correctness.

If you mock an API response, verify the shape against a real server first.

### Don't trust your own measurement

More false conclusions here came from broken tooling than from broken code:

- zsh's builtin `echo` interprets backslash escapes and **corrupts JSON** — this
  produced a confident, wrong claim that an endpoint returned malformed data.
- `tac` doesn't exist on macOS, so a verification loop ran zero times and its empty
  output read as "passed".
- ratatui **diff-renders**: only changed cells are emitted, so screen-scraping the TUI
  for a value repeatedly suggested working features were broken.

When a check says something is broken, first ask whether the check is broken.

### Recurring bug classes

1. **Filtered view vs. action index.** If a table filters what it renders, actions must
   index the *filtered* list. Otherwise `x` deletes the wrong row. Render and actions
   share one source (`visible_services()`, `visible_domains()`).
2. **Silent defaults that change config.** A dropdown whose current value is absent from
   a freshly loaded option list must keep the current value, not jump to the first —
   otherwise opening a form is enough to change the deployed branch.
3. **Unmodelled fields dropped on edit.** Edit forms start from the original JSON so
   fields the form doesn't model (middlewares, `nixpacksVersion`) survive.
4. **Confidently wrong numbers.** Prefer `-` over a fake `0`. A metric reading `0.00`
   because of a shape mismatch is worse than a missing one.
5. **Named colours.** `Color::Blue` is reinterpreted by terminal themes and has twice
   produced unreadable text. Use `Color::Indexed`.
6. **Destructive actions confirm first** — and the prompt names the real target.

### Live verification

Unit tests cannot see endpoint side effects. `updateSourceGithub` silently resets
`autoDeploy` to `false`; only a live call revealed it. If you change something that
touches the API, test it against a throwaway project (`zzz-*`) and clean up after
yourself. If you can't, say so in the PR rather than implying you did.

## Commits and releases

- Commit messages say what changed and **why it mattered**, and honestly name what was
  not verified.
- User-visible changes get a `CHANGELOG.md` entry under `## [Unreleased]`
  ([Keep a Changelog](https://keepachangelog.com) headings).
- Releases are cut by tagging `vX.Y.Z`; `.github/workflows/release.yml` builds and
  publishes the binaries. The workflow can be dry-run from the Actions tab — it only
  publishes on a tag.

## Scope

Happy to take: bug fixes, tests, docs, accessibility, packaging, endpoints from
`easypanel-api.json` that you can verify.

Ask first: anything that needs a live server you can't test against, or that expands
the tool beyond managing EasyPanel hosts.
