#!/usr/bin/env bash
# Hippo sessionStart hook: refresh the index, then inject a constant-size memory
# digest (count + top topics) plus usage guidance via additional_context. The
# full catalog is NOT injected — recall is on demand via `hippo query`, so cost
# stays flat as the store grows. Fails open — never breaks a Cursor session.

cat >/dev/null 2>&1  # stdin unused

hippo_bin="$(command -v hippo 2>/dev/null || true)"
if [ -z "$hippo_bin" ] && [ -x "$HOME/.local/bin/hippo" ]; then
  hippo_bin="$HOME/.local/bin/hippo"
fi
if [ -z "$hippo_bin" ]; then
  echo '{}'
  exit 0
fi

"$hippo_bin" reindex --changed >/dev/null 2>&1 || true
export HIPPO_DIGEST="$("$hippo_bin" digest 2>/dev/null || true)"
export HIPPO_STATUS="$("$hippo_bin" status 2>/dev/null || true)"

python3 <<'PY'
import json, os

digest = os.environ.get("HIPPO_DIGEST", "").strip() or "_(memory unavailable)_"
status = os.environ.get("HIPPO_STATUS", "").strip()

guide = """# Hippo — your personal memory

You have a durable personal memory store ("second brain"). Nothing is preloaded —
retrieve on demand:

- **Recall:** when the user references past context, preferences, or starts a task
  that may overlap the topics below, run `hippo query "<text>" --json`, then read
  the top hit files before answering. `query` is the only way to see memory bodies.
- **Save:** when a durable preference, decision, lesson, or correction emerges,
  first `hippo query` to dedupe, then `hippo add --title "..." --keywords a,b
  --category <cat>` (body on stdin) or `hippo update <id>`. Do this quietly.
- If the user says "remember this" / "note that", always save it (manual override).
- If `add` or `update` reports a near-duplicate, fold it into the existing memory.
  If it reports a contradiction, resolve the two memories before moving on.
  `hippo consolidate` lists both. `--apply` merges duplicates only.

## What's in memory
"""

ctx = guide + "\n" + digest
if status:
    ctx += "\n\n<!-- hippo status: " + " ".join(status.split()) + " -->"
print(json.dumps({"additional_context": ctx}))
PY
