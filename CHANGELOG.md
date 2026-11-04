# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org) within the scope the README declares.

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

[1.0.0]: https://github.com/dlepaux/kaneo-workflow-mcp/releases/tag/v1.0.0
