//! `mori <command>`: the note library from a terminal, a script or an AI agent.
//! Commands share the MCP tool implementations and the app's library, so whatever
//! one writes the others see (the running app picks changes up within seconds).
//! Output is plain text, or JSON with `--json`.

use std::io::{IsTerminal as _, Read as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::core::images;
use crate::core::query::{MemoFilter, SortOrder, filter_memos};
use crate::core::store::{Store, database_path};
use crate::core::{io, time};
use crate::mcp;

pub const GUIDE: &str = include_str!("../docs/AI.md");

const COMMANDS: &[&str] = &[
    "help", "guide", "info", "list", "search", "get", "new", "edit", "append", "rm", "tags", "related", "stats",
    "review", "export", "import", "import-obsidian", "mcp", "install-cli",
];

fn usage() -> String {
    format!(
        "Mori {version} — 本地卡片笔记\n\n\
用法：mori <命令> [参数] [--json]\n\n\
读取\n\
  list    [--tag T] [--since 日期] [--until 日期] [--limit N] [--offset N] [--full]   最新笔记\n\
  search  <关键词…> [--semantic] [--tag T] [--since …] [--limit N] [--full]          搜索\n\
  get     <id>                                    一条笔记（含反向链接）\n\
  related <id> [--limit N]                        语义相关的笔记\n\
  tags                                            全部标签和数量\n\
  stats                                           统计\n\
  review                                          今天的每日回顾\n\
  export  [--format md|jsonl|json] [--out 文件] [--tag T] [--since …] [--until …]\n\
          把笔记整批导出给 AI 分析（默认 Markdown 输出到终端，按时间正序）\n\n\
写入（内容可用参数，或写 - / 省略并从标准输入读取）\n\
  new     [内容] [--tag T]                         新建\n\
  edit    <id> [内容]                              改写（旧内容进入历史版本）\n\
  append  <id> [内容]                              在末尾追加\n\
  rm      <id>                                    移入回收站\n\n\
其他\n\
  import  <文件…>                                  导入 flomo / JSON / Markdown / CSV\n\
  import-obsidian <目录>                           导入 Obsidian 仓库\n\
  mcp                                             以 stdio 运行 MCP 服务器\n\
  info                                            版本与数据目录\n\
  guide                                           给 AI 工具看的使用说明\n\
  install-cli [--dir 目录]                         在 ~/.local/bin 放一个 mori 链接\n\n\
数据目录默认是 ~/Library/Application Support/Mori，可用 MORI_DATA_DIR 指向别处。\n\
标签写作 #标签/子标签，引用其他笔记写作 [[memo:ID]]。\n",
        version = env!("CARGO_PKG_VERSION")
    )
}

#[derive(Default)]
struct Args {
    positional: Vec<String>,
    json: bool,
    full: bool,
    semantic: bool,
    tag: Option<String>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<u64>,
    offset: Option<u64>,
    format: Option<String>,
    out: Option<PathBuf>,
    dir: Option<PathBuf>,
}

fn parse_args(raw: &[String]) -> Result<Args> {
    let mut args = Args::default();
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| iter.next().cloned().with_context(|| format!("{name} 需要一个值"));
        match arg.as_str() {
            "--json" => args.json = true,
            "--full" => args.full = true,
            "--semantic" => args.semantic = true,
            "--tag" | "-t" => args.tag = Some(value(arg)?.trim_start_matches('#').to_string()),
            "--since" => args.since = Some(value(arg)?),
            "--until" => args.until = Some(value(arg)?),
            "--limit" | "-n" => args.limit = Some(value(arg)?.parse().context("--limit 需要数字")?),
            "--offset" => args.offset = Some(value(arg)?.parse().context("--offset 需要数字")?),
            "--format" | "-f" => args.format = Some(value(arg)?),
            "--out" | "-o" => args.out = Some(value(arg)?.into()),
            "--dir" => args.dir = Some(value(arg)?.into()),
            _ => args.positional.push(arg.clone()),
        }
    }
    Ok(args)
}

/// Runs a CLI command when the arguments name one; `None` launches the app.
pub fn try_run() -> Option<i32> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = match raw.first().map(String::as_str) {
        Some("--mcp") => ("mcp", &raw[1..]),
        Some("--import") => ("import", &raw[1..]),
        Some("--import-obsidian") => ("import-obsidian", &raw[1..]),
        Some("--help" | "-h") => ("help", &raw[1..]),
        Some("--version" | "-V") => {
            println!("mori {}", env!("CARGO_PKG_VERSION"));
            return Some(0);
        }
        Some(c) if COMMANDS.contains(&c) => (c, &raw[1..]),
        _ => return None,
    };
    match run(command, rest) {
        Ok(()) => Some(0),
        Err(err) => {
            eprintln!("mori {command}: {err:#}");
            Some(1)
        }
    }
}

fn run(command: &str, rest: &[String]) -> Result<()> {
    match command {
        "help" => {
            print!("{}", usage());
            return Ok(());
        }
        "guide" => {
            print!("{GUIDE}");
            return Ok(());
        }
        "mcp" => return mcp::run(),
        "install-cli" => {
            let args = parse_args(rest)?;
            println!("{}", install_cli(args.dir.as_deref())?);
            return Ok(());
        }
        _ => {}
    }
    let args = parse_args(rest)?;
    let mut store = Store::open_default().context("无法打开 Mori 数据库")?;
    match command {
        "info" => {
            let dir = store.data_dir().to_path_buf();
            let stats: Value = serde_json::from_str(&mcp::call_tool(&mut store, "get_stats", &json!({}))?)?;
            let info = json!({
                "version": env!("CARGO_PKG_VERSION"),
                "data_dir": dir,
                "database": database_path(&dir),
                "images_dir": store.images_dir(),
                "memos": stats["memos"],
                "tags": stats["tags"],
            });
            print_value(&info, args.json, |v| {
                format!(
                    "Mori {}\n数据目录  {}\n数据库    {}\n图片目录  {}\n笔记 {} 条 · 标签 {} 个\n",
                    v["version"].as_str().unwrap_or(""),
                    v["data_dir"].as_str().unwrap_or(""),
                    v["database"].as_str().unwrap_or(""),
                    v["images_dir"].as_str().unwrap_or(""),
                    v["memos"],
                    v["tags"],
                )
            });
        }
        "list" | "search" => {
            let query = args.positional.join(" ");
            if command == "search" && query.trim().is_empty() {
                bail!("用法：mori search <关键词…>");
            }
            let value = tool(&mut store, "search_memos", json!({
                "query": query,
                "tag": args.tag,
                "since": args.since,
                "until": args.until,
                "semantic": args.semantic,
                "limit": args.limit.unwrap_or(20),
                "offset": args.offset.unwrap_or(0),
            }))?;
            let full = args.full;
            print_value(&value, args.json, |v| memos_text(&v["memos"], full));
        }
        "get" => {
            let id = one(&args, "mori get <id>")?;
            let value = tool(&mut store, "get_memo", json!({"id": id}))?;
            print_value(&value, args.json, |v| memo_text(v, true));
        }
        "related" => {
            let id = one(&args, "mori related <id>")?;
            let value = tool(&mut store, "get_related_memos", json!({"id": id, "limit": args.limit.unwrap_or(8)}))?;
            print_value(&value, args.json, |v| memos_text(v, false));
        }
        "tags" => {
            let value = tool(&mut store, "list_tags", json!({}))?;
            print_value(&value, args.json, |v| {
                v.as_array()
                    .into_iter()
                    .flatten()
                    .map(|t| format!("#{}  {}\n", t["tag"].as_str().unwrap_or(""), t["count"]))
                    .collect()
            });
        }
        "stats" => {
            let value = tool(&mut store, "get_stats", json!({}))?;
            print_value(&value, args.json, |v| {
                format!("笔记 {} · 标签 {} · 记录 {} 天 · {} 字 · 连续 {} 天\n", v["memos"], v["tags"], v["days"], v["chars"], v["streak_days"])
            });
        }
        "review" => {
            let value = tool(&mut store, "get_daily_review", json!({}))?;
            print_value(&value, args.json, |v| memos_text(v, true));
        }
        "new" => {
            let mut content = content_arg(&args.positional, 0)?;
            if let Some(tag) = &args.tag
                && !content.split_whitespace().any(|w| w == format!("#{tag}"))
            {
                content = format!("#{tag}\n{content}");
            }
            let value = tool(&mut store, "create_memo", json!({"content": content, "source": "cli"}))?;
            print_value(&value, args.json, |v| format!("已创建 {}\n", v["created"]["id"].as_str().unwrap_or("")));
        }
        "edit" => {
            let id = args.positional.first().context("用法：mori edit <id> [内容]")?.clone();
            let content = content_arg(&args.positional, 1)?;
            let value = tool(&mut store, "update_memo", json!({"id": id, "content": content}))?;
            print_value(&value, args.json, |_| format!("已更新 {id}\n"));
        }
        "append" => {
            let id = args.positional.first().context("用法：mori append <id> [内容]")?.clone();
            let content = content_arg(&args.positional, 1)?;
            let value = tool(&mut store, "append_memo", json!({"id": id, "content": content}))?;
            print_value(&value, args.json, |_| format!("已追加到 {id}\n"));
        }
        "rm" => {
            let id = one(&args, "mori rm <id>")?;
            let value = tool(&mut store, "delete_memo", json!({"id": id}))?;
            print_value(&value, args.json, |_| format!("已移入回收站 {id}（可在 Mori 中恢复）\n"));
        }
        "export" => export(&store, &args)?,
        "import" => {
            anyhow::ensure!(!args.positional.is_empty(), "用法：mori import <文件…>");
            let paths: Vec<PathBuf> = args.positional.iter().map(Into::into).collect();
            println!("{}", io::import_files(&mut store, &paths, true)?.summary());
        }
        "import-obsidian" => {
            let dir = one(&args, "mori import-obsidian <仓库目录>")?;
            println!("{}", io::import_obsidian(&mut store, Path::new(&dir), true)?.summary());
        }
        other => bail!("未知命令：{other}"),
    }
    Ok(())
}

fn tool(store: &mut Store, name: &str, args: Value) -> Result<Value> {
    Ok(serde_json::from_str(&mcp::call_tool(store, name, &args)?)?)
}

fn one(args: &Args, usage: &str) -> Result<String> {
    args.positional.first().cloned().with_context(|| format!("用法：{usage}"))
}

/// Text from the argument at `index`, or from standard input when it is `-` or missing.
fn content_arg(positional: &[String], index: usize) -> Result<String> {
    let rest = positional.get(index..).unwrap_or_default();
    if !rest.is_empty() && rest != ["-"] {
        return Ok(rest.join(" "));
    }
    let stdin = std::io::stdin();
    anyhow::ensure!(!stdin.is_terminal() || rest == ["-"], "没有内容：把内容写在命令后，或通过管道传入");
    let mut text = String::new();
    stdin.lock().read_to_string(&mut text)?;
    anyhow::ensure!(!text.trim().is_empty(), "内容为空");
    Ok(text.trim_end().to_string())
}

fn print_value(value: &Value, json: bool, text: impl FnOnce(&Value) -> String) {
    if json {
        println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
    } else {
        print!("{}", text(value));
    }
}

fn memo_text(m: &Value, full: bool) -> String {
    let tags: Vec<String> = m["tags"].as_array().into_iter().flatten().filter_map(|t| t.as_str()).map(|t| format!("#{t}")).collect();
    let content = m["content"].as_str().unwrap_or("");
    let body = if full {
        content.trim().to_string()
    } else {
        crate::core::parse::snippet(content, 120)
    };
    let mut out = format!("{}  {}  {}\n{}\n", m["id"].as_str().unwrap_or(""), m["created_at"].as_str().unwrap_or(""), tags.join(" "), body);
    if let Some(back) = m["backlinks"].as_array().filter(|b| !b.is_empty()) {
        out.push_str(&format!("被引用：{}\n", back.iter().filter_map(|b| b.as_str()).collect::<Vec<_>>().join(", ")));
    }
    out.push('\n');
    out
}

fn memos_text(list: &Value, full: bool) -> String {
    let memos = list.as_array().cloned().unwrap_or_default();
    if memos.is_empty() {
        return "没有找到笔记\n".into();
    }
    memos.iter().map(|m| memo_text(m, full)).collect()
}

fn export(store: &Store, args: &Args) -> Result<()> {
    let filter = MemoFilter {
        tag: args.tag.clone(),
        date_from: args.since.as_deref().and_then(time::parse_date),
        date_to: args.until.as_deref().and_then(time::parse_date),
        keywords: args.positional.join(" "),
        ..Default::default()
    };
    let memos = filter_memos(store, &filter, SortOrder::CreatedAsc);
    let format = args.format.as_deref().unwrap_or("md");
    let mut out = String::new();
    match format {
        "md" | "markdown" => {
            out.push_str(&format!("# Mori 笔记导出（{} 条）\n\n", memos.len()));
            for memo in &memos {
                let body = images::map_library(&memo.body(), |name| format!("<{}>", store.image_path(name).display()));
                out.push_str(&format!("## {} · {}\n\n{}\n\n", time::fmt_datetime(memo.created_at), memo.id, body.trim()));
            }
        }
        "jsonl" | "json" => {
            let rows: Vec<Value> = memos.iter().map(|m| mcp::memo_value(store, m)).collect();
            if format == "jsonl" {
                for row in rows {
                    out.push_str(&serde_json::to_string(&row)?);
                    out.push('\n');
                }
            } else {
                out = serde_json::to_string_pretty(&rows)?;
                out.push('\n');
            }
        }
        other => bail!("不支持的格式：{other}（可选 md、jsonl、json）"),
    }
    match &args.out {
        Some(path) => {
            std::fs::write(path, out)?;
            eprintln!("已导出 {} 条笔记到 {}", memos.len(), path.display());
        }
        None => print!("{out}"),
    }
    Ok(())
}

/// Links `mori` into a directory on the user's `PATH` (default `~/.local/bin`).
pub fn install_cli(dir: Option<&Path>) -> Result<String> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let dir = match dir {
        Some(dir) => dir.to_path_buf(),
        None => dirs::home_dir().context("找不到主目录")?.join(".local/bin"),
    };
    std::fs::create_dir_all(&dir)?;
    let link = dir.join("mori");
    if let Ok(meta) = std::fs::symlink_metadata(&link) {
        anyhow::ensure!(meta.file_type().is_symlink(), "{} 已存在且不是链接，未改动", link.display());
        std::fs::remove_file(&link)?;
    }
    std::os::unix::fs::symlink(&exe, &link)?;
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == dir))
        .unwrap_or(false);
    let mut message = format!("已创建 {} → {}", link.display(), exe.display());
    if !on_path {
        message.push_str(&format!("\n{} 不在 PATH 中，可在 ~/.zshrc 加一行：export PATH=\"{}:$PATH\"", dir.display(), dir.display()));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flags_and_positionals() {
        let raw: Vec<String> = ["读书", "方法", "--tag", "#学习", "--limit", "5", "--json"].iter().map(|s| s.to_string()).collect();
        let args = parse_args(&raw).unwrap();
        assert_eq!(args.positional, vec!["读书", "方法"]);
        assert_eq!(args.tag.as_deref(), Some("学习"));
        assert_eq!(args.limit, Some(5));
        assert!(args.json);
        assert!(parse_args(&["--limit".to_string()]).is_err());
    }

    #[test]
    fn guide_and_usage_cover_every_command() {
        let help = usage();
        for command in COMMANDS.iter().filter(|c| !matches!(**c, "help")) {
            assert!(help.contains(command), "help mentions {command}");
            assert!(GUIDE.contains(command), "guide mentions {command}");
        }
    }
}
