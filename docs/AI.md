# Mori for AI tools

Mori is a local card-note library (flomo style) on macOS. Notes live in a SQLite
database in the user's data folder; nothing is uploaded. An AI tool can work with
them in three ways, all over the same library:

| Way | When to use |
| --- | --- |
| `mori` command line | Agents with a shell (Claude Code, Codex, Cursor, scripts). Works whether the app is open or not. |
| MCP server (`mori mcp`) | Chat apps that speak MCP (Claude Desktop, Cursor, Cherry Studio). |
| HTTP API | Shortcuts and scripts while the app runs with Settings → MCP / API → enabled. |

Changes made through any of them show up in the running app within a few seconds.

## Notes

- One note is a short Markdown text. Tags are written inline: `#tag`, nested `#work/project`.
  A tag needs a space (or line start) before `#`; `#2`, `#000` and anything inside code are not tags.
- `[[memo:ID]]` cites another note; `[[Title]]` links a note by title, alias or file name.
- A note may start with YAML frontmatter (`---` … `---`). Its `tags` count as tags; `title`
  and `aliases` name the note for `[[links]]`.
- Images are part of the text: `![alt](images/<file>)`. JSON output also lists their
  absolute paths under `images`, so a multimodal model can open them.
- Every edit keeps the previous text in the note's history; delete moves a note to the trash.
  Both can be undone in the app. Ask the user before editing or deleting their notes.

## Command line

The binary is `/Applications/Mori.app/Contents/MacOS/mori`. `mori install-cli` (or Settings →
MCP / API → 安装 mori 命令) links it as `~/.local/bin/mori`. `MORI_DATA_DIR=<dir>` points
any command at another library, which is useful for trying things on a copy.

Add `--json` to any reading command for machine-readable output.

```bash
mori info                                   # version, data folder, counts
mori list --limit 20 [--offset 20]          # newest notes; --tag, --since, --until filter
mori search 读书 方法 --json                 # every keyword must match
mori search "how I plan my week" --semantic # ranked by meaning
mori get <id> --json                        # full text, tags, backlinks, image paths
mori related <id> --limit 8                 # notes similar in meaning
mori tags --json                            # every tag with its note count
mori stats                                  # notes, tags, days, characters
mori review                                 # today's daily-review picks

mori new "An idea #inbox"                   # or pipe text: echo "…" | mori new --tag inbox
mori edit <id> "Rewritten text"             # or: mori edit <id> - < file.md
mori append <id> "One more line"
mori rm <id>                                # to the trash

mori export --format jsonl --since 2026-01-01 > notes.jsonl   # bulk, oldest first
mori export --format md --tag 学习 --out study.md
mori import notes.html export.csv           # flomo HTML, Mori JSON, Markdown, CSV
mori import-obsidian ~/vault                # one note per Markdown file
mori mcp                                    # the MCP server on stdio
mori guide                                  # this text
```

Dates are `YYYY-MM-DD`. Exit status is non-zero on error, with the reason on stderr.

`--json` shapes: `list` and `search` return `{"count", "total", "offset", "memos": [...]}`;
`get` returns one memo; `tags` returns `[{"tag", "count"}]`. A memo is:

```json
{"id": "a1b2c3d4e5f6", "content": "…", "created_at": "2026-11-25 10:30:00",
 "updated_at": "…", "tags": ["学习/AI"], "pinned": false, "source": "app",
 "refs": ["…"], "images": ["/Users/…/Mori/images/x.png"]}
```

`export --format jsonl` writes one such memo per line.

## MCP

```json
{"mcpServers": {"mori": {"command": "/Applications/Mori.app/Contents/MacOS/mori", "args": ["--mcp"]}}}
```

Tools: `search_memos` (query, tag, since, until, semantic, limit, offset; an empty query lists
the newest notes), `get_memo`, `create_memo`, `update_memo`, `append_memo`, `delete_memo`,
`list_tags`, `get_related_memos`, `get_daily_review`, `get_memory_profile`, `get_stats`.

## Recipes

- **Weekly summary:** `mori export --format md --since <monday>` and summarise the output.
- **Theme analysis:** `mori export --format jsonl`, group by `tags`, read the largest groups.
- **Find before writing:** `mori search <words> --json` first, then `mori append <id>` to an
  existing note instead of creating a near-duplicate.
- **Tag hygiene:** `mori tags --json` shows rarely used or near-duplicate tags; propose
  merges to the user, then rewrite notes with `mori edit`.
- **Save an answer:** `mori new --tag <topic> -` with the text on stdin.
