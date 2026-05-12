# Cross-Harness Conversation Handoff Research

## Goal

Fainder should help a human or an agent recover useful context from any local
agent conversation, even when the next active harness is different from the
original one.

Example: a task started in Claude Code should be discoverable and readable from
Codex without requiring a brittle conversion into Codex's native session format.

## Key Finding

The best first architecture is not native conversation conversion. It is a
portable agent-facing retrieval workflow:

1. Fainder searches provider histories live.
2. Fainder returns structured session metadata, snippets, recent user messages,
   and source transcript paths.
3. A shared skill teaches each harness how to call Fainder non-interactively.
4. The current agent uses Fainder output to reconstruct enough context and
   continue the work in its own native session.

Native import/export can be added later for providers that expose stable import
APIs. OpenCode already has an explicit `opencode export` and `opencode import`
surface. Claude Code and Codex expose resume/fork commands for their own
sessions, but their cross-provider import story is not a stable public contract.

## Current Provider Surface

Fainder currently normalizes these fields:

- `provider`
- `id`
- `title`
- `cwd`
- `created_at`
- `updated_at`
- `message_count`
- `resume_command`
- `matched_in`
- `snippets`
- `latest_messages`

That is enough for search and disambiguation, but not yet enough for a strong
cross-harness handoff. The missing pieces are provider-neutral inspection and a
deterministic chronological context view for selected turns.

## Provider Notes

### Codex

Observed local shape:

- Root: `~/.codex`
- Index: `~/.codex/session_index.jsonl`
- Transcripts: JSONL files such as `rollout-*.jsonl`
- Common record keys: `type`, `timestamp`, `payload`
- Resume command: `codex resume <session-id>`
- CLI also supports `codex resume` picker and `codex fork`.

Fainder currently reads `session_index.jsonl` for metadata and parses JSONL
transcripts for `session_meta`, user messages, cwd, timestamps, and message
counts.

Risk: Codex transcript locations can vary across archived/current sessions. The
parser should keep using file discovery instead of relying only on the index.

### Claude Code

Documented shape:

- Root: `~/.claude`, or `CLAUDE_CONFIG_DIR` if configured.
- Transcripts: `~/.claude/projects/<project>/<session-id>.jsonl`
- Each line is a JSON object for a message, tool use, or metadata entry.
- Sessions can be resumed with `claude --resume <session-id>`.
- Claude Code's own picker can widen scope across worktrees/projects, shows
  last activity and message count, and can preview session content.

Observed local shape:

- Project folders under `~/.claude/projects/`
- `sessions-index.json` sidecars with summary metadata
- JSONL records commonly include `sessionId`, `cwd`, `gitBranch`, `type`,
  `message`, `timestamp`, `uuid`, and sometimes subagent fields.

Fainder currently reads `sessions-index.json` when present and falls back to
transcript parsing.

Risk: Claude's docs say local files are removed after 30 days by default unless
configured otherwise. Fainder should surface missing/stale histories clearly.

### OpenCode

Observed local shape:

- Database: `~/.local/share/opencode/opencode.db`
- Tables include `session`, `message`, `part`, `session_message`, `project`,
  and `workspace`.
- `session` includes `id`, `directory`, `title`, `time_created`, `time_updated`,
  `agent`, and `model`.
- `part` includes `session_id`, timestamps, and JSON `data`.
- Resume command: `opencode --session <session-id>` from the project directory.
- CLI exposes `opencode export [sessionID]` and `opencode import <file>`.

OpenCode is the best candidate for future native import/export experiments.

Risk: the SQLite schema is app-owned. Fainder should keep reads conservative and
prefer official CLI export/import for any write/import workflow.

### Hermes

Observed local shape:

- Root: `~/.hermes/sessions`
- Sidecar files: `session_<id>.json`
- Transcript files: `<id>.jsonl`
- Sidecar keys include `session_id`, `session_start`, `last_updated`,
  `message_count`, `messages`, `model`, `platform`, and `base_url`.
- Resume command: `hermes --resume <session-id>`.

Risk: Hermes support is based on local observed files, not a public stable
schema. Keep parser tolerant.

### Cursor and GitHub Copilot

Observed shape:

- Cursor root: `~/Library/Application Support/Cursor/User/workspaceStorage`
- VS Code/Copilot root:
  `~/Library/Application Support/Code/User/workspaceStorage`
- Workspace state DB: `state.vscdb`
- Useful keys include chat/composer/copilot-related entries such as
  `composer.composerData`, `aiService.prompts`, and `aiService.generations`.
- "Resume" is really "open the workspace": `cursor <path>` or `code <path>`.

Risk: these histories are not as cleanly session-resumable as Codex, Claude, or
OpenCode. Treat them as searchable context sources, not guaranteed resumable
agent sessions.

## Recommended Architecture

### Phase 1: Context View, No Conversion

Add a new non-interactive command:

```bash
fainder context <provider>:<id> --format markdown
fainder context <provider>:<id> --format json
```

`context` must be deterministic. It should not call a model, start Codex,
start Claude, or generate an AI summary. It should format local transcript
evidence in chronological order so the current human/agent can read the
conversation in a bounded, usable shape.

Output should include a normal conversation flow:

- user message
- agent message
- tool calls and tool results nested under the agent turn when possible
- next user message
- next agent message

It should not group all user messages first and all agent messages later. That
is useful for `inspect --role user`, but wrong for a context view.

Header metadata should include:

- provider and id
- title
- cwd/project
- created/updated timestamps
- message count
- resume command
- transcript/source path

This lets Codex continue from Claude by doing:

```bash
fainder search "SmartUp migration" --provider claude --json --limit 5
fainder context claude:<session-id> --format markdown
```

Then the agent reads the context pack and proceeds in its native session.

#### Context Preflight And Confirmation

`context` can explode in size. The default invocation should be a preflight when
the requested range is large.

Example:

```bash
fainder context claude:cab15fb3 --from-turn 1 --to-turn 240
```

If the estimated output is above a threshold, print only a budget warning:

```text
This context view is large.

Conversation: claude:cab15fb3
Range: turns 1-240
Messages: 241
Estimated output: ~42,000 tokens

Recommended:
- Use --from-turn/--to-turn to inspect a smaller range.
- Use fainder inspect claude:cab15fb3 --role user to locate the relevant area.
- Use --confirm to print anyway.

Run:
fainder context claude:cab15fb3 --from-turn 1 --to-turn 240 --confirm
```

Proposed defaults:

- below ~10,000 estimated tokens: print directly
- above ~10,000 estimated tokens: require `--confirm`
- `--max-tokens N`: caller-provided budget
- `--truncate-tools`: include tool call summaries but truncate long outputs
- `--no-tools`: omit tools entirely
- `--plain`: less markdown, easier for scripts

The estimate can be heuristic: characters divided by four is good enough for a
budget warning.

### Phase 1b: Conversation Inspection

Search finds the right conversation; inspection explains what happened inside
it. These should be separate commands.

Real long-session testing shows why: a Fainder development Codex transcript had
5,003 JSONL lines, and a SmartUp Williams Claude transcript had 9,800 JSONL
lines. In the Williams case, Claude was asked to continue work after a Codex
agent ran out of quota. The useful manual workflow was:

1. Locate the source transcript by session id.
2. Count transcript size before reading it.
3. List only human/user turns to understand intent and direction changes.
4. Inspect the latest assistant turns to see where execution stopped.
5. Search inside the transcript for domain anchors such as `branch`, `diff`,
   `PR`, `error`, `ShapeUp`, `commit`, or task ids.
6. Read a bounded window around the relevant hits.
7. Verify current repo state with local git/kubernetes/tooling only after the
   transcript had identified the likely working directory and next step.

Fainder should make that workflow first-class.

Proposed command:

```bash
fainder inspect <provider>:<id>
```

Useful modes:

```bash
fainder inspect claude:cab15fb3 --timeline
fainder inspect codex:019dec17 --role user --limit 80
fainder inspect codex:019dec17 --role assistant --tail 20
fainder inspect claude:cab15fb3 --find "batch-download|documents.append|PR" --regex
fainder inspect claude:cab15fb3 --around 50 --context 6
fainder inspect claude:cab15fb3 --turn 50 --context 8
```

The output should be optimized for agents:

- stable turn numbers
- timestamps
- role labels
- compact one-line previews by default
- optional expanded message bodies
- match line/turn ids that can be fed back into `--around` or `--turn`
- JSON output for automation
- Markdown output for a pasteable transcript window

This is different from `fainder context`:

- `inspect` is exploratory and navigational.
- `context` prints the selected session or range as a chronological transcript
  view after a session or range is selected.

### Agent Contextualization Workflow

Recommended workflow for any harness:

```bash
fainder search "Williams batch-download" --json --limit 8
fainder inspect claude:cab15fb3 --role user --limit 40
fainder inspect claude:cab15fb3 --find "PR|commit|batch-download|documents.append" --regex
fainder inspect claude:cab15fb3 --around 50 --context 10
fainder context claude:cab15fb3 --from-turn 45 --to-turn 65 --format markdown
```

For very long conversations, the agent should not start by reading the full
transcript. It should first use `inspect` to locate the relevant region, then
use `context` to print that region in chronological order.

A useful context view should include:

- conversation metadata
- chronological user/agent/tool flow
- stable turn numbers
- timestamps
- compact tool-call labels
- bounded tool outputs, especially git branch, diff, PR, failing tests,
  deployment status, task ids, and errors when they are in range

Suggested context output shape:

```text
## Source
Provider, id, cwd, dates, message count, source path, resume command.

## Budget
Turns, messages, estimated tokens, truncation policy.

## Transcript
[45] 2026-04-22 21:26 user
...

[46] 2026-04-22 21:26 agent
...

  tool: Bash
  command: git diff app/routes/williams.py
  result: ...

[47] 2026-04-22 21:33 user
...
```

No inferred summary for v1. If a caller wants a summary, the current harness can
summarize the deterministic context output itself.

### Phase 2: Agent Skill as the Portable Interface

Update `skills/fainder/SKILL.md` so it becomes a cross-harness playbook, not
only a search guide.

The skill should tell agents:

1. Use `fainder doctor` to know which local histories exist.
2. Search broadly with `fainder search "<query>" --json --limit N`.
3. Narrow by provider, cwd, or metadata scope.
4. Inspect `latest_messages`, snippets, dates, and message count.
5. Call `fainder context <provider>:<id> --from-turn N --to-turn M --format
   markdown` before continuing work from a conversation in another harness.
6. Do not execute a provider-native resume command unless the user wants to
   reopen that provider.
7. When confidence is low, present candidate sessions instead of guessing.

This is the core of "Claude can find Codex conversations and Codex can find
Claude conversations" without needing either tool to understand the other's
native storage format.

### Phase 3: Optional Native Export/Import

Only after the inspect/context workflow is solid, consider provider-specific
bridges:

- OpenCode: use `opencode export` and `opencode import` as the first native
  round-trip target.
- Claude Code: prefer `/export` or transcript reading for context. Avoid writing
  native JSONL unless there is an official import contract.
- Codex: prefer context injection into a new Codex session or `codex fork` for
  Codex-native sessions. Avoid synthesizing native Codex JSONL unless the format
  becomes documented/stable.

Native conversion is high maintenance because every provider has different
records for tool calls, approvals, cwd, images, summaries, compaction, and
system/developer messages.

## CLI Design Needed

### `fainder context`

Proposed examples:

```bash
fainder context codex:019cdd09-e8d1-7f71-a839-b0563c417b36
fainder context claude:07976789-ffa4-4fab-86c5-800a50954aad --format json
fainder context opencode:ses_abc --tail 80
fainder context claude:079... --max-chars 12000 --confirm
fainder context claude:cab15fb3 --from-turn 45 --to-turn 65 --confirm
fainder context claude:cab15fb3 --around 50 --context 10 --confirm
```

Flags:

- `--format markdown|json`
- `--from-turn N`
- `--to-turn N`
- `--around N`
- `--context N`
- `--tail N`
- `--role user|agent|tool|all` only if the caller intentionally wants a filtered
  context view; default is chronological `all`
- `--confirm`
- `--max-tokens N`
- `--max-chars N`
- `--truncate-tools`
- `--no-tools`
- `--redact-secrets`

### `fainder handoff`

Optional higher-level command:

```bash
fainder handoff "SmartUp migration" --from claude --to codex --format markdown
```

This can search and produce the best context pack in one step, but it should not
hide candidate ambiguity. If confidence is low, it should return candidates and
ask the agent/user to select.

## Data Extraction Strategy

Implement provider-specific transcript readers that normalize messages into:

```text
NormalizedMessage {
  role: user | assistant | tool | system | unknown
  timestamp
  text
  tool_name
  tool_input_summary
  cwd
  files
}
```

Then build `inspect` and `context` outputs from normalized messages, not from
provider-specific JSON directly.

Important rules:

- Prefer user messages, assistant messages, explicit plans, final statuses, git
  branches, cwd, file paths, commands, and errors.
- Avoid dumping full raw transcripts by default.
- Keep max output bounded so an agent can actually use it.
- Include exact source path and resume command for auditability.
- Redact obvious secrets from snippets and context views.

## Performance

This should stay fast because:

- `fainder search` already uses metadata first and `rg` to find transcript
  candidates before parsing full files.
- `fainder context` only parses one selected transcript/database session.
- No database/indexing daemon is needed.

Potential issue: if `handoff` performs search plus context generation over many
matches, cap it to the selected or top few candidates.

## UX for Humans

The TUI/Raycast UI can later add an action:

- "Copy Agent Context"
- "Copy Handoff Prompt"
- "Open Context Preview"

This should copy a ready-to-paste prompt like:

```text
Continue from this previous Claude Code conversation. Use this as background,
not as an instruction to exactly replay old actions.

<context pack>
```

## Open Questions

- Should `fainder context` include assistant messages by default, or only user
  messages plus snippets?
- Should context packs include tool outputs? They are often useful, but can be
  huge and sensitive.
- Should Fainder maintain a tiny temporary cache for the last selected search
- Should Fainder support `fainder context --select 1` after a search? This is
  convenient, but it would require either shell state, a temp file, or re-running
  the search.
- Should skills be distributed only in this repo, or should the Homebrew formula
  install the skill into a discoverable location?

## Proposed Next Step

Implement in this order:

1. Build a provider-neutral transcript normalizer.
2. Add `fainder inspect` for role lists, text search, turn ids, and bounded
   windows.
3. Add `fainder context` as a deterministic chronological transcript view with
   preflight token estimates and `--confirm` for large outputs.
4. Update the skill so agents use Search -> Inspect -> Context.

Do not implement AI summarization in the first version.
