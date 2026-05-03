# Fainder

Fainder is a terminal app for finding and resuming local AI agent conversations.

It searches Codex, Claude Code, OpenCode, and Hermes histories from one place,
then copies or runs the right resume command.

## Install

```bash
brew install satelerd/tap/fainder
```

Fainder uses `ripgrep` for fast transcript search. The Homebrew formula installs
it automatically.

To update an existing install:

```bash
brew update
brew upgrade fainder
```

## Quick Start

```bash
fainder
```

Type a query, pick a conversation, then press `Enter` to copy the resume command.

Non-interactive search is also available:

```bash
fainder search SmartUp
fainder search SmartUp --provider codex,claude --json
fainder doctor
```

## TUI Keys

- Type to search.
- `Enter` copies the resume command.
- `Ctrl-o` opens the selected conversation directly.
- `Ctrl-p` opens the preview pane.
- `Tab` cycles provider filters.
- `Ctrl-r` toggles regex mode.
- `Ctrl-f` switches between title metadata and full transcript search.
- `Esc` exits.

## Query Behavior

- A single word searches titles, paths, recent messages, and transcript content.
- Multiple words narrow the search. For example, `SmartUp agents` matches
  conversations containing both words.
- Regex mode lets you use patterns like `SmartUp|bedrock`.
- Search waits briefly after typing so it does not scan while you are still
  entering a query.

## Search Model

- Metadata is read live from native provider files.
- Full-text search uses live transcript scans and `rg` candidate discovery.
- OpenCode is read from OpenCode's own SQLite database.
- Fainder does not require a background service or manual indexing step.

## Provider Defaults

- Codex: `~/.codex`
- Claude Code: `~/.claude`
- OpenCode: `~/.local/share/opencode/opencode.db`
- Hermes: `~/.hermes/sessions`

Optional overrides:

```toml
# ~/.config/fainder/config.toml
[paths]
codex = "~/.codex"
claude = "~/.claude"
opencode = "~/.local/share/opencode/opencode.db"
hermes = "~/.hermes/sessions"
```

## Development

```bash
cargo test
cargo run
cargo run -- search SmartUp --limit 5
```
