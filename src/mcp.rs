//! Model Context Protocol server over stdio (`mori --mcp`), so AI tools such as
//! Claude Desktop, Cursor or Cherry Studio can search, read and write memos.

use std::io::{BufRead, Write};

use anyhow::Result;
use serde_json::{Value, json};

use crate::core::query::{MemoFilter, SortOrder, filter_memos};
use crate::core::semantic::SemanticIndex;
use crate::core::store::{NewMemo, Store};
use crate::core::{review, stats, tags, time};

const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn run() -> Result<()> {
    let mut store = Store::open_default()?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            write_message(&mut stdout, &json!({
                "jsonrpc": "2.0", "id": Value::Null,
                "error": {"code": -32700, "message": "Parse error"}
            }))?;
            continue;
        };
        let id = request.get("id").cloned();
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));
        let Some(id) = id else {
            continue;
        };
        let _ = store.sync_external_changes();
        let response = match handle(&mut store, method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(err) => json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32603, "message": err.to_string()}
            }),
        };
        write_message(&mut stdout, &response)?;
    }
    Ok(())
}

fn write_message(out: &mut impl Write, value: &Value) -> Result<()> {
    writeln!(out, "{}", serde_json::to_string(value)?)?;
    out.flush()?;
    Ok(())
}

fn handle(store: &mut Store, method: &str, params: &Value) -> Result<Value> {
    match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or(PROTOCOL_VERSION);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mori", "version": env!("CARGO_PKG_VERSION")},
                "instructions": t!("Mori 是用户的卡片笔记库。可以搜索、阅读、创建和修改笔记，读取记忆档案。修改或删除前请向用户确认。")
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions()})),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(store, name, &args) {
                Ok(text) => Ok(json!({"content": [{"type": "text", "text": text}]})),
                Err(err) => Ok(json!({
                    "content": [{"type": "text", "text": t!("错误：{}", err)}],
                    "isError": true
                })),
            }
        }
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        other => anyhow::bail!("Method not found: {other}"),
    }
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required}
    })
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "search_memos",
            t!("搜索笔记。query 为关键词（空格分隔，需全部命中）；semantic=true 时按语义相关度排序。"),
            json!({
                "query": {"type": "string"},
                "tag": {"type": "string", "description": t!("标签，不含 #，包含子标签")},
                "since": {"type": "string", "description": t!("开始日期 YYYY-MM-DD")},
                "until": {"type": "string", "description": t!("结束日期 YYYY-MM-DD")},
                "semantic": {"type": "boolean"},
                "limit": {"type": "integer", "default": 20}
            }),
            &[],
        ),
        tool("get_memo", t!("读取一条笔记的完整内容、反向链接。"), json!({"id": {"type": "string"}}), &["id"]),
        tool(
            "create_memo",
            t!("创建一条笔记。内容中可使用 #标签、#标签/子标签，以及 [[memo:ID]] 引用其他笔记。"),
            json!({"content": {"type": "string"}}),
            &["content"],
        ),
        tool(
            "update_memo",
            t!("修改一条笔记的内容（旧内容保存在历史版本中）。"),
            json!({"id": {"type": "string"}, "content": {"type": "string"}}),
            &["id", "content"],
        ),
        tool("delete_memo", t!("把一条笔记移入回收站。"), json!({"id": {"type": "string"}}), &["id"]),
        tool("list_tags", t!("列出全部标签及笔记数量。"), json!({}), &[]),
        tool("get_daily_review", t!("获取今天的每日回顾笔记。"), json!({}), &[]),
        tool(
            "get_related_memos",
            t!("获取与某条笔记语义相关的笔记。"),
            json!({"id": {"type": "string"}, "limit": {"type": "integer", "default": 8}}),
            &["id"],
        ),
        tool("get_memory_profile", t!("读取用户的记忆档案（用户画像与记忆文档）。"), json!({}), &[]),
        tool("get_stats", t!("获取记录统计：笔记数、标签数、记录天数、字数等。"), json!({}), &[]),
    ]
}

fn memo_json(store: &Store, id: &str) -> Value {
    let Some(m) = store.get(id) else {
        return Value::Null;
    };
    json!({
        "id": m.id,
        "content": m.content,
        "created_at": time::fmt_datetime(m.created_at),
        "updated_at": time::fmt_datetime(m.updated_at),
        "tags": m.tags,
        "pinned": m.pinned,
        "refs": m.refs,
    })
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty())
}

pub fn call_tool(store: &mut Store, name: &str, args: &Value) -> Result<String> {
    let value = match name {
        "search_memos" => {
            let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(20) as usize;
            let filter = MemoFilter {
                tag: str_arg(args, "tag").map(|t| t.trim_start_matches('#').to_string()),
                date_from: str_arg(args, "since").and_then(time::parse_date),
                date_to: str_arg(args, "until").and_then(time::parse_date),
                ..Default::default()
            };
            let query = str_arg(args, "query").unwrap_or("").to_string();
            let semantic = args.get("semantic").and_then(|s| s.as_bool()).unwrap_or(false);
            let ids: Vec<String> = if semantic && !query.is_empty() {
                let candidates: Vec<String> = filter_memos(store, &filter, SortOrder::CreatedDesc)
                    .iter()
                    .map(|m| m.id.clone())
                    .collect();
                let docs: Vec<(String, String)> = candidates
                    .iter()
                    .filter_map(|id| store.get(id).map(|m| (id.clone(), m.content.clone())))
                    .collect();
                let index = SemanticIndex::build(&docs, Default::default());
                index.search(&query, None, limit, |_| true).into_iter().map(|(id, _)| id).collect()
            } else {
                let filter = MemoFilter { keywords: query, ..filter };
                filter_memos(store, &filter, SortOrder::CreatedDesc)
                    .iter()
                    .take(limit)
                    .map(|m| m.id.clone())
                    .collect()
            };
            json!({"count": ids.len(), "memos": ids.iter().map(|id| memo_json(store, id)).collect::<Vec<_>>()})
        }
        "get_memo" => {
            let id = str_arg(args, "id").ok_or_else(|| anyhow::anyhow!(t!("缺少 id")))?;
            anyhow::ensure!(store.get_active(id).is_some(), "笔记不存在：{id}");
            let mut v = memo_json(store, id);
            v["backlinks"] = json!(store.backlinks(id).iter().map(|m| m.id.clone()).collect::<Vec<_>>());
            v
        }
        "create_memo" => {
            let content = str_arg(args, "content").ok_or_else(|| anyhow::anyhow!(t!("缺少 content")))?;
            let id = store.create_memo(NewMemo {
                content: content.to_string(),
                created_at: None,
                images: vec![],
                source: "mcp".into(),
            })?;
            json!({"created": memo_json(store, &id)})
        }
        "update_memo" => {
            let id = str_arg(args, "id").ok_or_else(|| anyhow::anyhow!(t!("缺少 id")))?;
            let content = str_arg(args, "content").ok_or_else(|| anyhow::anyhow!(t!("缺少 content")))?;
            let images = store
                .get_active(id)
                .map(|m| m.images.clone())
                .ok_or_else(|| anyhow::anyhow!("笔记不存在：{id}"))?;
            store.update_memo(id, content.to_string(), images)?;
            json!({"updated": memo_json(store, id)})
        }
        "delete_memo" => {
            let id = str_arg(args, "id").ok_or_else(|| anyhow::anyhow!(t!("缺少 id")))?;
            anyhow::ensure!(store.get_active(id).is_some(), "笔记不存在：{id}");
            store.trash(id)?;
            json!({"deleted": id, "note": t!("已移入回收站，可在 Mori 中恢复")})
        }
        "list_tags" => {
            let counts = tags::tag_counts(store.active_memos());
            json!(counts.into_iter().map(|(t, c)| json!({"tag": t, "count": c})).collect::<Vec<_>>())
        }
        "get_daily_review" => {
            let cfg = store.settings().review.clone();
            let ids = review::daily_review(store, &cfg, time::today(), 0);
            json!(ids.iter().map(|id| memo_json(store, id)).collect::<Vec<_>>())
        }
        "get_related_memos" => {
            let id = str_arg(args, "id").ok_or_else(|| anyhow::anyhow!(t!("缺少 id")))?;
            let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(8) as usize;
            let docs: Vec<(String, String)> = store
                .active_memos()
                .map(|m| (m.id.clone(), m.content.clone()))
                .collect();
            let index = SemanticIndex::build(&docs, Default::default());
            let related = index.related(id, limit, |_| true);
            json!(related
                .iter()
                .map(|(rid, score)| {
                    let mut v = memo_json(store, rid);
                    v["similarity"] = json!(score);
                    v
                })
                .collect::<Vec<_>>())
        }
        "get_memory_profile" => {
            let profile = store.memory_profile();
            if profile.is_empty() {
                json!({"message": t!("用户尚未生成记忆档案，可在 Mori 的「记忆档案」页面生成")})
            } else {
                json!({"markdown": profile.to_markdown(), "generated_at": time::fmt_datetime(profile.generated_at)})
            }
        }
        "get_stats" => {
            let o = stats::overview(store.active_memos());
            json!({"memos": o.memos, "tags": o.tags, "days": o.days, "chars": o.chars, "streak_days": o.streak})
        }
        other => anyhow::bail!("未知工具：{other}"),
    };
    Ok(serde_json::to_string_pretty(&value)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mori-mcp-{}", crate::core::store::new_id()));
        let mut store = Store::open(&dir).unwrap();
        let created = call_tool(&mut store, "create_memo", &json!({"content": "MCP 写入 #测试"})).unwrap();
        assert!(created.contains("MCP 写入"));
        let found = call_tool(&mut store, "search_memos", &json!({"query": "MCP"})).unwrap();
        assert!(found.contains("\"count\": 1"));
        let tags = call_tool(&mut store, "list_tags", &json!({})).unwrap();
        assert!(tags.contains("测试"));
        let init = handle(&mut store, "tools/list", &json!({})).unwrap();
        assert_eq!(init["tools"].as_array().unwrap().len(), 10);
    }
}
