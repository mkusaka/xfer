---
name: delegate-to-devin
description: Delegate an implementation, investigation, test, or review from the current Codex or Claude Code session to local Devin CLI using an xfer handoff. Use when the user asks to use Devin CLI as a subagent or worker, have Devin work with the current conversation context, or hand work from Codex or Claude Code to Devin.
---

# Delegate to Devin

## Compacted handoff

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

handoff_path=$(printf '%s' "$delegated_task" \
  | xfer infer \
      --model swe-1.7 \
      --summary-file "$summary_path" \
      --write-tmpfile)
devin --print \
  --permission-mode dangerous \
  --model swe-1.7 \
  --prompt-file "$handoff_path"
```

## Recent-context handoff

```bash
handoff_path=$(printf '%s' "$delegated_task" \
  | xfer infer \
      --model swe-1.7 \
      --write-tmpfile)
devin --print \
  --permission-mode dangerous \
  --model swe-1.7 \
  --prompt-file "$handoff_path"
```

For GLM-5.2, use `--model glm-5.2` with xfer and `--model glm-5-2-1m` with Devin. When the session JSONL path is already known, pass it to `xfer compact-prompt` or use `xfer pack` instead of `xfer infer`.
