<img width="2170" height="725" alt="image" src="https://github.com/user-attachments/assets/bd260efb-8fd5-42e7-9d94-40755bac9171" />
<br />

### May you and your agents never lose a conversation ever again.
<br />

Fainder is a tiny, local and read-only terminal app for finding and resuming your conversations.

It searches Codex, Claude Code, OpenCode, Hermes, Cursor, and GitHub Copilot
histories from one place, then returns the right resume command.
Agents can also inspect a conversation by turn and print bounded chronological
context without starting another harness or calling an LLM.

## Install

```bash
brew install satelerd/tap/fainder
fainder
```

To update an existing install:

```bash
brew update
brew upgrade fainder
```

## Quick Start

Humans can open the TUI:

```bash
fainder
```

Fainder opens on your most recent conversations, newest first. Type a query,
pick a conversation, then press `Enter` to copy the resume command. A live line
under the search box always tells you exactly how your query is being matched.
Press `Ctrl-s` (or `Tab`) for a filter panel to pick providers, search mode, and
scope.
#### That’s it.

<br />

## Agent CLI

Find candidates:

```bash
fainder recent --limit 8
fainder search "SmartUp migration" --json --limit 8
```

Inspect one conversation before reading a large transcript:

```bash
fainder inspect claude:cab15fb3 --role user --limit 40
fainder inspect claude:cab15fb3 --find "PR|commit|deploy" --regex
fainder inspect claude:cab15fb3 --around 96 --context 8
```

Print a deterministic transcript window in chronological order:

```bash
fainder context claude:cab15fb3 --from-turn 90 --to-turn 130
fainder context codex:019dec17 --tail 80 --truncate-tools
```

Large context views show a token estimate first and require `--confirm`.

## Raycast

The repo includes an optional Raycast extension that uses the Fainder CLI:

```bash
cd raycast
npm install --cache /private/tmp/fainder-npm-cache
npm run dev
```

Now you can find your conversations from Raycast.

The extension calls `fainder recent --json` before you type, then
`fainder search <query> --json` once you search. It lists conversations and lets
you copy or open the selected `resume_command`.

For Raycast-only usage, the extension needs to be published to the Raycast
Store. The extension is prepared for that flow:

```bash
cd raycast
npm run build
npm run publish
```

Once Raycast approves and publishes it, install it from Raycast Store and run
`Search Conversations`. Fainder itself is still the local search engine, so the
extension will prompt you to install it with Homebrew if the `fainder` binary is
missing.

## Skill

Fainder includes a portable agent skill at:

```text
skills/fainder/SKILL.md
```

Agent harnesses that support skills can load that file to learn how to install,
query, parse, and safely use Fainder.

## Query Behavior

- By default the query matches as one exact phrase (case-insensitive). For
  example, `SmartUp agents` only matches conversations with those words adjacent
  and in that order.
- `words` mode (`Ctrl-m` in the TUI, `--words` in the CLI) requires every word
  independently in any order.
- `regex` mode lets you use patterns like `SmartUp|bedrock`.
- A single word behaves the same in every mode and searches titles, paths,
  recent messages, and transcript content.
- Search waits briefly after typing so it does not scan while you are still
  entering a query.

## Search Model

- Metadata is read live from native provider files.
- Uses `ripgrep` for fast transcript search. The Homebrew formula installs it automatically.
- Full-text search uses live transcript scans and `rg` candidate discovery.
- OpenCode is read from OpenCode's own SQLite database.
- Cursor and GitHub Copilot are read from VS Code-style `workspaceStorage`
  SQLite databases.
- Fainder does not require a background service or manual indexing step.


## Development

```bash
cargo test
cargo run
cargo run -- search SmartUp --limit 5
cargo run -- inspect codex:019dec17 --role user --limit 10
cargo run -- context codex:019dec17 --tail 20
```

## Release

Release automation updates Cargo metadata, tags the repo, computes the Homebrew
tarball SHA, updates the formula, and pushes the tap:

```bash
scripts/release.sh 0.1.3
```
