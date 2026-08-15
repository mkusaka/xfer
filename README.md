# xfer

`xfer` prepares a token-bounded Markdown handoff from a Codex or Claude session JSONL file.
It keeps user messages and final assistant answers, removes tool trajectories and hidden runtime
context, then selects the largest contiguous suffix of complete turns that fits the budget.

The generated file is persisted in the operating system's temporary directory and its path is
printed to standard output. `xfer` does not start the receiving agent or perform semantic
summarization.

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

List the supported destination models, context windows, default budgets, and tokenizer profiles:

```bash
xfer models
```

SWE-1.7 defaults to a 32,768-token handoff and is counted with the Kimi K2.7 tokenizer plus a 10%
safety margin. GLM-5.2 defaults to 65,536 tokens and uses its published tokenizer directly.
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

The repository includes two agent skills. List or install them with:

```bash
npx -y skills add https://github.com/mkusaka/xfer --list
npx -y skills add https://github.com/mkusaka/xfer -y
```
