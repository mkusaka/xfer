---
name: delegate-with-xfer
description: Prepare a token-bounded handoff from the current Codex or Claude session and delegate a task to another coding agent. Use when the user asks to pass current-session context to Devin CLI or another local agent, create a handoff prompt file, or continue work in a destination model such as SWE-1.7 or GLM-5.2.
---

# Delegate with xfer

1. Write an explicit delegated task containing the requested outcome, scope, constraints, success criteria, and required checks. Do not paste repository contents that the receiving agent can inspect.
2. Check `xfer models` when selecting the destination model or budget. Use the model's default budget unless the user requests an exact token count or context percentage.
3. From a Codex or Claude Code shell tool, generate the handoff by inferring the current session and capture the path printed to standard output:

```bash
handoff_path=$(printf '%s' "$DELEGATED_TASK" | xfer infer --model swe-1.7)
```

Use `xfer pack "$SESSION_JSONL"` only when the session file is explicitly known. Use `--task-file` for an existing task document and `--budget 12.5%` when the handoff should occupy a percentage of the selected model's context.

4. Pass the file to the requested agent. For Devin CLI, use its prompt-file option rather than copying the Markdown through shell interpolation:

```bash
devin --prompt-file "$handoff_path"
```

5. Treat the receiving agent's working-tree diff and check results as the source of truth. Inspect them before accepting the work.

`xfer` removes tool trajectories and hidden runtime context, keeps real user messages and final assistant answers, and drops old turns until the complete handoff fits. It does not semantically summarize omitted turns. If the latest user message alone exceeds the budget, report the error instead of truncating it.
