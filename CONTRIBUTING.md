# Contributing

Issues and pull requests are welcome. The server is deliberately opinionated, so for a
new tool or a change to a board rule, please open an issue first to agree on the
behaviour.

## Before you open a pull request

```sh
cargo fmt
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo deny check
```

CI runs all of these on Linux and macOS, checks the build at the minimum supported Rust
version (`rust-version` in `Cargo.toml`), and builds every release target.

- Every behaviour change comes with a test. The tests in `tests/ops.rs` run each tool
  against a mock Kaneo and check every write it makes.
- The rules live in `src/workflow.rs` as pure functions, and `src/ops.rs` applies them.
  A rule belongs in `workflow.rs`, not in a tool's prose.
- A change to anything the README lists under Versioning needs a changelog entry, and a
  breaking change to it needs a major version.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org).

## Releasing

1. Set `version` in `Cargo.toml`, run `cargo build` so that `Cargo.lock` follows, and
   add the `## [x.y.z] - YYYY-MM-DD` section to `CHANGELOG.md`.
2. Merge to `main`, then push the tag `vx.y.z` on that commit.
3. The release workflow checks that the tag, `Cargo.toml` and the changelog agree, runs
   the checks again, builds and attests every target, and publishes the release.
