# Fainder on Linux

Fainder runs on Linux (x86_64 and aarch64, glibc 2.31+: Ubuntu 20.04+,
Debian 11+) with the same commands as on macOS, including `fainder team`.

## Install

Prebuilt binary, no Rust needed:

```bash
mkdir -p ~/.local/bin
curl -fsSL "https://github.com/satelerd/fainder/releases/latest/download/fainder-$(uname -m)-unknown-linux-gnu.tar.gz" \
  | tar -xz -C ~/.local/bin
fainder --version
```

Make sure `~/.local/bin` is on your `PATH`. To update, run the same command
again. Each tarball has a `.sha256` next to it in the release.

Alternatives:

```bash
# With a Rust toolchain (1.85+)
cargo install --git https://github.com/satelerd/fainder --locked

# With Homebrew on Linux (builds from source)
brew install satelerd/tap/fainder
```

[ripgrep](https://github.com/BurntSushi/ripgrep) (`rg`) makes content search
faster when it is on `PATH`; without it Fainder scans the files itself.

## Where it looks

| Provider | Default path on Linux |
|---|---|
| Claude Code | `~/.claude` |
| Codex | `~/.codex` |
| OpenCode | `~/.local/share/opencode/opencode.db` |
| Hermes | `~/.hermes/sessions` |
| Cursor | `~/.config/Cursor/User/workspaceStorage` |
| GitHub Copilot | `~/.config/Code/User/workspaceStorage` |
| Kiro CLI | `~/.local/share/kiro-cli/data.sqlite3` |

`fainder doctor` shows what it found. If a harness keeps its data elsewhere,
override it in `~/.config/fainder/config.toml` (or `$XDG_CONFIG_HOME/fainder/`):

```toml
[paths]
kiro = "~/.local/share/kiro-cli/data.sqlite3"
```

Cursor and Copilot chats live on the machine running the editor, so on a
devbox reached through Remote-SSH they are usually not there.

## Team mode

Same as on macOS:

```toml
# ~/.config/fainder/config.toml
[team]
url = "https://admin.smartup.lat"
```

```bash
export FAINDER_TEAM_KEY=...   # personal operator key with devinsights:read
fainder team search "rollback" --since 2026-09-01
```

## Copying over SSH

`Enter` in the TUI and `--copy` copy the resume command. Over SSH (when
`SSH_TTY` or `SSH_CONNECTION` is set) or without a display, Fainder asks your
local terminal to copy it with an OSC 52 escape. Most terminals support it
(iTerm2, kitty, WezTerm, Ghostty, Windows Terminal; in iTerm2 enable
"Applications in terminal may access clipboard"). Inside tmux add:

```tmux
set -g set-clipboard on
```

## Building the Linux binaries

`scripts/build-linux.sh` builds both targets in Docker on Debian bullseye and
writes `dist/fainder-<arch>-unknown-linux-gnu.tar.gz`. After a release:

```bash
git checkout vX.Y.Z
scripts/build-linux.sh --upload vX.Y.Z
```
