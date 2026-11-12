//! OpenAI-compatible chat and embedding client plus the prompts used by the AI features.
//! Works with OpenAI, DeepSeek, Moonshot, Qwen, Ollama, LM Studio and any other server
//! that exposes `/chat/completions` and `/embeddings`.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::model::{AiConfig, Memo, MemoryFragment};
use super::time;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

fn reply_in() -> &'static str {
    crate::i18n::language().reply_instruction()
}

fn with_reply_language(prompt: &str) -> String {
    format!("{prompt}\n{}", reply_in())
}

fn endpoint(cfg: &AiConfig, path: &str) -> String {
    format!("{}/{}", cfg.base_url.trim().trim_end_matches('/'), path)
}

fn agent(timeout_secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .build()
        .new_agent()
}

fn post(cfg: &AiConfig, path: &str, body: &Value, timeout_secs: u64) -> Result<ureq::http::Response<ureq::Body>> {
    if !cfg.is_configured() {
        bail!("尚未配置 AI 服务，请先在「设置 → AI 服务」中填写接口地址与模型");
    }
    let mut request = agent(timeout_secs)
        .post(&endpoint(cfg, path))
        .header("Content-Type", "application/json");
    if !cfg.api_key.trim().is_empty() {
        request = request.header("Authorization", &format!("Bearer {}", cfg.api_key.trim()));
    }
    let mut response = request
        .send_json(body)
        .map_err(|e| anyhow!("无法连接 AI 服务：{e}"))?;
    let status = response.status().as_u16();
    if status >= 400 {
        let text = response.body_mut().read_to_string().unwrap_or_default();
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .or_else(|| v.get("message"))
                    .or_else(|| v.get("error"))
                    .and_then(|m| m.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| text.chars().take(300).collect());
        bail!("AI 服务返回错误 {status}：{message}");
    }
    Ok(response)
}

/// Streams a chat completion. `on_delta` receives each text fragment and returns
/// `false` to stop early. Returns the full text.
pub fn chat_stream(
    cfg: &AiConfig,
    messages: &[ChatMessage],
    mut on_delta: impl FnMut(&str) -> bool,
) -> Result<String> {
    let body = json!({
        "model": cfg.chat_model.trim(),
        "messages": messages,
        "temperature": cfg.temperature,
        "stream": true,
    });
    let response = post(cfg, "chat/completions", &body, 300)?;
    let reader = BufReader::new(response.into_body().into_reader());
    let mut full = String::new();
    let mut raw_json = String::new();
    for line in reader.lines() {
        let line = line.map_err(|e| anyhow!("读取 AI 响应失败：{e}"))?;
        let line = line.trim();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let Some(payload) = line.strip_prefix("data:") else {
            raw_json.push_str(line);
            continue;
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            break;
        }
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            continue;
        };
        if let Some(err) = value.pointer("/error/message").and_then(|m| m.as_str()) {
            bail!("AI 服务错误：{err}");
        }
        if let Some(delta) = value
            .pointer("/choices/0/delta/content")
            .and_then(|c| c.as_str())
            && !delta.is_empty()
        {
            full.push_str(delta);
            if !on_delta(delta) {
                return Ok(full);
            }
        }
    }
    if full.is_empty() && !raw_json.is_empty() {
        let value: Value = serde_json::from_str(&raw_json)
            .map_err(|_| anyhow!("无法解析 AI 响应：{}", raw_json.chars().take(200).collect::<String>()))?;
        if let Some(content) = value.pointer("/choices/0/message/content").and_then(|c| c.as_str()) {
            full = content.to_string();
            on_delta(&full);
        }
    }
    if full.is_empty() {
        bail!("AI 服务没有返回内容");
    }
    Ok(full)
}

pub fn chat_complete(cfg: &AiConfig, messages: &[ChatMessage]) -> Result<String> {
    let body = json!({
        "model": cfg.chat_model.trim(),
        "messages": messages,
        "temperature": cfg.temperature.min(0.5),
        "stream": false,
    });
    let response = post(cfg, "chat/completions", &body, 300)?;
    let value: Value = response
        .into_body()
        .read_json()
        .map_err(|e| anyhow!("无法解析 AI 响应：{e}"))?;
    value
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("AI 响应中没有内容"))
}

pub fn embed(cfg: &AiConfig, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
    let body = json!({
        "model": cfg.embedding_model.trim(),
        "input": inputs,
    });
    let response = post(cfg, "embeddings", &body, 120)?;
    let value: Value = response
        .into_body()
        .read_json()
        .map_err(|e| anyhow!("无法解析 Embedding 响应：{e}"))?;
    let data = value
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow!("Embedding 响应格式不正确"))?;
    let mut out = vec![Vec::new(); inputs.len()];
    for (pos, item) in data.iter().enumerate() {
        let index = item.get("index").and_then(|i| i.as_u64()).map(|i| i as usize).unwrap_or(pos);
        let mut vector: Vec<f32> = item
            .get("embedding")
            .and_then(|e| e.as_array())
            .ok_or_else(|| anyhow!("Embedding 响应缺少向量"))?
            .iter()
            .filter_map(|x| x.as_f64().map(|f| f as f32))
            .collect();
        super::semantic::normalize_dense(&mut vector);
        if index < out.len() {
            out[index] = vector;
        }
    }
    if out.iter().any(|v| v.is_empty()) {
        bail!("Embedding 响应条数与请求不符");
    }
    Ok(out)
}

pub fn test_connection(cfg: &AiConfig) -> Result<String> {
    let reply = chat_complete(
        cfg,
        &[ChatMessage::user("请只回复“连接成功”四个字。")],
    )?;
    let mut summary = t!("对话模型 {} 可用：{}", cfg.chat_model, reply.trim());
    if cfg.uses_remote_embeddings() {
        let v = embed(cfg, &["测试".to_string()])?;
        summary.push_str(&t!("；Embedding 模型 {} 可用（{} 维）", cfg.embedding_model, v[0].len()));
    }
    Ok(summary)
}

/// Extracts the first JSON value from a model reply that may wrap it in prose or code fences.
pub fn extract_json(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        return Some(v);
    }
    let starts: Vec<usize> = text
        .char_indices()
        .filter(|(_, c)| *c == '{' || *c == '[')
        .map(|(i, _)| i)
        .collect();
    for start in starts {
        let open = text.as_bytes()[start];
        let close = if open == b'{' { '}' } else { ']' };
        for (end, _) in text.char_indices().rev().filter(|(_, c)| *c == close) {
            if end <= start {
                break;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&text[start..=end]) {
                return Some(v);
            }
        }
    }
    None
}

// -------------------------------------------------------------- prompts

pub struct Perspective {
    pub name: &'static str,
    pub description: &'static str,
    pub instruction: &'static str,
}

pub const PERSPECTIVES: &[Perspective] = &[
    Perspective {
        name: "默认洞察",
        description: "识别核心主题、揭示盲点、提出好问题",
        instruction: "从第三方视角深度阅读这些笔记：先用一两句话点出核心主题；再指出 1-2 个可能的思维盲点或隐藏联系；最后提出 2-3 个能推动思考的好问题。",
    },
    Perspective {
        name: "每日肯定",
        description: "看见努力与品质，给出真诚的肯定",
        instruction: "从笔记中找出值得被肯定的努力、品质与微小进步，用温暖、具体、不浮夸的语言肯定用户，并给出一句可以带走的鼓励。",
    },
    Perspective {
        name: "行动指南",
        description: "把想法变成清晰的小步行动",
        instruction: "把笔记中的想法与困惑整理成清晰的行动指南：明确目标，列出 3-5 个今天就能开始的小步行动，并指出最可能的阻碍及应对方式。",
    },
    Perspective {
        name: "意义雷达",
        description: "找到忙碌背后的意义",
        instruction: "像雷达一样扫描这些笔记，找出用户在忙碌与琐碎中真正在意的东西、反复出现的价值线索，以及这些经历对他的意义。",
    },
    Perspective {
        name: "心底之问",
        description: "提出直抵内心的问题",
        instruction: "不要给答案。基于笔记提出 4-6 个直抵内心、具体而不空泛的问题，每个问题后用一句话说明为什么值得想一想。",
    },
    Perspective {
        name: "亚里士多德",
        description: "德性、中道与幸福",
        instruction: "以亚里士多德的口吻与思想（德性伦理、中道、实践智慧、eudaimonia）解读这些笔记，给出他会怎样看待用户当下的处境，以及他会建议的实践。",
    },
    Perspective {
        name: "塞涅卡",
        description: "斯多葛式的来信",
        instruction: "以斯多葛哲学家塞涅卡的口吻，给用户写一封简短的书信：区分可控与不可控，谈时间、欲望与内心的安宁，语气真诚克制。",
    },
    Perspective {
        name: "价值澄清",
        description: "理清价值观与其中的冲突",
        instruction: "澄清笔记中体现出的价值观：列出最核心的 3-5 项价值，指出它们之间可能的冲突或排序，以及用户行为与所述价值是否一致。",
    },
    Perspective {
        name: "逆向思考",
        description: "反过来想，找出被忽略的假设",
        instruction: "运用逆向思维：反过来看笔记中的判断与计划，指出被忽略的前提假设、可能的反面证据、失败的方式，并给出如何避免。",
    },
    Perspective {
        name: "主题回顾",
        description: "跨标签找出长期隐藏的主题",
        instruction: "忽略标签分类，跨时间、跨标签找出这些笔记中反复出现却未被命名的 2-3 个隐藏主题，按时间线串起关键笔记，说明想法如何演变。",
    },
];

pub const INSIGHT_SYSTEM: &str = "你是用户的思考伙伴，正在阅读用户自己写下的笔记。你的目标不是替用户思考，而是帮助他更深入地思考、看见自己。\n\
要求：\n\
1. 只依据提供的笔记，不要编造笔记中没有的事实；\n\
2. 每当观点来自某条笔记，在句末用 [编号] 标注来源，例如 [3]；\n\
3. 使用 Markdown 输出，结构清晰，300-600 字；\n\
4. 语气真诚、具体，避免空洞的鸡汤和说教。";

/// Formats memos as numbered context entries, newest first, within a character budget.
pub fn format_notes(memos: &[&Memo], budget_chars: usize) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut ids = Vec::new();
    for memo in memos {
        let tags = if memo.tags.is_empty() {
            String::new()
        } else {
            format!(" · {}", memo.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "))
        };
        let body: String = super::parse::plain_text(&memo.content).chars().take(800).collect();
        let entry = format!(
            "[{}] {}{}\n{}\n\n",
            ids.len() + 1,
            time::fmt_minute(memo.created_at),
            tags,
            body
        );
        if out.chars().count() + entry.chars().count() > budget_chars && !ids.is_empty() {
            break;
        }
        out.push_str(&entry);
        ids.push(memo.id.clone());
    }
    (out, ids)
}

/// Picks a representative subset spread across time when there are too many memos.
pub fn sample_memos<'a>(memos: &[&'a Memo], max: usize) -> Vec<&'a Memo> {
    if memos.len() <= max {
        return memos.to_vec();
    }
    let mut sorted = memos.to_vec();
    sorted.sort_by_key(|m| std::cmp::Reverse(m.created_at));
    let recent = max / 3;
    let mut picked: Vec<&Memo> = sorted[..recent].to_vec();
    let rest = &sorted[recent..];
    let remaining = max - recent;
    for i in 0..remaining {
        picked.push(rest[i * rest.len() / remaining]);
    }
    picked
}

pub fn insight_messages(instruction: &str, perspective_name: &str, notes: &str, note_count: usize) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(with_reply_language(INSIGHT_SYSTEM)),
        ChatMessage::user(format!(
            "洞察视角：{perspective_name}\n{instruction}\n\n以下是我的 {note_count} 条笔记：\n\n{notes}"
        )),
    ]
}

pub fn chat_system(memory: Option<&str>) -> String {
    let mut s = String::from(
        "你是用户的笔记 AI 伙伴，读过用户的笔记，陪他回顾与思考。\n\
回答规则：\n\
1. 优先依据「相关笔记」作答，引用时在句末标注 [编号]；\n\
2. 笔记中没有的信息要坦诚说明，可以补充通用知识，但要和笔记中的事实区分开；\n\
3. 简洁、具体，适当使用 Markdown。",
    );
    if let Some(memory) = memory.filter(|m| !m.trim().is_empty()) {
        s.push_str("\n\n以下是用户的记忆档案，帮助你了解他：\n");
        s.push_str(memory);
    }
    with_reply_language(&s)
}

pub fn chat_user_turn(question: &str, notes: &str) -> String {
    if notes.trim().is_empty() {
        format!("（没有检索到相关笔记）\n\n我的问题：{question}")
    } else {
        format!("相关笔记：\n\n{notes}\n我的问题：{question}")
    }
}

pub const MEMORY_CATEGORIES: &[&str] = &[
    "身份", "性格", "价值观", "偏好", "兴趣", "工作", "项目", "目标", "关系", "习惯", "经历", "情绪",
];

pub fn memory_extract_messages(notes: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage::system(with_reply_language(&format!(
            "你是一名细致的记忆整理员。阅读用户的笔记，提取关于用户本人的结构化记忆碎片。\n\
每条碎片是一个具体的特质、事实或事件，用第三人称简洁陈述（如“用户在做独立开发”）。\n\
类别只能从以下选择：{}。\n\
只输出 JSON：{{\"fragments\":[{{\"category\":\"类别\",\"content\":\"内容\",\"refs\":[笔记编号]}}]}}，不要输出其他文字。\n\
没有关于用户本人的信息时输出 {{\"fragments\":[]}}。",
            MEMORY_CATEGORIES.join("、")
        ))),
        ChatMessage::user(format!("笔记如下：\n\n{notes}")),
    ]
}

pub fn parse_fragments(reply: &str, ids: &[String]) -> Vec<MemoryFragment> {
    let Some(value) = extract_json(reply) else {
        return Vec::new();
    };
    let list = value
        .get("fragments")
        .cloned()
        .unwrap_or(value)
        .as_array()
        .cloned()
        .unwrap_or_default();
    list.iter()
        .filter_map(|item| {
            let content = item.get("content")?.as_str()?.trim().to_string();
            if content.is_empty() {
                return None;
            }
            let category = item
                .get("category")
                .and_then(|c| c.as_str())
                .unwrap_or("经历")
                .to_string();
            let memo_ids = item
                .get("refs")
                .and_then(|r| r.as_array())
                .map(|refs| {
                    refs.iter()
                        .filter_map(|n| n.as_u64())
                        .filter_map(|n| ids.get((n as usize).wrapping_sub(1)).cloned())
                        .collect()
                })
                .unwrap_or_default();
            Some(MemoryFragment {
                category,
                content,
                memo_ids,
            })
        })
        .collect()
}

pub fn memory_integrate_messages(fragments: &[MemoryFragment]) -> Vec<ChatMessage> {
    let mut list = String::new();
    for f in fragments {
        list.push_str(&format!("- [{}] {}\n", f.category, f.content));
    }
    vec![
        ChatMessage::system(with_reply_language(
            "你负责把记忆碎片整合成一份 AI 能读懂的记忆档案，供其他 AI 助手了解用户。\n\
输出两部分，严格使用下面的分隔标记：\n\
=====PROFILE=====\n\
用户画像（Markdown）：身份、性格、价值观、偏好等相对稳定的信息，分小节，条目化。\n\
=====MEMORY=====\n\
记忆文档（Markdown）：当前的工作、兴趣、项目、目标、近期在想的事等会随时间变化的信息，分小节，条目化。\n\
要求：合并重复，保留具体细节，不编造，不评价，使用第三人称“用户”。",
        )),
        ChatMessage::user(format!("记忆碎片：\n{list}")),
    ]
}

pub fn parse_memory_documents(reply: &str) -> (String, String) {
    let profile_marker = "=====PROFILE=====";
    let memory_marker = "=====MEMORY=====";
    match (reply.find(profile_marker), reply.find(memory_marker)) {
        (Some(p), Some(m)) if p < m => (
            reply[p + profile_marker.len()..m].trim().to_string(),
            reply[m + memory_marker.len()..].trim().to_string(),
        ),
        (None, Some(m)) => (
            reply[..m].trim().to_string(),
            reply[m + memory_marker.len()..].trim().to_string(),
        ),
        _ => (reply.trim().to_string(), String::new()),
    }
}

pub fn tag_suggest_messages(content: &str, existing_tags: &[String]) -> Vec<ChatMessage> {
    let tags: Vec<String> = existing_tags.iter().take(200).map(|t| format!("#{t}")).collect();
    vec![
        ChatMessage::system(with_reply_language(
            "你帮助用户为笔记选择标签。优先复用用户已有的标签体系，可使用多级标签（如 读书/小说）。\
标签不含空格和 # 号。只输出 JSON 字符串数组，1-3 个标签，例如 [\"读书/小说\",\"想法\"]。",
        )),
        ChatMessage::user(format!(
            "已有标签：{}\n\n笔记内容：\n{}",
            if tags.is_empty() { "（暂无）".to_string() } else { tags.join(" ") },
            content
        )),
    ]
}

pub fn parse_string_list(reply: &str) -> Vec<String> {
    extract_json(reply)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.trim().trim_start_matches('#').replace(' ', "_"))
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn peak_naming_messages(groups: &[Vec<String>]) -> Vec<ChatMessage> {
    let mut body = String::new();
    for (i, group) in groups.iter().enumerate() {
        body.push_str(&format!("第 {} 组：\n", i + 1));
        for snippet in group.iter().take(8) {
            body.push_str(&format!("- {snippet}\n"));
        }
        body.push('\n');
    }
    vec![
        ChatMessage::system(with_reply_language(
            "下面每组笔记代表用户反复思考的一个主题山峰。为每组起一个 2-8 个字、有画面感的主题名。\
只输出 JSON 字符串数组，顺序与组一一对应。",
        )),
        ChatMessage::user(body),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_extraction() {
        let v = extract_json("好的：\n```json\n[\"读书\", \"想法\"]\n```").unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        let tags = parse_string_list("[\"#读书/小说\", \"想 法\"]");
        assert_eq!(tags, vec!["读书/小说", "想_法"]);
    }

    #[test]
    fn memory_docs_split() {
        let (p, m) = parse_memory_documents("=====PROFILE=====\n画像\n=====MEMORY=====\n文档");
        assert_eq!(p, "画像");
        assert_eq!(m, "文档");
    }
}
