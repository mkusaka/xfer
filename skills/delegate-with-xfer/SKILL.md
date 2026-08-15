---
name: delegate-with-xfer
description: Prepare a token-bounded handoff from the current Codex or Claude session and delegate a task to another coding agent. Use when the user asks to pass current-session context to Devin CLI or another local agent, create a handoff prompt file, or continue work in a destination model such as SWE-1.7 or GLM-5.2.
---

# Delegate with xfer

1. Resolve the current session JSONL path using the host's session lookup capability. Do not guess among similarly recent files.
2. Write an explicit delegated task containing the requested outcome, scope, constraints, success criteria, and required checks. Do not paste repository contents that the receiving agent can inspect.
3. Select `swe-1.7` unless the receiving model is explicitly GLM-5.2. Keep the default 32,768-token budget unless the user requests another budget or provides a concrete context-limit reason.
4. Generate the handoff and capture the path printed to standard output:

```bash
handoff_path=$(xfer "$SESSION_JSONL" --model swe-1.7 --task "$DELEGATED_TASK")
```

5. Pass the file to the requested agent. For Devin CLI, use its prompt-file option rather than copying the Markdown through shell interpolation:

```bash
devin --prompt-file "$handoff_path"
```

6. Treat the receiving agent's working-tree diff and check results as the source of truth. Inspect them before accepting the work.

`xfer` removes tool trajectories and hidden runtime context, keeps real user messages and final assistant answers, and drops old turns until the complete handoff fits. It does not semantically summarize omitted turns. If the latest user message alone exceeds the budget, report the error instead of truncating it.
