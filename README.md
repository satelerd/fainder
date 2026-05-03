<img width="2170" height="725" alt="image" src="https://github.com/user-attachments/assets/bd260efb-8fd5-42e7-9d94-40755bac9171" />
<br />

May you and your agents never lose a conversation ever again.

Fainder is a terminal app for finding and resuming local AI agent conversations.
It is built for humans using the TUI and for agents using the non-interactive
CLI.

It searches Codex, Claude Code, OpenCode, and Hermes histories from one place,
then returns the right resume command.

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

Type a query, pick a conversation, then press `Enter` to copy the resume command.

Agents should usually use non-interactive search:

```bash
fainder search SmartUp
fainder search SmartUp --provider codex,claude --json
fainder search SmartUp --scope metadata --preview --limit 5
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

## Agent Usage

For deterministic usage, agents should prefer:

```bash
fainder doctor
fainder search "SmartUp agents" --json --limit 10
fainder search "bedrock latency" --provider codex,claude --json --limit 5
fainder search "SmartVOC|SmartOrders|multichannel" --regex --json --limit 15
fainder search "SmartUp agents" --scope metadata --preview --limit 5
fainder search "SmartUp agents" --command-only --select 1
fainder search "SmartUp agents" --copy --select 1
```

JSON results include `provider`, `id`, `title`, `cwd`, `updated_at`,
`resume_command`, `score`, `matched_in`, `snippets`, and `latest_messages`.

Do not blindly execute the first `resume_command`. Inspect the candidate first,
then run or show the command once the intended conversation is clear.

Useful non-interactive flags:

- `--json`: machine-readable output for agents and integrations.
- `--provider codex,claude`: search only selected providers.
- `--scope all`: search metadata and full transcript content. This is the default.
- `--scope metadata`: search title, path, and recent messages only.
- `--regex`: treat the query as a case-insensitive regex.
- `--preview`: include snippets and latest messages in text output.
- `--command-only`: print only the selected resume command.
- `--copy`: copy the selected resume command.
- `--open`: execute the selected resume command.
- `--select N`: choose the one-based result used by `--command-only`, `--copy`, or `--open`.
- `--limit N`: cap result count.

## Skill

Fainder includes a portable agent skill at:

```text
skills/fainder/SKILL.md
```

Agent harnesses that support skills can load that file to learn how to install,
query, parse, and safely use Fainder.

## Raycast

The repo includes a Raycast extension that uses the Fainder CLI instead of
duplicating provider parsers:

```bash
cd raycast
npm install --cache /private/tmp/fainder-npm-cache
npm run dev
```

The extension calls `fainder search <query> --json`, lists conversations, and
lets you copy or open the selected `resume_command`.

## Search Model

- Metadata is read live from native provider files.
- Uses `ripgrep` for fast transcript search. The Homebrew formula installs it automatically.
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
