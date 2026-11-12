[简体中文](README.zh-Hans.md) · [English](README.md)

# Mori

記下來的卡片，過一陣還會再遇見。

像 [flomo](https://flomoapp.com) 一樣隨手記錄。每日回顧把舊筆記重新翻出來，隨機漫步沿著語義往下走，認知地圖把反覆思考的主題堆成山。原生 macOS 應用，用 Rust 和 [GPUI](https://gpui.rs) 編寫，筆記、圖片和設定都在本機。

不配置 AI 也能用：相關筆記、語義搜尋、隨機漫步和認知地圖使用本地 TF-IDF（中文按字二元組、英文按詞）。配置相容 OpenAI 介面的服務後，改為模型向量；只有這時，相關內容才會發到你填寫的地址。

介面可以在「設定 → 通用」切換简体中文、English、繁體中文。macOS 12 及以上，Apple Silicon，[MIT](LICENSE) 許可。

![全部筆記](docs/screenshots/main.png)

| 隨機漫步 | 認知地圖 |
| --- | --- |
| ![隨機漫步](docs/screenshots/walk.png) | ![認知地圖](docs/screenshots/map.png) |

## 功能

**記錄**
- 輸入框支援 `#標籤`（可多級，如 `#讀書/心理學`）、圖片（可直接貼上）、列表，以及 `**加粗**`、`==高亮==`、`__下劃線__`、`~~刪除線~~`
- `@` 引用其他筆記，「批註」給舊筆記寫新想法，兩者都會建立雙向連結
- 歷史版本、修改時間、置頂、垃圾桶（可撤銷）
- 在某個標籤下記錄時自動帶上該標籤

**整理與瀏覽**
- 側邊欄：筆記數 / 標籤數 / 天數統計、記錄熱力圖（點選檢視某天）、標籤樹（置頂、圖示、重新命名、刪除）
- 搜尋（`⌘K`）支援關鍵詞、日期範圍、包含 / 排除標籤、來源篩選，以及基於語義的相似搜尋
- 標題選單：排序方式、筆記來源、多選筆記（批次加標籤 / 匯出 / 刪除）、匯出當前檢視
- 側邊欄可收起（`⌘\`）；視窗變窄時自動收起為抽屜，筆記詳情改為浮層
- 淺色 / 深色模式、正文字型與字號、視窗置頂、應用鎖

**回顧與 AI**
- 每日回顧：按規則從舊筆記中挑出幾條重新看見
- 隨機漫步：從一條筆記出發，沿語義相關的筆記分岔前進
- 認知地圖：按相似度把筆記排成 3D 等高線地形，山峰就是你反覆思考的主題
- 筆記關係圖、相關筆記、記錄統計
- AI 洞察（按不同視角解讀一組筆記）、問問筆記（基於你的筆記回答問題）、AI 記憶檔案、推薦標籤
- 分享圖片：把單條筆記或認知地圖生成 PNG 卡片

相關筆記、語義搜尋、隨機漫步和認知地圖在沒有配置 AI 時離線工作；配置了嵌入模型後，已有向量的筆記改用模型向量。隨機漫步、認知地圖、筆記關係圖、記錄統計和記憶檔案仍在打磨，設定裡的「實驗室」也可以開啟它們。

**匯入匯出與外部寫入**
- 匯入：flomo 匯出的 HTML、Mori JSON、Markdown / TXT、CSV（Day One 及通用格式）、微信讀書筆記文字
- 匯出：JSON（完整，可再匯入）、Markdown、HTML，圖片一併複製
- 本地 HTTP API 和 MCP 伺服器，讓快捷指令、腳本和 AI 工具讀寫筆記（見下文）

## 安裝

從 [Releases](https://github.com/beforeload/mori/releases) 下載最新的 `.dmg`（Apple Silicon，macOS 12 及以上），把 Mori 拖進「應用程式」。

安裝包未經 Apple 公證，第一次開啟會被系統攔截。可以在「系統設定 → 隱私權與安全性」裡點「仍要打開」，或在終端執行：

```bash
xattr -dr com.apple.quarantine /Applications/Mori.app
```

## 從原始碼構建

需要 [Rust](https://rustup.rs)（版本由 `rust-toolchain.toml` 固定，rustup 會自動安裝）和 Xcode Command Line Tools。

```bash
git clone https://github.com/beforeload/mori.git
cd mori
cargo run --release
```

打包成 `.app`、`.dmg` 和 `.zip`（輸出到 `dist/`）：

```bash
./scripts/bundle-macos.sh
```

執行測試：`cargo test`。

## 配置 AI

在「設定 → AI 服務」中填寫服務地址、API Key 和模型名。任何相容 OpenAI `/chat/completions` 與 `/embeddings` 介面的服務都可以，設定頁內建了 OpenAI、DeepSeek、通義千問、Kimi、智譜、硅基流動和 Ollama（本地）的預設。

## 外部寫入：HTTP API 與 MCP

**HTTP API**：在「設定 → MCP / API」中啟用後，Mori 會在 `127.0.0.1:17321`（埠可改）監聽，並自動生成訪問令牌。

```bash
# 新建筆記
curl -X POST http://127.0.0.1:17321/api/v1/memos \
  -H "Authorization: Bearer <令牌>" \
  -H "Content-Type: application/json" \
  -d '{"content": "來自快捷指令的靈感 #收集箱"}'

# 搜尋筆記 / 列出標籤
curl -H "Authorization: Bearer <令牌>" "http://127.0.0.1:17321/api/v1/memos?q=產品&tag=讀書&limit=20"
curl -H "Authorization: Bearer <令牌>" http://127.0.0.1:17321/api/v1/tags
```

**MCP**：`mori --mcp` 以 stdio 方式執行 [MCP](https://modelcontextprotocol.io) 伺服器，Claude Desktop、Cursor、Cherry Studio 等工具可以直接搜尋、閱讀和記錄筆記。在「設定 → MCP / API」點「複製 MCP 配置」即可得到類似下面的配置：

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

提供的工具：`search_memos`、`get_memo`、`create_memo`、`update_memo`、`delete_memo`、`list_tags`、`get_related_memos`、`get_daily_review`、`get_memory_profile`、`get_stats`。

透過 API 和 MCP 寫入的筆記會出現在側邊欄的「API 輸入」中，點側邊欄的重新整理按鈕可立即讀取。

## 命令列

```bash
mori                       # 啟動應用
mori --mcp                 # 執行 MCP 伺服器
mori --import a.html b.csv # 匯入檔案後退出（自動跳過重複筆記）
```

從原始碼執行時，可執行檔名是 `mori`（`cargo run --release` 會生成 `target/release/mori`）。環境變數 `MORI_DATA_DIR` 可以指定資料目錄，方便用一份測試資料試用而不影響真實筆記。舊的 `MINDFLOW_DATA_DIR` 仍然有效。

## 資料與隱私

- 筆記、圖片和設定儲存在 `~/Library/Application Support/Mori`（SQLite 資料庫 `mori.db` 加圖片目錄），可在「設定 → 資料與儲存」中備份。如果本機還留著舊的 `MindFlow` 目錄且還沒有 `Mori` 目錄，第一次啟動會把舊目錄改名為 `Mori`，並把 `mindflow.db` 改名為 `mori.db`。
- 不需要帳號，不上傳任何資料。只有在你配置了 AI 服務並使用 AI 功能時，相關筆記內容才會傳送給你填寫的服務地址。
- HTTP API 預設關閉，開啟後只監聽本機地址 `127.0.0.1`。

## 快捷鍵

| 操作 | 快捷鍵 |
| --- | --- |
| 新建筆記 | `⌘N` 或 `⌘/` |
| 搜尋 | `⌘K` |
| 顯示 / 隱藏側邊欄 | `⌘\` |
| 回到全部筆記 | `⌘⇧F` |
| 加粗 / 高亮 / 下劃線 | `⌘B` / `⌘⇧H` / `⌘U` |
| 切換深色模式 | `⌘⇧D` |
| 鎖定 | `⌘⇧L` |
| 設定 | `⌘,` |

## 專案結構

```
src/
  main.rs         入口：視窗、選單，以及 --mcp / --import 命令列
  api_server.rs   本地 HTTP API
  mcp.rs          MCP 伺服器
  core/           與介面無關的邏輯：儲存、解析、搜尋、語義相似度、回顧、AI 客戶端、匯入匯出、分享圖
  ui/             GPUI 介面：側邊欄、筆記流、輸入框、詳情、各功能頁面與設定
scripts/          打包腳本與圖示生成
```

## 致謝

互動和設計參考了 [flomo 浮墨筆記](https://flomoapp.com)，本專案與 flomo 官方無關。介面基於 [GPUI](https://gpui.rs)、[gpui-kit](https://crates.io/crates/gpui-kit) 和 [gpui-component](https://github.com/longbridge/gpui-component)。

## 許可證

[MIT](LICENSE)
