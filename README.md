# xfer

`xfer` prepares a token-bounded Markdown handoff from a Codex or Claude session JSONL file.
It keeps user messages and final assistant answers, removes tool trajectories and hidden runtime
context, then selects the largest contiguous suffix of complete turns that fits the budget.

The generated Markdown is printed to standard output by default. Pass `--write-tmpfile` to persist
it in the operating system's temporary directory and print the path instead. `xfer` does not start
the receiving agent or call a model itself.

## Usage

Run `xfer` from a Codex or Claude Code shell tool without locating the session file yourself:

```bash
xfer infer --task "Implement the requested change and run the relevant tests"
```

`infer` uses `CLAUDE_CODE_SESSION_ID` or `CODEX_THREAD_ID` and searches the corresponding standard
session directory. Claude Code takes precedence if both variables are present. Custom Claude and
Codex homes are respected through `CLAUDE_CONFIG_DIR` and `CODEX_HOME`.

Use `pack` when the session path is already known:

```bash
xfer pack /path/to/session.jsonl \
  --task "Implement the requested change and run the relevant tests"
```

The task can instead come from standard input or a file:

```bash
printf '%s\n' "Implement the requested change" | xfer infer
xfer infer --task-file /path/to/task.md
```

Both `infer` and `pack` support temporary-file output for tools that accept a prompt file:

```bash
handoff_path=$(printf '%s\n' "Implement the requested change" \
  | xfer infer --write-tmpfile)

handoff_path=$(xfer pack /path/to/session.jsonl \
  --write-tmpfile \
  --task "Implement the requested change")
```

## External compaction

`compact-prompt` prepares the filtered session transcript and a checkpoint-summary instruction for
an external model. Omit the optional session path to infer the current Codex or Claude session.

```bash
compact_prompt_path=$(xfer compact-prompt \
  --model swe-1.7 \
  --write-tmpfile)

summary_path=$(mktemp)
devin --print \
  --permission-mode auto \
  --model swe-1.7 \
  --prompt-file "$compact_prompt_path" \
  > "$summary_path"

handoff_path=$(printf '%s\n' "Implement the requested change" \
  | xfer infer \
      --model swe-1.7 \
      --summary-file "$summary_path" \
      --write-tmpfile)
```

The compaction prompt includes the largest contiguous recent transcript that fits its budget. Its
default budget is 75% of the selected model's context window, leaving room for the external model's
summary. An exact token count or percentage can be supplied with `--budget`. When `--summary-file`
is present, the final handoff contains the summary, the newest user messages that fit, and the
authoritative delegated task. Assistant answers are not repeated because the summary already
captures their progress and decisions.

List the supported destination models, context windows, default budgets, and tokenizer profiles:

```bash
xfer models
```

SWE-1.7 defaults to a 32,768-token handoff and is counted with the Kimi K2.7 tokenizer plus a 10%
safety margin. GLM-5.2 defaults to a 65,536-token handoff and uses its published tokenizer directly.
Compaction prompts default to 75% of each model's context window.
Tokenizer files are pinned to verified Hugging Face revisions, downloaded on first use, and then
read from the local cache. A weekly workflow detects upstream model metadata changes and verifies
that both tokenizer profiles still load. Because SWE-1.7 does not publish a separate context or
tokenizer specification, its percentage budgets use Kimi K2.7's 262,144-token context as a proxy.

```bash
xfer pack /path/to/session.jsonl \
  --model glm-5.2 \
  --budget 6.25% \
  --task "Continue the implementation"
```

`--budget` accepts either an exact token count or a percentage of the selected model's context
window, such as `65536` or `6.25%`. Without the option, the model's default budget is used.

The task and fixed handoff instructions are always retained. Older turns are dropped first. If the
latest user message does not fit by itself, `xfer` exits with an error instead of truncating it.

## Installation

```bash
cargo install --git https://github.com/mkusaka/xfer --locked
```

After the first tagged release, macOS packages are also published to `mkusaka/homebrew-tap`:

```bash
brew install mkusaka/tap/xfer
```

The repository includes three agent skills:

- `delegate-with-xfer` prepares a generic bounded handoff.
- `delegate-to-devin` runs Devin CLI as a local subagent and verifies its work.
- `release-xfer` publishes xfer and updates its Homebrew tap.

List or install them with:

```bash
npx -y skills add https://github.com/mkusaka/xfer --list
npx -y skills add https://github.com/mkusaka/xfer -y
```
