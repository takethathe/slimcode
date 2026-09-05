# Skills are frontmatter-driven, scope-discovered, and trigger via `/skill:name`; the system prompt advertises them as a markdown index and re-triggers dedupe

Skills are installable agent instructions: a `SKILL.md` file whose YAML-style frontmatter carries `name`, `description`, and an optional `disable-model-invocation`. They are discovered from a user scope (`<home>/skills/`) and a project scope (`<cwd>/.slimcode/skills/`), with project winning on a name clash. Skills are triggered with the pi-style `/skill:name` command shape (bare `/name` is still accepted as a fallback so existing muscle memory keeps working; completion and suggestions surface the canonical `/skill:name`). The system prompt advertises auto-invokable skills as a `## Skills` markdown index; a triggered skill's body is injected into the user message wrapped in pi-style XML, and re-triggering the same skill in one conversation is deduplicated.

## Trigger shape: `/skill:name`

A skill trigger is a `/`-prefixed line resolved by the same prediction path as built-in commands, so unknown-slash completion and `did you mean` suggestions naturally include skills. The command registry itself is never coupled to file I/O: skill dispatch consults the discovered skill store instead.

## System prompt advertisement

Skills with `disable-model-invocation` omitted or `false` are advertised in the system prompt as a `## Skills` markdown index, one bullet per skill:

```
## Skills

Use a skill when its name or description matches the task, or when the user
references it explicitly as /{name}. Read the file and follow its instructions.
When a skill file references a relative path, resolve it against the skill
directory and use the absolute path in tool commands.

- grill: stress-test a plan [Read from /path/to/skills/grill/SKILL.md]
```

Each bullet co-locates the trigger (name + description) with how to reach the
full instructions: `[Read from …]` is the skill's `SKILL.md` file path (the
`Skill.file` field), so the model can `read` it on demand. Descriptions are
flattened to a single line. A skill marked `disable-model-invocation: true` is
omitted entirely, so the model does not auto-discover it and can only run it
through an explicit `/skill:name` trigger.

The model faces skills two ways: implicitly (apply a skill whose name or
description matches the task) or explicitly (the user references it as `/{name}`
in a prompt). The `/skill:` prefix is a frontend spelling only — in the TUI it is
a command and the skill content is embedded; in the one-shot CLI (which has no
command parser) a leading `/skill:{name}` is normalized to `/{name}` before it
becomes a user message.

## User-message injection and dedup

A `/skill:name` trigger injects the skill into that turn's user message as a
pi-style `<skill>` block:

```
<skill name="grill" location="/path/to/skills/grill/SKILL.md">
References are relative to /path/to/skills/grill.

<body>
</skill>
```

with an optional `\n\n<task>` appended when an argument is given. Because the
whole message list is in context, re-loading the same skill in a later message
would duplicate its body. The `ContextBuilder` therefore scans the message
history for the injected `<skill name="<name>"` marker: if the skill was already
loaded earlier, the re-trigger keeps the `<skill name location>` wrapper and the
`References are relative to <dir>.` line but replaces the body with an
"already loaded" notice pointing at the earlier message, so the model finds the
instructions there instead of reloading. The scan is stateless (marker-based),
so it survives session reload via `/load`.

## Keeping file I/O out of the command registry

Discovery, parsing, and installation all live in the skill store; the command
registry stays a static table. This keeps skills installable per user/project and
discoverable through the same prediction path as commands, without coupling the
command registry to file I/O.
