# Pi adapter

Pi loads this adapter as a skill and a session extension. Both call the `hippo`
command. The model backend does not matter: OpenRouter, or any other provider Pi
is logged into, uses the same extension. JSON shapes and exit codes are in
[`adapters/README.md`](../README.md).

```bash
hippo adapter install pi
```

From a checkout, `install.sh` links this working tree instead. Either way you get:

- `~/.pi/agent/skills/use-memory`: when to recall and save. It is the same file as the Cursor adapter's skill.
- `~/.pi/agent/extensions/hippo.ts`:
  - At session start it runs `hippo reindex --changed` and injects `hippo digest`.
  - After Pi's `edit` or `write` tool changes a file, it runs `hippo sync --file <path>`.
  - Before Pi settles, it runs `hippo status --json` and, once per prompt, asks the model to report unsynced work.

`hippo add` and `hippo update` commit and push on their own, so a Pi `bash` call needs no hook.

Log the model in separately, for example `/login openrouter` or `OPENROUTER_API_KEY`. This adapter does not configure providers.
