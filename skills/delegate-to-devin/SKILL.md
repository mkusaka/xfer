---
name: delegate-to-devin
description: Delegate a scoped implementation, investigation, test, or review from the current Codex or Claude Code session to local Devin CLI using an xfer handoff, then verify Devin's working-tree changes and checks. Use when the user asks to use Devin CLI as a subagent or worker, have Devin implement or inspect something with the current conversation context, or hand work from Codex or Claude Code to Devin.
---

# Delegate to Devin

Use Devin CLI as a serial local worker in the current working tree. Keep final review, correction, commit, push, and completion judgment with the parent agent.

## Prepare the delegation

1. Inspect the current working tree before launching Devin. Preserve existing changes and do not edit the same files concurrently while Devin runs.
2. Confirm `xfer` and `devin` are available. Run `devin auth status`, `xfer models`, and `devin models list` without exposing credentials.
3. Write a self-contained delegated task with the outcome, scope, constraints, success criteria, and required checks. State that Devin must not commit, push, create a pull request, or revert unrelated changes unless the user explicitly requested that action.
4. Use SWE-1.7 unless the user selects GLM-5.2. Match the handoff profile to the actual Devin model:

| Destination | xfer model | Devin model |
| --- | --- | --- |
| SWE-1.7 | `swe-1.7` | `swe-1.7` |
| GLM-5.2 1M | `glm-5.2` | `glm-5-2-1m` |

Require the exact Devin model to appear in `devin models list`. Do not pair xfer's 1M GLM profile with Devin's 200K `glm-5.2` default when using percentage budgets.

## Run Devin

Generate the handoff from the current host session. Use `xfer pack` only when the exact session path is already known.

```bash
handoff_path=$(printf '%s' "$delegated_task" \
  | xfer infer --model swe-1.7 --write-tmpfile)
devin --print \
  --permission-mode smart \
  --model swe-1.7 \
  --prompt-file "$handoff_path"
```

Use `smart` for implementation because it permits workspace edits and safety-checks other actions. Use `auto` for read-only investigation. Never switch to `dangerous` or disable workspace trust unless the user explicitly authorizes it. If a required action is denied, report the denial or narrow the task instead of silently broadening permissions.

Keep the parent agent idle with respect to working-tree mutations until the command finishes. Devin's final text is a report, not proof of completion.

## Verify the result

1. Check Devin's exit status and read its report.
2. Inspect `git status --short`, the complete diff, and any new files. Compare them with the pre-delegation working tree and reject unrelated edits.
3. Run the relevant formatting, lint, tests, and build checks independently.
4. Fix or re-delegate concrete failures only within the original scope.
5. Report the actual diff and verification outcome. Commit or push only when the user requested it and repository rules permit it.
