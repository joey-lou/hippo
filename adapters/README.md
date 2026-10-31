# Adapter contract

An adapter maps one coding agent onto the `hippo` command. The markdown files, the index, and the data repo stay the same for every agent. A new agent is a new directory under `adapters/`.

Each directory includes `adapter.json`. It names a root (`cursor` or `pi`), the files to link, and any hook entries. `hippo adapter install` refuses a directory without one. Links and hook commands are recorded in `~/.config/hippo/adapters/manifest.json`, one object per adapter. The next install of that adapter removes the recorded links and hook entries, then writes the new list. Other adapters in the file stay put. A hook command must contain `hippo-`.

Libraries can also call `MemoryStore` directly. Agents should stick to the CLI below.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `2` | Validation error or memory not found |
| other | Unexpected error |

## Commands & JSON shapes

All JSON is emitted on stdout with `--json` (or `--format json`).

### `hippo query "<text>" [--k N] [--json]`
```json
[
  {"id": "...", "path": "...", "title": "...", "score": -1.23,
   "snippet": "...", "scope": "global"}
]
```
Lower `score` = better. With the default `hash` encoder it is BM25 minus boosts for the active scope, update
recency, and recall frequency, minus `1.0 * max(cosine, 0)` from a cached hashing
embedding (word tokens and character trigrams — not a neural model). That second
signal lifts memories that share subword shape when the FTS tokenizer misses
(`fulltext` vs `full-text`). It does not understand synonyms. A neighbor with no
BM25 hit starts from a miss penalty of 8.0 before those boosts. Treat `score` as
an opaque ordering signal, not a raw BM25 value. Recall unions `global` + active scope.

`embed` in `~/.config/hippo/config.toml` (or `--embed` / `HIPPO_EMBED`) can be `hash`, `off`, `bge-small`, `bge-base`, or `bge-m3`. `off` drops the vector term. A `bge-*` encoder ranks by cosine of that model's vectors and keeps a strong keyword hit (raw BM25 below -8) ahead of a vector-only hit. Those vectors live in the index, keyed by encoder name, and are rebuilt with `hippo reindex --all`. The markdown files stay put. `hippo status` says when the active encoder is missing vectors.

### `hippo add [--title --keywords --tags --category --scope --source --body --reason --confidence --force] [--json]`
Body is read from stdin when `--body` is omitted. Or pass `--from-json` to read
a full JSON record from stdin. Emits `{"id": "...", "path": "..."}`. When the new memory
overlaps another, `notices` is added (omitted when empty). When lexical capture
checks fire, `warnings` is added (omitted when empty):

```json
{"id": "...", "path": "...",
 "warnings": ["body is shorter than 40 characters"],
 "notices": [
  {"kind": "duplicate", "other_id": "...", "detail": "near-duplicate (score 0.72)"}
]}
```

`kind` is `duplicate` or `contradiction`. Text mode prints notices and
`warning: ...` lines on stderr.

`--reason` is an optional string stored in frontmatter (why the memory was saved).
`--confidence`, when present, must be `high`, `medium`, or `low`. Omitted
confidence is allowed. The write is rejected (exit 2, no file) when confidence
is `low`, or when the lexical assessment is `low` (secret-like text), unless
`--force` is set. `--force` is the manual override. Invalid confidence is also
exit 2 and writes nothing.

### `hippo update <id> [--title --keywords --tags --body --scope --reason --confidence --force] [--json]`
Emits the same object as `add`, including `notices` and `warnings` when present.
The capture gate runs on the memory after the patch is applied and before the
file is written. Omitted body, title, and keywords are read from the current
file. A low `--confidence` or a low assessment is rejected unless `--force`.
Updating other fields does not require `--force`.

### `hippo consolidate [--apply] [--json]`
Reports near-duplicate clusters and contradictions. Does not merge unless
`--apply` is set, and `--apply` merges duplicates only — contradictions stay for
a person to resolve. Reindex never merges, because session start runs it.

```json
{"duplicates": [{"keeper": "...", "members": ["...", "..."], "score": 0.72}],
 "contradictions": [{"a": "...", "b": "...", "tokens": ["pip", "uv"]}],
 "applied": false, "removed": []}
```

The keeper is the most-recalled member, else the most recently updated, else the
lowest id. Absorbed files are deleted and their body is appended to the keeper.

### Sync after writes

When the data home is a git repo, `add`, `update`, and `consolidate --apply`
commit the memory folder as "Record memory updates." and push it when the branch
has an upstream. Writers hold `.index/write.lock`, so parallel agents queue. A
failed commit or push does not undo the write: the command still exits `0` and
adds `memory saved but not synced: <git error>` to `warnings`.

`--source` defaults to `HIPPO_SOURCE`, then `pi` when `PI_CODING_AGENT` is set,
then `cursor` when `CURSOR_AGENT` is set. Pi comes first because a Cursor model
inside Pi sets both.

### `hippo sync [--file PATH]`
Reindexes changed files, then commits and pushes the same way. Emits
`committed=BOOL pushed=BOOL`. With `--file`, a path outside `<home>/memory/` or
not ending in `.md` prints `skipped: ...` and changes nothing. Adapters call this
after an editor changes a memory file without the CLI. A git failure exits `1`.

### `hippo reindex [--all|--changed]`
Emits `indexed=N added=N updated=N deleted=N`.

### `hippo manifest [--format md|json]`
Full catalog (every memory's metadata). Not injected at session start; use for
tooling/debugging.
```json
[{"id": "...", "title": "...", "keywords": ["..."], "path": "..."}]
```

### `hippo digest [--format md|json]`
Constant-size session primer: memory count + top topic tags, no bodies and no
per-memory rows. Injected at session start so cost stays flat as the store grows;
the agent recalls on demand via `hippo query`.
```json
{"total": 6, "topics": [{"tag": "hippo", "count": 2}]}
```

### `hippo status [--json]`
```json
{"indexed": 4, "new": [], "changed": [], "removed": [],
 "git": {"is_repo": true, "dirty": false, "ahead": 0},
 "actions": []}
```

## Scope resolution

Active scope: `--scope` → `HIPPO_SCOPE` → `.hippo/scope` marker → `global`.

## Data home resolution

`--home` → `HIPPO_HOME` → `~/.config/hippo/config.toml` (`home = "..."`). Hippo creates that directory when it is missing. A path that already exists and is not a directory is an error.
