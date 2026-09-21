# How Skillshard uses skills.sh

Notes gathered from the `skills` CLI source (v1.7.0,
[vercel-labs/skills](https://github.com/vercel-labs/skills)) and verified
against a live install. The published docs at <https://www.skills.sh/docs>
cover only `skills add`, so this file records the rest.

## Division of labour

Skillshard does **not** implement install, update or uninstall logic. Anything
that fetches from a source shells out to the CLI. Skillshard owns only what the
CLI has no command for: a cross-agent view, enable/disable, per-agent linking,
and read-only update status.

| Action | Owner |
| --- | --- |
| Install / update / uninstall | `skills` CLI |
| Security review | skills.sh audit API (same data the CLI shows) |
| Move / copy between scopes | `skills add` (+ `skills remove` for a move) |
| Enable / disable | Skillshard (filesystem) |
| Link / unlink one agent | Skillshard (symlinks) |
| Update *status* | Skillshard (GitHub trees API, read-only) |

## Commands Skillshard runs

All are non-interactive. `-y` is always present, and `--json` (which the CLI
rejects without `-y`) is used wherever it is supported.

```
skills add <source> -s <skill>… -a <agent>… [-g] [--copy] -y --json
skills remove <skill> [-a <agent>…] [-g] -y
skills update <skill>… (-g|-p) -y
```

`--json` puts exactly one JSON array on stdout and redirects all human-facing
progress to stderr, so the output is safe to parse. Repeated `-s`/`-a` flags
accumulate, which is how names containing spaces stay unambiguous.

## Where skills live

* Canonical copy: `.agents/skills/<name>` (project) or `~/.agents/skills`
  (global).
* Agents are usually symlinked to it, e.g.
  `~/.claude/skills/x -> ../../.agents/skills/x`.
* A single-agent install instead writes a **copy** straight into that agent's
  directory, with no canonical copy at all. Skillshard handles both.
* About 22 agents (Cursor, Codex, OpenCode, Amp…) read `.agents/skills`
  *directly*. They have no link of their own, so they cannot be toggled
  individually — they follow the skill's enabled state. See
  `ops::is_togglable`.

## Lock files

| Scope | Path | Version | Hash |
| --- | --- | --- | --- |
| Global | `$XDG_STATE_HOME/skills/.skill-lock.json`, else `~/.agents/.skill-lock.json` | 3 | `skillFolderHash` — a GitHub tree SHA |
| Project | `<root>/skills-lock.json` | 1 | `computedHash` — SHA-256 of local files |

Skillshard only reads these; the CLI stays the sole writer. Lock and disk drift
apart in normal use (a locked skill can be missing, an installed skill can be
unlocked), so the filesystem is treated as the source of truth and the lock as
metadata.

## Update checking

`skills check` is an alias for `skills update` — it *performs* the update, so
there is no read-only way to ask "is anything outdated?". To show status
without changing anything, Skillshard repeats the CLI's own comparison:

```
GET https://api.github.com/repos/<owner>/<repo>/git/trees/<ref>?recursive=1
```

then finds the `tree` entry whose path is the skill's folder and compares its
SHA with `skillFolderHash`. A pinned `ref` is honoured, otherwise `main` then
`master`. Only GitHub sources with a tree SHA can be checked; everything else
is reported as "not tracked" rather than silently assumed current.

Applying an update is still `skills update`.

## Security review

The CLI fetches partner audits during `add`. Skillshard calls the same endpoint
so the verdict is visible *before* the user commits to installing:

```
GET https://add-skill.vercel.sh/audit?source=<owner/repo>&skills=<slug,slug>
```

Response is `{ skillSlug: { partner: { risk, alerts?, score?, analyzedAt } } }`
with partners such as `ath`, `socket`, `snyk` and `zeroleaks`. Risk is one of
`safe|low|medium|high|critical`.

Partners disagree in practice — a live lookup for `vercel-optimize` returned
`ath: safe`, `snyk: low` and `socket: critical` at once — so the UI leads with
the **worst** verdict and lists each partner underneath.

Skill names must be slugified to match the service's keys (lowercase, spaces
and underscores to `-`, drop everything else, collapse and trim dashes). See
`registry::to_slug`.

## Discovery

```
GET https://skills.sh/api/search?q=<query>&limit=<n>
```

returns `{ skills: [{ name, source, skillId, installs }] }`. `skills add
<source> --list` also lists a repository's skills, but it cannot be combined
with `--json`, so the search API is used for the install dialog instead.

## Agent registry

`src/agents.rs` is generated from the supported-agents table in the CLI's
README (79 agents). Regenerate it when the CLI adds agents; the paths must keep
matching the CLI's or the two tools will disagree about where skills live.
