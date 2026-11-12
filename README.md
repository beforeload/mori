[简体中文](README.zh-Hans.md) · [繁體中文](README.zh-Hant.md)

# Mori

A card you wrote can find you again.

Capture notes the way [flomo](https://flomoapp.com) does. Daily Review brings old cards back, Random Walk follows semantic neighbors, and the cognitive map stacks the themes you keep returning to into peaks. Mori is a native macOS app written in Rust and [GPUI](https://gpui.rs). Notes, images, and settings stay on the machine.

It works without an AI account. Related notes, semantic search, Random Walk, and the cognitive map use local TF-IDF (Chinese character bigrams, Latin words). Point it at any OpenAI-compatible service and those notes switch to embedding vectors. Note text leaves the machine only then, and only to the endpoint you set.

The interface language can be switched in Settings → General: 简体中文, English, or 繁體中文. macOS 12 or later, Apple Silicon, [MIT](LICENSE) licensed.

![All notes](docs/screenshots/main.png)

| Random Walk | Cognitive map |
| --- | --- |
| ![Random Walk](docs/screenshots/walk.png) | ![Cognitive map](docs/screenshots/map.png) |

## Features

**Capture**
- The composer takes `#tags` (nested, such as `#reading/psychology`), pasted images, lists, and `**bold**`, `==highlight==`, `__underline__`, `~~strike~~`
- `@` cites another note. An annotation writes a new thought onto an old card. Both create a backlink
- History, edit time, pins, and a trash that can be undone
- Writing under a tag attaches that tag automatically

**Browse**
- Sidebar: note, tag, and day counts; a writing heatmap (click a day); a tag tree you can pin, icon, rename, or delete
- Search (`⌘K`): keywords, date range, included and excluded tags, source, and semantic similarity
- Title menu: sort, source, multi-select (batch tag, export, or delete), export the current view
- The sidebar collapses (`⌘\`). A narrow window turns it into a drawer, and note detail becomes a floating sheet
- Light and dark mode, body font and size, window pin, app lock

**Revisit, with or without AI**
- Daily Review picks a few older notes by rules you set
- Random Walk starts at one note and branches through semantically related cards
- Cognitive map lays notes out by similarity as a 3D contour. Peaks are themes you keep thinking about
- Link graph, related notes, writing stats
- AI insight (read a set of notes from a chosen angle), Ask your notes, an AI memory profile, suggested tags
- Share images: render one note, or the cognitive map, as a PNG card

Related notes, semantic search, Random Walk, and the cognitive map work offline. After an embedding model is configured, notes that already have a vector use it. Random Walk, the cognitive map, the link graph, stats, and the memory profile are still being polished. Settings → Lab opens them too.

**Import, export, and outside writers**
- Import flomo HTML, Mori JSON, Markdown / TXT, CSV (Day One and generic), and WeChat Reading highlight text
- Export JSON (complete, re-importable), Markdown, and HTML, images included
- A local HTTP API and an MCP server so Shortcuts, scripts, and AI tools can read and write notes (below)

## Install

Download the latest `.dmg` from [Releases](https://github.com/beforeload/mori/releases) (Apple Silicon, macOS 12+), and drag Mori to Applications.

The build is not notarized, so macOS blocks the first launch. Choose “Open Anyway” in System Settings → Privacy & Security, or run:

```bash
xattr -dr com.apple.quarantine /Applications/Mori.app
```

## Build from source

You need [Rust](https://rustup.rs) (the version is pinned in `rust-toolchain.toml`; rustup installs it) and the Xcode Command Line Tools.

```bash
git clone https://github.com/beforeload/mori.git
cd mori
cargo run --release
```

Package an `.app`, `.dmg`, and `.zip` into `dist/`:

```bash
./scripts/bundle-macos.sh
```

Tests: `cargo test`.

## Configure AI

In Settings → AI, fill in the endpoint, API key, and model names. Any service that speaks OpenAI `/chat/completions` and `/embeddings` works. Presets cover OpenAI, DeepSeek, Qwen, Kimi, Zhipu, SiliconFlow, and Ollama (local).

## HTTP API and MCP

**HTTP API.** Turn it on in Settings → MCP / API. Mori listens on `127.0.0.1:17321` (the port is editable) and generates an access token.

```bash
# Create a note
curl -X POST http://127.0.0.1:17321/api/v1/memos \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"content": "An idea from Shortcuts #inbox"}'

# Search notes / list tags
curl -H "Authorization: Bearer <token>" "http://127.0.0.1:17321/api/v1/memos?q=product&tag=reading&limit=20"
curl -H "Authorization: Bearer <token>" http://127.0.0.1:17321/api/v1/tags
```

**MCP.** `mori --mcp` runs an [MCP](https://modelcontextprotocol.io) server over stdio. Claude Desktop, Cursor, Cherry Studio, and similar tools can search, read, and write notes. Settings → MCP / API → “Copy MCP config” produces something like:

```json
{
  "mcpServers": {
    "mori": {
      "command": "/Applications/Mori.app/Contents/MacOS/mori",
      "args": ["--mcp"]
    }
  }
}
```

Tools: `search_memos`, `get_memo`, `create_memo`, `update_memo`, `delete_memo`, `list_tags`, `get_related_memos`, `get_daily_review`, `get_memory_profile`, `get_stats`.

Notes written through the API or MCP show up under “API inbox” in the sidebar. The sidebar refresh button reads them immediately.

## Command line

```bash
mori                       # launch the app
mori --mcp                 # run the MCP server
mori --import a.html b.csv # import files and exit (duplicates are skipped)
```

A source build produces the `mori` binary (`cargo run --release` writes `target/release/mori`). `MORI_DATA_DIR` points at a data directory, so you can try the app against a copy and leave real notes alone. The older `MINDFLOW_DATA_DIR` still works.

## Data and privacy

- Notes, images, and settings live in `~/Library/Application Support/Mori` (the SQLite file `mori.db`, plus an image directory). Back them up from Settings → Data & Storage. On first launch, an existing `MindFlow` folder is renamed to `Mori` when `Mori` is not already there, and `mindflow.db` is renamed to `mori.db`.
- No account, and nothing is uploaded. Note text is sent to the endpoint you configured only when you use an AI feature.
- The HTTP API is off by default. When enabled, it binds to `127.0.0.1` only.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| New note | `⌘N` or `⌘/` |
| Search | `⌘K` |
| Show or hide the sidebar | `⌘\` |
| Back to all notes | `⌘⇧F` |
| Bold / highlight / underline | `⌘B` / `⌘⇧H` / `⌘U` |
| Toggle dark mode | `⌘⇧D` |
| Lock | `⌘⇧L` |
| Settings | `⌘,` |

## Layout

```
src/
  main.rs         entry: window, menus, and the --mcp / --import flags
  api_server.rs   local HTTP API
  mcp.rs          MCP server
  core/           UI-free logic: storage, parsing, search, similarity, review, AI client, import/export, share images
  ui/             GPUI interface: sidebar, stream, composer, detail, feature pages, settings
scripts/          packaging and icon generation
```

## Acknowledgements

Interaction and visual design take cues from [flomo](https://flomoapp.com). This project is not affiliated with flomo. The interface is built on [GPUI](https://gpui.rs), [gpui-kit](https://crates.io/crates/gpui-kit), and [gpui-component](https://github.com/longbridge/gpui-component).

## License

[MIT](LICENSE)
