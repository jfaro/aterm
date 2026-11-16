---
name: aterm-orchestration
description: Spawn and drive sibling coding-agent panes from inside an aterm pane - creating new panes, sending them text/keys, waiting on agent status, and reading their output. Use whenever the user asks to open/spawn/start a new agent, pane, or terminal, to split off a sub-task into another pane, or to check on/message a sibling pane, and ATERM_SOCKET is set in the environment.
---

# aterm orchestration

If the `ATERM_SOCKET` environment variable is set, you are running inside an
`aterm` pane, and the `aterm` CLI on `PATH` lets you create and control
sibling panes (including other agents) in the same session. `ATERM_PANE`
holds your own pane id.

Check first: `echo "$ATERM_SOCKET" "$ATERM_PANE"`. If `ATERM_SOCKET` is
unset, this skill doesn't apply — you're not inside aterm.

## Spawning a new agent pane

```sh
aterm pane create -- claude            # prints the new pane's id, e.g. "4"
aterm pane create --folder 1 -- claude # start it inside folder 1 instead of top level
aterm pane create --cwd /path/to/repo -- claude
```

Use a folder to group a task's panes together (`aterm folder create <name>`
prints a folder id).

## Sending it work

```sh
aterm pane send-text 4 'fix the failing test in src/foo.rs' --enter
aterm pane send-keys 4 down enter      # reply to a prompt, navigate a menu, etc.
```

## Waiting for it and reading results

```sh
aterm agent wait 4 --status idle --timeout 600   # blocks until the agent goes idle (or times out)
aterm pane read 4 --lines 50                      # read its terminal output
```

Agent status is one of `working`, `blocked`, `idle`. Poll with `aterm pane
read` or `aterm pane list` instead of guessing timing.

## Cleaning up

```sh
aterm pane close 4
```

## Notes

- Everything here is also available as a socket API (NDJSON over
  `$ATERM_SOCKET`) if a raw script is more convenient than shelling out to
  the CLI — see this repo's README for the method table.
- Don't spawn a pane for work you can just do yourself in-process; reach for
  this when the task is genuinely a separate, parallel, or long-running
  agent run (e.g. a background investigation, a second agent working a
  different part of the repo).
- `aterm pane list` / `aterm folder list` show what already exists before
  you create more panes.
