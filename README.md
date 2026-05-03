# Fainder

Fainder is a live terminal finder for local AI agent conversations.

It searches Codex, Claude Code, OpenCode, and Hermes histories without creating
its own database and without requiring an indexing step.

## Usage

```bash
cargo install --path .
fainder
```

Non-interactive search:

```bash
fainder search imalab
fainder search imalab --provider codex,claude --json
fainder doctor
```

## TUI Keys

- Type to search.
- `Enter` copies the resume command.
- `Ctrl-o` opens the selected conversation directly.
- `Tab` cycles provider filters.
- `Ctrl-r` toggles regex mode.
- `Ctrl-f` toggles full-text search.
- `Esc` exits.

## Search Model

Fainder is intentionally stateless:

- Metadata is read live from native provider files.
- Full-text search uses live transcript scans and `rg` candidate discovery.
- OpenCode is read from OpenCode's own SQLite database.
- Fainder does not create a search database.

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

## Homebrew

The formula scaffold lives in `packaging/homebrew/fainder.rb`. It is a release
template; update the URL and checksum when publishing a tarball.
