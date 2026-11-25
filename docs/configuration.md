# Configuration

On Linux/macOS, the app reads `$XDG_CONFIG_HOME/qued/config.yaml`, or
`~/.config/qued/config.yaml` when XDG_CONFIG_HOME is unset. On Windows it reads
`%APPDATA%\qued\config.yaml`. It does not create
or modify home-directory configuration automatically.

Only the `qued` directory is used. When upgrading from an earlier application
name, manually move your configuration directory to this location to retain
settings, custom agents, and private credentials. Existing directories are not
migrated or deleted automatically; preserve private file permissions.

```yaml
schema_version: 1
theme: mocha           # mocha or daylight
keybindings: vscode    # vscode, emacs, or a custom profile filename
agent: Planner         # last selected built-in or custom agent
agent_modes:            # remembered chat/agent mode by agent name
  Planner: agent        # pristine-install default
model: mock            # or a genai provider::model identifier
max_output_tokens: 32768 # requested output budget per model generation
model_output_limits: {} # optional exact provider::model overrides
models: [mock]         # suggestions shown by /models
providers: {}          # connections created by /connect
persist_sessions: false
recovery: true
```

Missing fields inherit embedded defaults. Unknown fields, invalid themes, and
unsupported schema versions warn and fall back to safe defaults. A bad binding
profile falls back to vscode. Model, agent, and each agent's Chat/Agent mode are
saved immediately and restored on the next launch; a removed custom agent falls
back to Planner. A pristine environment starts with Planner in Agent mode. After
the user selects a mode, that saved selection is restored as before. Other agents
without a saved mode default to read-only Chat mode.
Live Agent mode automatically applies and saves completed edits before Keep/Undo
review, including existing unsaved text. There is no approval-before-save option.
Unnamed documents use private temporary backing with normalized cross-platform path
handling until Ctrl+S/Save As chooses a real file; subsequent agent edits save that
file. `recovery: true` also
requires a durable pre-edit snapshot before auto-save and retains it after Keep/exit;
`recovery: false` explicitly disables that safeguard, not automatic saving.
`/theme daylight` changes the theme for the current run only.
Both themes include a lighter surface for the borderless editor, prompt, and
submitted messages; agent replies use the standard background.

## Output budgets

`max_output_tokens` is the requested output-token budget for each provider generation
(including tool arguments), defaulting to 32,768. `model_output_limits` overrides it
for an exact `provider::model` identifier:

```yaml
max_output_tokens: 32768
model_output_limits:
  openai::gpt-4o-mini: 16384
```

Values must be positive integers. Unknown namespaces and unqualified model keys
are rejected; custom provider namespaces must be defined in `providers`. Choose
limits supported by your provider/model—discovery does not supply verified output
capabilities. Unsupported limits can cause provider errors; Qued does not silently
clamp them or retry. Settings are captured when a request starts and apply to Chat,
Agent, and compaction. Changes to YAML take effect after restarting.

Long edits normally use targeted batches or complete-document chunks instead of
regenerating unchanged text in one response. Truncated responses still discard all
staged edits; higher output limits do not remove model/context or byte-size limits.

## Providers and credentials

`/connect` writes non-secret provider metadata to `config.yaml`. A custom
OpenAI-compatible connection can also be configured declaratively:

```yaml
model: company::coder
providers:
  company:
    name: Company Gateway
    protocol: openai_compatible
    base_url: https://llm.example.com/v1/
    credential:
      env: COMPANY_LLM_API_KEY
    models: [coder]
```

Exactly one credential source is required: `env` or `file`. `env`
contains the environment variable's **name**, not its value. For example,
`env: OPENAI_API_KEY` resolves the
current value when a request starts, so key rotation does not require a config edit.

Never place a raw key in `config.yaml`. File references point to an entry in
`credentials.yaml` in the same configuration directory. That fallback is unencrypted plaintext, created
atomically with mode 0600 on Unix; Qued rejects a permissive or symlinked file.
Use `/connect delete <provider-id>` to remove a provider and its file secret.
Environment variables are never modified by Qued.

## Editing bindings

Place overrides in `keybindings/<profile>.yaml` beneath the same configuration
directory. For example, `keybindings/vscode.yaml`:

```yaml
editor:
  ctrl+u: undo
  ctrl+r: redo
  alt+c: copy
  ctrl+x: none
```

Built-in profiles are loaded first, then overrides. A custom profile starts from
the vscode-style bindings. Keys use lower-case names and modifiers in
`ctrl+alt+shift+key` order; printable characters and arrow/home/end/delete/backspace
names are supported by event normalization. Global Ctrl+S, Ctrl+Q and Tab are
reserved and cannot be overridden by editor bindings. Multi-key chords and
chat-specific custom binding maps are not implemented.

Available editor actions: `left`, `right`, `up`, `down`, `home`, `end`, `delete`,
`backspace`, `kill_line`, `mark`, `select_all`, `cut`, `copy`, `paste`, `undo`,
`redo`, `none`.
Unknown actions reject the profile. `/keys` shows resolved bindings.

### Default profile subsets

VS Code: Ctrl+A/E line start/end, Ctrl+K deletes from the cursor to line end,
Ctrl+C/X/V internal copy/cut/paste, Ctrl+Z undo, Ctrl+Y or Ctrl+Shift+Z redo.

Emacs: Ctrl+B/F character movement, Ctrl+P/N line movement, Ctrl+A/E line
start/end, Ctrl+K delete to line end, Ctrl+D delete, Ctrl+H backspace, Ctrl+Space mark, Ctrl+W cut,
Alt+W copy, Ctrl+Y paste, Ctrl+Z undo. Escape clears the mark. These are familiar
editing subsets, not complete emulator/key-chord compatibility.

## Custom agents

Built-in agents live in the repository's `assets/agents/`, one Markdown file per
agent: `01-planner.md`, `02-blogger.md`, and any additional definitions. YAML frontmatter holds `name` and an
optional `description`; the Markdown body is the prompt, using the same format as
custom agents below. Names must be nonempty and unique (case-insensitive), and
built-in prompts must be nonempty.

The build automatically embeds all `.md` files in filename order, which is also
picker/rotation order. Numbered filenames keep the order explicit. Edit a file or
add another (for example, `03-reviewer.md`) and rebuild Qued—no Rust registry
changes are needed. Installed binaries need no external copy of the files. This
does not modify your home-directory configuration.

### Built-in writing guidance

- **Blogger** focuses on audience, voice, factual accuracy, search intent, natural
  keywords, descriptive headings, and accessibility—not keyword stuffing or promises
  of rankings. For a new complete blog post it is instructed to include valid YAML
  frontmatter, following your publishing schema when supplied. Without a schema it
  uses conservative draft fields: `title`, `description`, `slug`, `tags`, `draft: true`.
  It preserves existing/unknown metadata and publication state, does not invent
  authors, dates, or URLs, and does not add frontmatter to excerpts or unrelated text
  unless requested. Supply a site-specific example when field names matter.
- **Planner** separates requirements from assumptions and recommendations, and
  produces scoped, actionable plans with dependencies, risks, and testable acceptance
  criteria. It avoids invented approvals, deadlines, and unnecessary complexity.
- **Reviewer** prioritizes accuracy, clarity, structure, consistency, and accessibility.
  Essential corrections are distinguished from optional polish. Blog reviews may
  include SEO/frontmatter checks; unrelated documents are not forced into a blog template.

All three are mode-aware: Chat provides discussion or suggestions; Agent may make
requested revisions through the structured edit tool. Asking only for review should
not trigger speculative edits. Research requires local permission, and unsupported
claims should be flagged. These are model instructions, not guaranteed output checks:
inspect generated text, facts, and frontmatter before publishing. Mock mode does not
run these prompts through a model.

### Runtime custom agents

For runtime custom agents, create `agents/copyeditor.md` beneath the configuration directory:

```markdown
---
name: Copyeditor
description: Reviews clarity and consistency
---

Preserve the writer's meaning and voice. Flag claims needing verification.
In Chat mode, suggest corrections. In Agent mode, make requested revisions
through the structured edit tool; do not edit when asked only for feedback.
```

Names must be nonempty and unique, including against all built-in definitions. Files with
invalid frontmatter produce a warning while valid files continue loading.
`/agents` opens the picker; `/agents Copyeditor` selects one. Shift+Tab cycles all
agent/mode pairs: Chat then Agent for each loaded agent before moving to the next.
Custom agents join automatically after the built-ins in filename order. Ctrl+G
cycles modes for the current agent only. Selection and per-agent modes are saved.

In live mode, the selected agent's instructions become the system prompt. Agent
instructions never override local permissions or edit-review validation.
API keys come from provider environment variables, never this YAML.
See [live AI](live-ai.md) for provider setup, privacy, and permissions.
