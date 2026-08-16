---
name: delegate-with-xfer
description: Prepare a token-bounded handoff from the current Codex or Claude session for another coding agent, optionally using an external model to compact the session first. Use when the user asks to create a handoff prompt file, preserve a long current-session context for a local agent, or continue work in a destination model such as SWE-1.7 or GLM-5.2 without requesting a Devin-specific delegation workflow.
---

# Delegate with xfer

1. Write an explicit delegated task containing the requested outcome, scope, constraints, success criteria, and required checks. Do not paste repository contents that the receiving agent can inspect.
2. Check `xfer models` when selecting the destination model or budget. Use the model's default budget unless the user requests an exact token count or context percentage.
3. For a compacted handoff, generate a compaction prompt, run a non-mutating external summarizer, then pass its output back to xfer:

```bash
compact_prompt_path=$(xfer compact-prompt \
  --model swe-1.7 \
  --write-tmpfile)
summary_path=$(mktemp)
<summarizer> "$compact_prompt_path" > "$summary_path"

handoff_path=$(printf '%s' "$DELEGATED_TASK" \
  | xfer infer \
      --model swe-1.7 \
      --summary-file "$summary_path" \
      --write-tmpfile)
```

The summarizer must return only the summary and must not modify the workspace. Skip compaction and omit `--summary-file` for a recent-turn-only handoff. Use an explicit session path only when it is already known. Use `--task-file` for an existing task document and `--budget 12.5%` for a context percentage. Omit `--write-tmpfile` when the next tool reads generated Markdown directly from standard input.

4. Pass the file through the receiving agent's prompt-file or equivalent input option rather than copying the Markdown through shell interpolation.

5. Treat the receiving agent's working-tree diff and check results as the source of truth. Inspect them before accepting the work.

`xfer` removes tool trajectories and hidden runtime context before compaction. With a summary, the final handoff keeps the summary and recent user messages but does not repeat assistant answers. If the latest user message alone exceeds the budget, report the error instead of truncating it.
