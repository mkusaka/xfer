# xfer

`xfer` prepares a token-bounded Markdown handoff from a Codex or Claude session JSONL file.
It keeps user messages and final assistant answers, removes tool trajectories and hidden runtime
context, then selects the largest contiguous suffix of complete turns that fits the budget.

The generated file is persisted in the operating system's temporary directory and its path is
printed to standard output. `xfer` does not start the receiving agent or perform semantic
summarization.

## Usage

```bash
xfer /path/to/session.jsonl \
  --task "Implement the requested change and run the relevant tests"
```

The default profile is `swe-1.7` with a 32,768-token total handoff budget. SWE-1.7 is counted with
the Kimi K2.7 tokenizer plus a 10% safety margin. GLM-5.2 uses its published tokenizer directly.
Tokenizer files are downloaded from Hugging Face on first use and then read from its local cache.

```bash
xfer /path/to/session.jsonl \
  --model glm-5.2 \
  --budget 65536 \
  --task "Continue the implementation"
```

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
