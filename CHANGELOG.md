# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org) within the scope the README declares.

## [1.1.0] - 2026-11-04

### Added

- `list_cards`, `board_digest` and `find` print each card's labels on its line
  (` · labels a, b`), so a check by label needs no read per card. `list_cards` and
  `board_digest` take them from Kaneo's task list; `find` reads the workspace's labels
  once per workspace with hits.
- `card_brief` shows the card's start date next to its due date.

## [1.0.0] - 2026-11-04

The first public release.

### Added

- 22 tools for an agent working a Kaneo board: reading cards and boards, the
  `start_card` / `pause_card` / `close_card` lifecycle, decisions of record, comments,
  labels, links, edits and moves.
- Board rules read from the project description (`Work in progress:`, `Column limit:`,
  `New cards go to:`), with fallbacks so that a default Kaneo board works unconfigured.
  A description edit through the server can tighten them but not loosen them.
- Optional sprints through `KANEO_SPRINT_PROJECT`.
- `--version` and `--help`.
- Release archives for `aarch64-apple-darwin`, `x86_64-unknown-linux-musl` and
  `aarch64-unknown-linux-musl`, with `SHA256SUMS` and build provenance attestations.

[1.1.0]: https://github.com/dlepaux/kaneo-workflow-mcp/releases/tag/v1.1.0
[1.0.0]: https://github.com/dlepaux/kaneo-workflow-mcp/releases/tag/v1.0.0
