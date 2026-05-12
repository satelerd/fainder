---
name: fainder
description: Use Fainder to search, inspect, and contextualize local AI agent conversations across Codex, Claude Code, OpenCode, Hermes, Cursor, and GitHub Copilot. Trigger when an agent needs to recover prior work, find a lost thread, inspect a transcript by role or turn, produce bounded chronological context from another harness, locate resume commands, or guide installation/update with Homebrew.
---

# Fainder

Fainder is a local, read-only conversation finder for humans and agents. Prefer the non-interactive CLI when you are an agent.

## Install

```bash
brew install satelerd/tap/fainder
brew update && brew upgrade fainder
fainder doctor
```

Supported providers:

- `codex`
- `claude`
- `opencode`
- `hermes`
- `cursor`
- `copilot`

## Agent Workflow

Use this sequence:

1. `fainder doctor` to confirm local histories.
2. `fainder search` to find candidate conversations.
3. `fainder inspect` to locate the relevant turn range inside the chosen conversation.
4. `fainder context` to print a bounded chronological transcript window.
5. Continue in the current harness using that evidence. Do not run a provider-native `resume_command` unless the user wants to reopen that provider.

## Search

Find candidate conversations:

```bash
fainder search "SmartUp migration" --json --limit 8
fainder search "batch-download Williams" --provider codex,claude --json --limit 10
fainder search "PR|commit|deploy" --regex --json --limit 10
fainder search "SmartUp agents" --scope metadata --json --limit 10
```

Important JSON fields:

- `provider`
- `id`
- `title`
- `cwd`
- `created_at`
- `updated_at`
- `message_count`
- `resume_command`
- `snippets`
- `latest_messages`

If confidence is low, show the best candidates to the user instead of guessing.

## Inspect

Use `inspect` to navigate one conversation without printing the whole transcript.

List human/user turns first when reconstructing intent:

```bash
fainder inspect claude:cab15fb3 --role user --limit 40
```

Check where the agent stopped:

```bash
fainder inspect claude:cab15fb3 --role agent --tail 20
```

Search inside one conversation:

```bash
fainder inspect claude:cab15fb3 --find "PR|commit|batch-download|documents.append" --regex
fainder inspect codex:019dec17 --find "Raycast Homebrew skill"
```

Read around a relevant turn:

```bash
fainder inspect claude:cab15fb3 --around 96 --context 8
fainder inspect claude:cab15fb3 --turn 96 --context 8 --expand
```

Use JSON for automation:

```bash
fainder inspect claude:cab15fb3 --find "PR|commit" --regex --json
```

`inspect` output uses stable turn numbers. Feed those turn numbers into `context`.

## Context

Use `context` after `inspect` identifies a useful range. It is deterministic: it does not call an LLM, does not start Codex or Claude, and does not summarize. It prints local transcript evidence in chronological order:

```bash
fainder context claude:cab15fb3 --from-turn 90 --to-turn 130
fainder context claude:cab15fb3 --around 96 --context 12
fainder context codex:019dec17 --tail 80 --truncate-tools
```

The default output is Markdown with:

- source metadata
- token budget estimate
- chronological transcript turns
- user, agent, and tool messages in the original order

Large outputs require confirmation. If Fainder prints a budget warning, narrow the range or rerun with `--confirm`:

```bash
fainder context claude:cab15fb3 --from-turn 1 --to-turn 240 --confirm
fainder context claude:cab15fb3 --from-turn 1 --to-turn 240 --max-tokens 30000 --confirm
```

Useful context flags:

- `--from-turn N --to-turn M`
- `--around N --context K`
- `--tail N`
- `--role user|agent|tool|system|all`
- `--truncate-tools`
- `--no-tools`
- `--max-tokens N`
- `--confirm`
- `--format json`

Do not ask Fainder to summarize. If a summary is needed, summarize the deterministic `context` output in the current harness after reading it.

## Resume Commands

Search results include `resume_command`. Use it only when the user wants to reopen the original provider:

```bash
fainder search "SmartUp migration" --command-only --select 1
fainder search "SmartUp migration" --copy --select 1
fainder search "SmartUp migration" --open --select 1
```

For cross-harness continuation, prefer `inspect` and `context` over `resume_command`.

## Query Rules

- Default search is case-insensitive word matching.
- Multiple words narrow results.
- Use `--regex` for alternatives like `SmartUp|bedrock|shapeup`.
- Use `--provider codex,claude` to reduce noise.
- Use `--scope metadata` when full-text is too noisy.
- Use `--json` for machine-readable output.

## Config

Override provider paths with `~/.config/fainder/config.toml`:

```toml
[paths]
codex = "~/.codex"
claude = "~/.claude"
opencode = "~/.local/share/opencode/opencode.db"
hermes = "~/.hermes/sessions"
cursor = "~/Library/Application Support/Cursor/User/workspaceStorage"
copilot = "~/Library/Application Support/Code/User/workspaceStorage"
```

