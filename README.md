# Skillshard

A desktop app for managing AI coding agent skills, built with Rust, [GPUI](https://gpui.rs)
and [GPUI Kit](https://gpui-kit.com).

Skills are installed by the [`skills` CLI](https://skills.sh), which is excellent at
fetching them and has no opinion about what happens next. Skillshard is the missing
other half: one window showing every skill across every agent, with the switches to
turn them on and off, keep agents in sync, and move a skill between global and project
scope.

## What it does

* **See everything at once** — every skill in a scope, which agents load it, where the
  files are, and where it came from.
* **Enable / disable** without uninstalling. A disabled skill is parked outside every
  agent's reach and restored, agents and all, when you switch it back on.
* **Per-agent control** — tick an agent to link the skill into it, untick to unlink.
  The canonical copy is never touched.
* **Sync across agents** — bring one skill's coverage to every agent you use in a
  single click.
* **Move between global and project scope** — done by reinstalling through the CLI so
  both lock files stay correct and update tracking survives the move.
* **Update status** — a read-only check against each skill's upstream, so you can see
  what is stale before changing anything. Updating itself is delegated to the CLI.
* **Install with a security review** — every option (source, skills, agents, scope,
  symlink or copy) is chosen up front, and the partner security audit for the source is
  shown *before* you commit. The resulting `npx skills add …` runs start to finish with
  no interactive prompt.

## Requirements

* Rust 1.90+
* Node.js, for `npx skills`. A `skills` binary on `PATH` is used instead when present.
* Linux, macOS or Windows. On Linux, GPUI needs Vulkan, Wayland or X11, xkbcommon and
  fontconfig.

## Running

```bash
cargo run
```

## Tests

```bash
cargo test                                     # unit + headless UI tests, all offline
cargo test --test live_api -- --ignored        # hits skills.sh and GitHub
cargo test --test live_install -- --ignored    # a real install into a temp project
```

The UI tests drive the real window headlessly through GPUI Kit's test harness: they
click actual controls and assert the effect on disk, so `cargo test` needs no display.

## How it is put together

Install, update and uninstall logic is never reimplemented — it is delegated to the
`skills` CLI. Skillshard adds the cross-agent view and the operations the CLI has no
command for.

| Module | Responsibility |
| --- | --- |
| `agents` | The 79 agents the CLI supports, and their skills directories |
| `scan` | Reads the filesystem: what exists, and which agents load it |
| `lock` | Reads the CLI's two lock files (read-only) |
| `ops` | Enable/disable, link/unlink — the filesystem work Skillshard owns |
| `skills_cli` | Builds and runs fully-specified, non-interactive CLI commands |
| `registry` | skills.sh search and the partner security audit |
| `updates` | Read-only update checks against the GitHub trees API |
| `ui` | The window, and the install dialog |

The filesystem is the source of truth; lock files are metadata. They drift apart in
normal use — a locked skill can be missing from disk, an installed skill can have no
lock entry — and the UI surfaces that rather than hiding it.

`docs/skills-sh-integration.md` records how the CLI actually behaves: its commands,
where it puts files, both lock formats, the audit API and the update-check algorithm.
Most of it is not in the published docs.

## A note on disabling

Roughly 22 of the supported agents (Cursor, Codex, OpenCode, Amp and friends) read
`.agents/skills` directly rather than keeping their own copy. They have no link of
their own to remove, so they cannot be toggled individually — they follow the skill's
enabled state. The detail pane lists them separately instead of showing a switch that
would not work.
