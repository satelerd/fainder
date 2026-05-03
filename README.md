<img width="2170" height="725" alt="image" src="https://github.com/user-attachments/assets/bd260efb-8fd5-42e7-9d94-40755bac9171" />
<br />

### May you and your agents never lose a conversation ever again.
<br />

Fainder is a tiny, local and read-only terminal app for finding and resuming your conversations.

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
#### That’s it.

<br />

## Raycast

The repo includes a optional Raycast extension that uses the Fainder CLI:

```bash
cd raycast
npm install --cache /private/tmp/fainder-npm-cache
npm run dev
```

Now you can find your conversations from Raycast.

The extension calls `fainder search <query> --json`, lists conversations, and
lets you copy or open the selected `resume_command`.

## Skill

Fainder includes a portable agent skill at:

```text
skills/fainder/SKILL.md
```

Agent harnesses that support skills can load that file to learn how to install,
query, parse, and safely use Fainder.

## Query Behavior

- A single word searches titles, paths, recent messages, and transcript content.
- Multiple words narrow the search. For example, `SmartUp agents` matches
  conversations containing both words.
- Regex mode lets you use patterns like `SmartUp|bedrock`.
- Search waits briefly after typing so it does not scan while you are still
  entering a query.

## Search Model

- Metadata is read live from native provider files.
- Uses `ripgrep` for fast transcript search. The Homebrew formula installs it automatically.
- Full-text search uses live transcript scans and `rg` candidate discovery.
- OpenCode is read from OpenCode's own SQLite database.
- Fainder does not require a background service or manual indexing step.


## Development

```bash
cargo test
cargo run
cargo run -- search SmartUp --limit 5
```
