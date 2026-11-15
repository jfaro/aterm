# aterm

A minimal terminal multiplexer for coding agents. It runs agents (or any
command) in panes, works out what each agent is doing (`working`, `blocked`,
`idle`), and exposes everything over a socket API so agents can orchestrate
each other.

## Usage

```sh
aterm                                   # attach the TUI (starts the server if needed)

aterm pane create -- claude             # start a pane, prints its id
aterm pane list
aterm pane read 1 --lines 20
aterm pane send-text 1 'fix the failing test' --enter
aterm pane send-keys 1 down enter       # named keys: enter, esc, ctrl-c, alt-left, f5, …
aterm pane move 3 1                     # move pane 3 to the top of the sidebar
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

### TUI keys

Prefix is `ctrl-b`, followed by:

| key           | action                           |
| ------------- | -------------------------------- |
| `c`           | new shell pane                   |
| `a`           | new pane running a typed command |
| `n`/`j`/`↓`   | next pane                        |
| `p`/`k`/`↑`   | previous pane                    |
| `1`–`9`       | focus pane by position           |
| `J`/`shift-↓` | move focused pane down           |
| `K`/`shift-↑` | move focused pane up             |
| `r`           | rename pane                      |
| `x`           | close pane                       |
| `d`/`q`       | detach (panes keep running)      |
| `ctrl-b`      | send a literal `ctrl-b`          |

## Architecture

```
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
| `pane.create`     | `command?`, `cwd?`, `name?`, `rows?`, `cols?` |
| `pane.list`       |                                              |
| `pane.get`        | `pane`                                       |
| `pane.read`       | `pane`, `format?: text\|ansi`, `lines?`      |
| `pane.send_text`  | `pane`, `text`                               |
| `pane.send_keys`  | `pane`, `keys: [..]`                         |
| `pane.resize`     | `pane`, `rows`, `cols`                       |
| `pane.rename`     | `pane`, `name?` (omit or blank to auto-name) |
| `pane.move`       | `pane`, `index` (0-based, clamped)           |
| `pane.close`      | `pane`                                       |
| `agent.wait`      | `pane`, `status`, `timeout_ms?`              |
| `events.subscribe`|                                              |

Events (`{"event": …, "data": {"pane": id, …}}`): `pane.created`, `pane.updated`,
`pane.moved`, `pane.output_changed`, `pane.agent_status_changed`, `pane.exited`, `pane.closed`.

### Detection manifests

Each manifest is `manifests/<agent>.toml`. Rules are tried from highest
`priority` to lowest, and the first one that matches decides the status. If
none match, the agent is `idle`. Regions are `title`, `bottom(N)` or `screen`.
A rule matches when all of its `contains`/`regex` match, at least one `any`
entry matches, and no `not` entry matches. Check new patterns against real
screens with `aterm pane read <pane>`.

## Development

```sh
nix develop
cargo test
ATERM_SOCKET=/tmp/aterm-dev/s.sock cargo run   # isolated from your real server
```

The server logs to `aterm.log` next to its socket (`/tmp/aterm-$UID/` by default).
