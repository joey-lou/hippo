# Hippo 🦛

[![Crates.io](https://img.shields.io/crates/v/hippo-cli?logo=rust)](https://crates.io/crates/hippo-cli)
[![License: MIT](https://img.shields.io/crates/l/hippo-cli)](LICENSE)

Hippo, a CLI to build your hippo campus! 🦛🦛🦛

There are many agent memory setups, but this one is yours to keep and see, in plain markdown.

You can read, modify and even git track your memory files easily in one place. Agents handle the hard work of adding new entries, synthesizing notes, and keeping everything fresh.

The CLI is built in rust, install with cargo and add the adapter for the harness you use, then you are good to go.

## Quick start

```bash
cargo install hippo-cli
hippo adapter install cursor
```

Restart Cursor after that. For Pi, run `hippo adapter install pi` and start a new session.

After you upgrade Hippo, run the install command again so the plugin matches the new binary.

Memories go in a folder you choose. Set it in `~/.config/hippo/config.toml`:

```toml
home = "~/memories"
# embed = "hash"   # hash (default), off, bge-small, bge-base, bge-m3
```

`embed` picks the vector used at search time. `hash` is the built-in hashing embedding. `off` is keyword search only. The `bge-*` options download a model on first use into `~/.cache/hippo/models` and store one vector per note. `hippo reindex --all` builds those vectors. `hippo encoder list` prints the choices, and `hippo encoder bench` times load and a single embedding. `--embed` and `HIPPO_EMBED` override the config for one command.

`HIPPO_HOME` points at the same folder. Hippo creates it if it is missing. Each memory is a markdown file under `<home>/memory/`.

When that folder is a git repo, every write commits the memory folder and pushes it if the branch has an upstream. A failed push leaves the memory saved and prints a warning.

## Commands

Agents use these. JSON shapes and the rest of the contract are in [adapters/README.md](adapters/README.md).

| Command                                                                           | Purpose                                                               |
| --------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| `hippo query "<text>" [--k N] [--json]`                                           | Ranked matches. A lower score is a better match.                      |
| `hippo add [--title --keywords --tags --category --reason --confidence] [--json]` | Create a memory. The body is read from stdin.                         |
| `hippo update <id> [...] [--json]`                                                | Edit a memory.                                                        |
| `hippo show <id>` / `hippo path <id>`                                             | Print the file or its path.                                           |
| `hippo reindex [--all\|--changed]`                                                | Rebuild the index from markdown.                                      |
| `hippo status [--json]`                                                           | Index drift, git state, and what to do next.                          |
| `hippo sync [--file PATH]`                                                        | Reindex, then commit and push the memory folder.                      |
| `hippo digest [--format md\|json]`                                                | Short summary: how many memories, and the top topics.                 |
| `hippo consolidate [--apply] [--json]`                                            | Near-duplicates and contradictions. `--apply` merges duplicates only. |
| `hippo encoder list` / `hippo encoder bench`                                      | List encoders, or time model load and one embedding.                  |

Exit `0` on success. Exit `2` when the input is invalid or the memory does not exist.

`--home` sets the folder for one command. Otherwise Hippo uses `HIPPO_HOME`, then `home` in `~/.config/hippo/config.toml`.

From a checkout of this repo, `cargo install --path .` installs that build. `adapters/cursor/install.sh` and `adapters/pi/install.sh` link the working tree instead of the copy shipped in the binary.

## Releasing

`Cargo.toml` on `main` stays at `0.0.0-dev`. Push a tag on the latest `main` commit and [the release workflow](.github/workflows/release.yml) publishes that version and attaches binaries plus `hippo-adapters.tar.gz`.

```bash
git tag v0.0.7 && git push origin v0.0.7
```
