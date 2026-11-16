# aterm

A minimal terminal multiplexer for coding agents.

aterm runs agents (or any command) in panes, works out what each agent is doing
(`working`, `blocked`, `idle`), and exposes everything over a socket API so
agents can orchestrate each other.

## Usage

```sh
aterm                                   # attach the TUI (starts the server if needed)

# Panes.
aterm pane create -- claude             # start a pane, prints its id
aterm pane list
aterm pane read 1 --lines 20
aterm pane send-text 1 'fix the failing test' --enter
aterm pane send-keys 1 down enter       # named keys: enter, esc, ctrl-c, alt-left, f5, …
aterm pane move 3 1                     # move pane 3 to the top of its folder (or the top level)

# Folders.
aterm folder create backend             # prints the folder's id
aterm pane create --folder 1 -- claude  # start a pane inside folder 1
aterm pane move 3 1 --folder 1          # move pane 3 to the top of folder 1
aterm pane move 3 1 --top-level         # … or out of its folder
aterm folder rename 1 api
aterm folder delete 1                   # its panes move to the top level

# Monitoring.
aterm agent wait 1 --status idle --timeout 600
aterm events                            # stream events as JSON lines
aterm server stop
```

Processes inside a pane get `ATERM_PANE` and `ATERM_SOCKET`, so an agent can
drive its sibling panes with the same CLI.

Panes name themselves until you name them: the sidebar shows the window title
the program sets (e.g. Claude Code's task summary), falling back to the
detected agent and then the program name. Renaming pins the name; renaming to
an empty name (`ctrl-b r`, clear, enter, or `aterm pane rename <id>`) goes
back to automatic naming.

Folders group panes in the sidebar, below the panes that aren't in one. A
folder always has a name, and deleting it moves its panes to the top level
rather than closing them. Folders are one level deep, and like panes they
live as long as the server.

### TUI keys

Prefix is `ctrl-b`, followed by:

| key           | action                                              |
| ------------- | --------------------------------------------------- |
| `c`           | new shell pane in the current folder                |
| `a`           | new pane running a typed command, likewise          |
| `f`           | new folder (asks for its name)                      |
| `n`/`j`/`↓`   | next pane or folder                                 |
| `p`/`k`/`↑`   | previous pane or folder                             |
| `1`–`9`       | focus pane by position                              |
| `J`/`shift-↓` | move focused pane down, into the next folder at its end |
| `K`/`shift-↑` | move focused pane up, into the previous folder at its start |
| `r`           | rename the selected pane or folder                  |
| `x`           | close the selected pane, or delete the selected folder |
| `<`/`>`       | shrink/grow sidebar, enter resize mode              |
| `b`           | hide/show sidebar                                   |
| `d`/`q`       | detach (panes keep running)                         |
| `ctrl-b`      | send a literal `ctrl-b`                             |
| `?`           | show all keys (any key closes it)                   |

The current folder is the selected folder, or else the focused pane's folder.
Selecting a folder row keeps the last pane on screen, and typing goes back to
that pane.

In resize mode, `<`/`>`, `h`/`l` or `←`/`→` keep resizing by 2 columns
(`H`/`L` or `shift-←`/`shift-→` by 8), `=` resets to the default width, and
any other key leaves the mode without reaching the pane. The sidebar is at
least 16 columns wide and always leaves the pane 20; set its starting width
with `aterm --sidebar-width N`.

## Architecture

```txt
 TUI client ─┐                      ┌─ PaneRuntime: PTY + child + vt100 terminal
 CLI ────────┼─ NDJSON / unix sock ─┤      (reader thread feeds the emulator)
 agents ─────┘                      └─ monitor thread: output events, detection, reaping
```

- **Server owns the runtime** (`src/server`). The TUI is just another client.
  Shared facts (panes, agent status) live on the server; presentation state
  (focus, prompts) lives only in the client.
- **State is separate from runtime.** `server/state.rs` is pure data;
  `server/pane.rs` holds the PTY and terminal.
- **Render is pure.** `client/render.rs` draws a `View` and never mutates it.
- **Detection is decoupled.** `detect/` sees only a screen snapshot plus the
  window title. The agent is identified from the PTY's foreground process
  group; its status comes from TOML rules in `manifests/`.

### Socket API

One JSON object per line. Requests look like `{"id": 1, "method": "pane.list", "params": {}}`,
and replies look like `{"id": 1, "result": …}` or `{"id": 1, "error": "…"}`.

| method            | params                                       |
| ----------------- | -------------------------------------------- |
| `server.ping`     |                                              |
| `server.stop`     |                                              |
| `pane.create`     | `command?`, `cwd?`, `name?`, `rows?`, `cols?`, `folder?` |
| `pane.list`       |                                              |
| `pane.get`        | `pane`                                       |
| `pane.read`       | `pane`, `format?: text\|ansi`, `lines?`      |
| `pane.send_text`  | `pane`, `text`                               |
| `pane.send_keys`  | `pane`, `keys: [..]`                         |
| `pane.resize`     | `pane`, `rows`, `cols`                       |
| `pane.rename`     | `pane`, `name?` (omit or blank to auto-name) |
| `pane.move`       | `pane`, `index` (0-based within the group, clamped), `folder?` (omit to stay, `null` for the top level) |
| `pane.close`      | `pane`                                       |
| `folder.list`     |                                              |
| `folder.create`   | `name` (not blank)                           |
| `folder.rename`   | `folder`, `name` (not blank)                 |
| `folder.delete`   | `folder` (its panes move to the top level)   |
| `agent.wait`      | `pane`, `status`, `timeout_ms?`              |
| `events.subscribe`|                                              |

Events (`{"event": …, "data": {"pane": id, …}}`): `pane.created`, `pane.updated`,
`pane.moved`, `pane.output_changed`, `pane.agent_status_changed`, `pane.exited`, `pane.closed`.
Folder events carry `{"folder": id, …}`: `folder.created`, `folder.updated`, `folder.deleted`.
Deleting a folder also sends `pane.moved` for each of its panes.

### Detection manifests

Each manifest is `manifests/<agent>.toml`. Rules are tried from highest
`priority` to lowest, and the first one that matches decides the status. If
none match, the agent is `idle`. Regions are `title`, `bottom(N)` or `screen`.
A rule matches when all of its `contains`/`regex` match, at least one `any`
entry matches, and no `not` entry matches. Check new patterns against real
screens with `aterm pane read <pane>`.

## Claude Code skill

`.claude/skills/aterm-orchestration/SKILL.md` teaches an agent running
inside an aterm pane how to spawn and drive sibling panes (other agents)
using the CLI above. Claude Code only picks up skills from paths it's
configured to look at, so add it to one of:

- **This project only** — already in place; nothing to do. Claude Code
  reads `.claude/skills/` relative to the working directory.
- **Every project you work in** — copy or symlink the skill into your
  personal skills directory so it's always available:

  ```sh
  mkdir -p ~/.claude/skills
  ln -s "$(pwd)/.claude/skills/aterm-orchestration" ~/.claude/skills/aterm-orchestration
  ```

- **One other project** — copy the same directory into that project's
  `.claude/skills/`:

  ```sh
  cp -r .claude/skills/aterm-orchestration /path/to/other-project/.claude/skills/
  ```

Either way, no restart is required beyond starting a new Claude Code
session; the skill triggers automatically once `ATERM_SOCKET` is set and the
request matches its description (spawning a pane/agent, messaging or
checking on a sibling pane).

## Development

```sh
nix develop
cargo test
ATERM_SOCKET=/tmp/aterm-dev/s.sock cargo run   # isolated from your real server
```

The server logs to `aterm.log` next to its socket (`/tmp/aterm-$UID/` by default).
