---
name: fainder
description: Use Fainder to find, inspect, and resume local AI agent conversations across Codex, Claude Code, OpenCode, and Hermes. Use when an agent needs to recover prior work, locate a lost thread, search conversation history, identify the right resume command, audit available local providers, or guide a human through Fainder installation and usage from the terminal or non-interactive CLI.
---

# Fainder

## Purpose

Fainder is a local conversation finder for humans and agents. It searches local AI agent histories from multiple providers and returns the command needed to resume the selected conversation.

Use Fainder when the task depends on prior local agent context and the user does not remember which tool or conversation contains it.

## Install And Update

Install with Homebrew:

```bash
brew install satelerd/tap/fainder
```

Update an existing install:

```bash
brew update
brew upgrade fainder
```

Verify the install:

```bash
fainder --help
fainder doctor
```

Fainder uses `ripgrep` for fast transcript search. The Homebrew formula installs it automatically.

## Providers

Fainder currently supports:

- `codex`: default path `~/.codex`
- `claude`: default path `~/.claude`; aliases accepted by CLI config/parser include `claude-code` and `cloud-code`
- `opencode`: default path `~/.local/share/opencode/opencode.db`
- `hermes`: default path `~/.hermes/sessions`

Check which providers exist locally:

```bash
fainder doctor
```

Override paths with `~/.config/fainder/config.toml`:

```toml
[paths]
codex = "~/.codex"
claude = "~/.claude"
opencode = "~/.local/share/opencode/opencode.db"
hermes = "~/.hermes/sessions"
```

## Agent-First Usage

Prefer the non-interactive CLI when you are an agent. It is deterministic, scriptable, and returns resume commands directly.

Basic search:

```bash
fainder search SmartUp
```

Limit results:

```bash
fainder search "SmartUp agents" --limit 5
```

Search only specific providers:

```bash
fainder search "bedrock latency" --provider codex,claude --limit 10
```

Search metadata only, matching the TUI scope toggle:

```bash
fainder search "SmartUp agents" --scope metadata --limit 10
```

Use regex for alternatives or structured patterns:

```bash
fainder search "SmartUp|bedrock|shapeup" --regex --limit 20
```

Get machine-readable JSON:

```bash
fainder search "SmartUp agents" --json --limit 10
```

Show snippets and recent messages in human-readable output:

```bash
fainder search "SmartUp agents" --preview --limit 5
```

Print only the selected resume command:

```bash
fainder search "SmartUp agents" --command-only --select 1
```

Copy or open the selected resume command:

```bash
fainder search "SmartUp agents" --copy --select 1
fainder search "SmartUp agents" --open --select 1
```

The JSON result items contain:

- `provider`: conversation provider, such as `codex` or `claude`
- `id`: provider-specific session/conversation id
- `title`: inferred conversation title
- `cwd`: working directory or project path, when available
- `updated_at`: last known update timestamp
- `resume_command`: shell command to resume or open the conversation
- `score`: Fainder ranking score
- `matched_in`: where the match was found, such as metadata or transcript text
- `snippets`: matched text excerpts
- `latest_messages`: recent messages for quick disambiguation

For an agent, a strong default workflow is:

1. Run `fainder doctor` to confirm available histories.
2. Search with a broad query and JSON output.
3. Inspect `provider`, `cwd`, `title`, `updated_at`, `snippets`, and `latest_messages`.
4. Use `resume_command` only after selecting the intended conversation.
5. If results are noisy, rerun with `--provider`, more query terms, `--limit`, or `--regex`.

Example agent workflow:

```bash
fainder doctor
fainder search "SmartUp agents" --json --limit 8
fainder search "SmartUp agents" --provider codex --json --limit 5
fainder search "SmartUp agents" --command-only --select 1
```

Do not blindly execute the first result. Prefer reporting the best candidates to the user when confidence is low.

## Query Semantics

Default search is word-based and case-insensitive.

- A single word searches titles, paths, recent messages, and transcript content.
- Multiple words narrow the search. `SmartUp agents` means both words should match.
- Use quotes in the shell when a query has spaces.
- Use `--regex` only when the query is a real regex or needs alternatives like `SmartUp|bedrock`.
- Use `--provider codex,claude` to reduce noise and speed up targeted searches.
- Use `--limit N` to control output volume.
- Use `--scope all` to include transcript content. This is the default.
- Use `--scope metadata` to search only title, path, and recent messages.
- Use `--preview` for text output with snippets and latest messages.
- Use `--json` for integrations such as Raycast or another agent harness.
- Use `--select N` with `--command-only`, `--copy`, or `--open` when acting on a specific result.

## Interactive TUI

Humans can run:

```bash
fainder
```

TUI controls:

- Type to search.
- `Enter` copies the selected resume command.
- `Ctrl-o` opens the selected conversation directly.
- `Ctrl-p` toggles the preview pane.
- `Tab` cycles provider filters.
- `Ctrl-r` toggles regex mode.
- `Ctrl-f` switches search scope.
- `Esc` exits.

The TUI shows waiting/searching state in the `Conversations` title, ranks recent matches higher, and displays project/repo context before the title.

## Raycast Extension

The repository includes a Raycast extension in `raycast/`. It is a UI wrapper around the non-interactive CLI and should call:

```bash
fainder search "<query>" --json --limit 50 --scope all
```

Provider and scope filters map to CLI flags:

```bash
fainder search "<query>" --json --limit 50 --scope metadata --provider claude
```

Develop locally:

```bash
cd raycast
npm install --cache /private/tmp/fainder-npm-cache
npm run dev
```

Build locally:

```bash
cd raycast
npm run build
```

Publish to Raycast Store:

```bash
cd raycast
npm run publish
```

Published Store installs are the only path that lets users install the extension fully from Raycast without running a local development server. The extension still depends on the local `fainder` binary; if it is missing, the Raycast UI should guide the user to run `brew install satelerd/tap/fainder`.

Do not duplicate provider parsers in the Raycast code. Keep provider discovery, ranking, snippets, and resume command generation in the Rust CLI, then consume the JSON output.

## Practical Patterns

Recover a project thread:

```bash
fainder search "SmartUp" --json --limit 10
```

Recover a specific debugging thread:

```bash
fainder search "timeout retry webhook" --json --limit 10
```

Search only Claude Code and Codex:

```bash
fainder search "kubernetes migration" --provider claude,codex --json --limit 10
```

Find one of several terms:

```bash
fainder search "SmartVOC|SmartOrders|multichannel" --regex --json --limit 15
```

Use the top candidate cautiously:

```bash
fainder search "SmartUp agents" --json --limit 1
```

Then inspect the `resume_command` field and decide whether to copy, show, or run it.

Copy the top candidate after inspection:

```bash
fainder search "SmartUp agents" --copy --select 1
```

Open the top candidate only when the user wants to resume it:

```bash
fainder search "SmartUp agents" --open --select 1
```

## Safety

- Fainder reads local histories and can surface sensitive conversation text. Do not paste large raw outputs into external services unless the user explicitly wants that.
- `fainder search` itself is read-only.
- `resume_command` may open an interactive tool or resume an agent session; ask before executing it if doing so could change files, spend tokens, or touch production systems.
- When summarizing results, include only enough snippets to help the user choose the right conversation.
