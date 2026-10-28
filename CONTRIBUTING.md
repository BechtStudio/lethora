# Contributing

A first version; the full guide (code of conduct, pull-request rules, code
style, issue templates) comes with
[#21](https://github.com/BechtStudio/lethora/issues/21).

## Where things live

- Open work: the [issue tracker](https://github.com/BechtStudio/lethora/issues),
  planned per release in [milestones](https://github.com/BechtStudio/lethora/milestones).
  Issues without a milestone are the backlog.
- Accepted or low-priority limitations of finished work:
  [docs/dev/known-issues.md](docs/dev/known-issues.md) (technical) and
  [docs/manual/known-limitations.md](docs/manual/known-limitations.md)
  (what a writer may notice).
- How the app works: [docs/dev/architecture.md](docs/dev/architecture.md); the
  layering rules are in [CLAUDE.md](CLAUDE.md).
- What changed: [CHANGELOG.md](CHANGELOG.md).

## Workflow

Per issue: detail the plan and decisions in the issue → build → review,
run all checks (below), look at the change in the dev UI → update
CHANGELOG.md, and `docs/dev/known-issues.md` or the manual's known limitations
for any limitation accepted on the way → commit with `Closes #<n>`.
Larger decisions go into the issue as comments, so the reason stays next
to the work.

`docs/plans/` is an archive: the step plans, decisions and hand-test lists
of the milestones before the move to the issue tracker (2026-10-27). Read
them for the why behind existing code; new plans go into issues.

## Commands

- Run the app: `npm --prefix app run tauri dev`
- Tests: `cargo test --workspace`, `npm --prefix app test`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings` (one known
  warning from vendor/loro-internal), `cargo fmt --check`
- Book benchmarks (release): `cargo test --release -p lethora-core -- --ignored --nocapture`,
  `cargo test --release -p lethora-typeset --test worker -- --ignored --nocapture`
- Dev UI in a browser: `NODE_ENV=development npx vite build --mode development
  --outDir <dir>` in `app/`, serve with `vite preview` (mock backend; `?book=1`
  for a book-size document)

## Docs site

`website/` builds the docs into a Docusaurus site, published to GitHub
Pages from `main` (`.github/workflows/docs.yml`).

- Install: `npm --prefix website install`
- Dev server (port 3000): `npm --prefix website start`
- Build (fails on broken links): `npm --prefix website run build`

The site reads the Markdown in place: `docs/dev/` becomes `/dev`,
`docs/manual/` becomes `/manual`; `docs/plans/` stays
off the site. Images live in `docs/images/`. Links that leave `docs/dev/` or
`docs/manual/` (README, CHANGELOG, `deploy/`, the plans, the other part of
the site) must be full GitHub URLs on `main`.

At the 1.0.0 tag, cut the versions with
`npm --prefix website run docusaurus docs:version:dev 1.0.0` and
`npm --prefix website run docusaurus docs:version:manual 1.0.0`.
