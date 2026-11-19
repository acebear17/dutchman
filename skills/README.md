# Agent skills

Skills for AI assistants that convert CAD files with Dutchman, through its
[MCP server](../docs/mcp.md) or the `dutchman` command. Each folder is one skill in the
[Agent Skills](https://agentskills.io) format: a `SKILL.md` whose name and description tell
the assistant when to load it.

| Skill | Loads when the assistant |
|---|---|
| [`dutchman-converting`](dutchman-converting/SKILL.md) | is asked to turn a Fusion, Inventor or SolidWorks file into a SolidCraft design |

It teaches the assistant to:

- check the format, and say plainly what to do with formats Dutchman does not convert
- inspect a file before converting it
- check the result in SolidCraft instead of trusting the reply alone
- explain the loss report in plain words
- keep the user's designs on the user's machine

Set up the MCP server first ([docs/mcp.md](../docs/mcp.md)). To check results in SolidCraft,
the skill also uses `solidcraft-cli` or SolidCraft's own MCP server; SolidCraft's skills
(in the SolidCraft repository's `skills/` folder) cover editing the converted design.

## Install

Release packages already include this folder and the referenced Markdown docs. Start an
assistant in the install folder and read its `AGENTS.md`, or copy this skill into your
assistant's skill directory. No repository download is needed for the bundled skill.

Claude Code, for all projects:

```sh
git clone --depth 1 https://github.com/dutchman-cad/dutchman
mkdir -p ~/.claude/skills
cp -r dutchman/skills/dutchman-converting ~/.claude/skills/
```

Windows (PowerShell):

```powershell
git clone --depth 1 https://github.com/dutchman-cad/dutchman
New-Item -ItemType Directory -Force $HOME\.claude\skills | Out-Null
Copy-Item -Recurse dutchman\skills\dutchman-converting $HOME\.claude\skills\
```

- **One project only:** copy the folder into that project's `.claude/skills/` instead.
- **Other assistants:** for those that read the Agent Skills format, copy the folder into the
  skills directory their documentation names.
- **Updating:** copy the folder again after you update Dutchman.

## Maintaining

When a tool, an argument, a report field or a message these files describe changes, update
them in the same pull request.

The [SolidCraft component front end](../docs/plugin.md) uses a host-drawn options form and
returns SolidCraft's loss report (items only for what the design lost, every note under
`dutchman.plugin.loss/1` details); it converts Fusion, Inventor and SolidWorks, not FreeCAD. The
conversion skill's CLI/MCP workflow is unchanged.
Plugin authoring and evaluator integration details are documented on that page.

The plugin aborts on host input failures before writing output. Its bounded loss report
prioritizes dropped and degraded losses and includes an explicit truncation notice and
`details.truncation` counts when shortened; see [the plugin contract](../docs/plugin.md).

The SolidCraft plugin keeps unresolved appearance losses and unknown degraded codes in its
loss items; only known source-bookkeeping diagnostics and informational notes are suppressed.
For input limits, both the plugin and CLI cap each file at 1 GiB. The plugin also caps an
assembly's combined input at 1 GiB; the CLI can convert larger totals when each file fits.
Save smaller parts in the source program when one file exceeds the shared cap.

Plugin refusals give a next action without raw decoder, writer, or option-parser details.
SolidCraft 0.44.1 adds its own typed error prefix. These messages do not change the
CLI/MCP error reports or the `dutchman.plugin.loss/1` fields.
