# kaneo-workflow-mcp

An opinionated [MCP](https://modelcontextprotocol.io) server for [Kaneo](https://kaneo.app)
boards. It is built for AI agents that work cards on a board, and it enforces a card
workflow in code rather than leaving it to the prompt:

- a card enters and leaves the work-in-progress column only through `start_card`,
  `pause_card` and `close_card`, and `start_card` refuses a card when the column is at
  its cap;
- every lifecycle step leaves a record comment (`START`, `PAUSE`, `DONE`) on the card;
- closing a card requires evidence;
- a decision is a `DECISION` comment that has to say who decided, since Kaneo credits
  every write to the API key's owner;
- the record comments cannot be faked through the plain `comment` tool, and they cannot
  be deleted;
- an agent can tighten the board's rules through the server but cannot loosen them.

If you want a general-purpose client for the Kaneo API, use Kaneo's own server,
[`@kaneo/mcp`](https://www.npmjs.com/package/@kaneo/mcp). This one gives you fewer
tools with stricter rules.

## How it differs from `@kaneo/mcp`

| | `@kaneo/mcp` | kaneo-workflow-mcp |
| --- | --- | --- |
| Scope | The Kaneo API, about one tool per endpoint (38 tools) | 22 tools, one per workflow step |
| Board rules | None: any status change is allowed | Work-in-progress cap, lifecycle comments, evidence to close, decisions with provenance |
| Addressing | Internal ids | Tickets (`ABC-12`) and project slugs (`ABC`); the server resolves ids |
| Results | API objects as JSON | Short plain text, sized for an agent's context |
| Runtime | Node.js | One static binary |

## Install

Each release ships archives for `aarch64-apple-darwin`, `x86_64-unknown-linux-musl` and
`aarch64-unknown-linux-musl`, a `SHA256SUMS` file, and a
[build provenance attestation](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations)
for every file. Check the attestation before you run the binary:

```sh
tag=v1.0.0
target=aarch64-apple-darwin
gh release download "$tag" --repo dlepaux/kaneo-workflow-mcp \
  --pattern "kaneo-workflow-mcp-$tag-$target.tar.gz"
gh attestation verify "kaneo-workflow-mcp-$tag-$target.tar.gz" \
  --repo dlepaux/kaneo-workflow-mcp \
  --signer-workflow dlepaux/kaneo-workflow-mcp/.github/workflows/release.yml \
  --source-ref "refs/tags/$tag" \
  --deny-self-hosted-runners
tar -xzf "kaneo-workflow-mcp-$tag-$target.tar.gz" kaneo-workflow-mcp
./kaneo-workflow-mcp --version
```

To build from source instead, you need Rust 1.89 or later:
`cargo install --locked --git https://github.com/dlepaux/kaneo-workflow-mcp --tag v1.0.0`.

## Configuration

The server speaks MCP over stdio, and your MCP client starts it. All configuration comes
from the environment:

| Variable | |
| --- | --- |
| `KANEO_API_URL` | The instance origin, with or without `/api`. Required. Plain `http` is refused except for loopback hosts. |
| `KANEO_API_KEY` | A Kaneo API key, sent as a Bearer token. Required. |
| `KANEO_SPRINT_PROJECT` | The slug of a project that holds one card per sprint. Optional: leave it unset if you don't run sprints. |

A Claude Code entry, for example:

```json
{
  "mcpServers": {
    "kaneo": {
      "command": "/usr/local/bin/kaneo-workflow-mcp",
      "env": { "KANEO_API_URL": "https://kaneo.example.com", "KANEO_API_KEY": "..." }
    }
  }
}
```

One binary can serve several instances. Give each MCP entry its own URL and key.

## Board rules

A new Kaneo project works without any setup. The server reads its rules from the board's
columns and, if they are there, from lines in the project description:

| Description line | Effect | Without the line |
| --- | --- | --- |
| `Work in progress: <column>` | The column `start_card` moves cards into, and that plain moves cannot enter or leave. | The column with slug `doing`, else `in-progress` (Kaneo's default *In Progress*). If there is neither, `start_card` refuses. |
| `Column limit: <column> holds at most <n> cards` | The work-in-progress cap. The column must be the work-in-progress column, and `1 card` reads too. A line that does not parse, or that caps another column, is an error, never a silent "no cap". | No cap. |
| `New cards go to: <column>` | Where `file_card` puts a card when no column is given. | The first column that is neither the work-in-progress column nor the final one. |

Columns are matched by slug or by name, ignoring case and punctuation: `To Do`, `to-do`
and `todo` all match. The final column is the one Kaneo marks final, and only
`close_card` moves a card into it. Moving a card out of it with `move_card` reopens it,
and that is allowed.

`edit_project_description` refuses an edit that drops one of these lines, states one
that does not resolve on the board, moves the work-in-progress column or raises its
cap. Lowering the cap or adding a line is fine. The agents that call the server work
under these rules, so loosening them is done in Kaneo itself.

`pause_card` takes a card out of the work-in-progress column, and refuses a card that is
not in it. The card goes to `sprint`
when sprints are configured and the card has a `related` link to an open card of the
sprints project, and to `backlog` otherwise. If the board has neither column, the card
goes to the intake column, and then to the first open column. A pause never lands in
the work-in-progress or the final column.

Kaneo has no compare-and-set, so the cap is checked, not locked: two agents that start a
card in the same instant can both pass a cap with one place left.

Record comments are stamped in UTC:

```text
START 2026-10-27 10:05Z — agent-1 — plan
PAUSE 2026-10-27 10:05Z — agent-1 — state Next: next step
DONE 2026-10-27 10:05Z — agent-1 — evidence
DECISION 2026-10-27 — choice

Why: reasoning
Overruled: concern (optional)
Covers: scope (optional)
Provenance: who decided
```

## Tools

| Tool | What it does |
| --- | --- |
| `card_brief` | Header, labels, relations, every DECISION comment, the newest comments and the description, in one call. |
| `start_card` | Refuses at the cap, otherwise posts START and moves the card to the work-in-progress column. A card already there is being taken over, so the cap does not apply. |
| `pause_card` | Posts PAUSE and moves a card in progress out, as described above. `needs_decision` also adds the workspace label `operator-decision`. |
| `close_card` | Requires evidence, posts DONE and moves the card to the final column. |
| `decide` | Posts a DECISION comment. Provenance is required. |
| `board_digest` | Open columns with their cards, counts for Backlog and the final column, and the work-in-progress column against its cap. |
| `comment` | Posts a plain comment. Text starting with START, PAUSE, DONE or DECISION is refused. |
| `find` | Searches cards, one line per card. A full ticket ranks first. |
| `label`, `unlabel` | Attaches a workspace label by name, or removes a card's copy of it. |
| `file_card` | Files a card in the intake column unless given another column. Never into the work-in-progress or the final column. Answers the new ticket. |
| `link`, `unlink` | `blocks`, `subtask` (the ticket is the parent) or `related`, between two cards anywhere in the workspace. Linking twice is a no-op. |
| `edit_card` | Any subset of title, description, priority, start and due dates, and assignee. Dates and assignee take `none` to clear. |
| `move_card` | Moves a card to any other column of its project, or to a column of another project, and answers the card's new ticket. |
| `edit_comment`, `delete_comment` | Takes a comment id, checked to belong to the card. An edit keeps the comment's kind. Only plain comments can be deleted. |
| `list_cards` | A project's cards by creation date, paged, optionally one column. |
| `card_activity` | When the card was created and each status change. |
| `project_info` | Columns, the rules the server reads, and the whole description. |
| `edit_project_description` | Replaces the description, refusing an edit that loosens the rules, and answers the rules it now states. |
| `projects` | Every project the key can see, by workspace, with the workspace's label names. |

Writes to a card answer `ok <ticket>`, plus `→ <column>` when the card moved, and
`· unassigned` when a move to another project dropped the assignee. Writes to a comment
answer `ok <comment id>`. `link` and `unlink` answer `ok <ticket> <kind> <ticket>`, and
`edit_project_description` answers `ok <project id> · <rules>`. Errors come back as tool
errors that the agent can read.
Dates take a day (`2026-11-01`, stored as midday UTC so that every zone from UTC−11 to
UTC+11 shows the same day) or an RFC 3339 time.

## Versioning

This project follows [Semantic Versioning](https://semver.org). The public interface,
which a minor or patch release will not break, is:

- the tool names, their argument names and types, and what each tool enforces;
- the description lines above and how they are read;
- the record comment formats (`START`, `PAUSE`, `DONE`, `DECISION`), which other tools
  may parse;
- the write answers listed under Tools, up to the `<rules>` text after `ok <project id> ·`;
- the environment variables, the command-line flags, the exit codes (0, 1 when the
  server stops on an error, 2 for an unknown argument), and the release asset names.

The server follows the Kaneo API of its release date, and the contract tests check it
against a real instance. Keeping up with a Kaneo API change behind the same interface
is a minor or patch release. A Kaneo change that forces the interface above to change
is a major one.

The layout of read results (`card_brief`, `board_digest` and the like) and the wording of
error messages are for agents to read. They may change in any release. The Rust library
inside the crate is internal and not covered.

## Development

```sh
cargo test                                  # unit, mock-Kaneo and stdio tests; no real instance
cargo clippy --all-targets -- -D warnings
cargo deny check
KANEO_API_URL=... KANEO_API_KEY=... KANEO_CONTRACT_TICKET=ABC-12 \
  cargo test --test contract -- --ignored   # read-only checks against a real Kaneo
```

See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md).

## Licence

MIT, see [LICENSE](LICENSE).
