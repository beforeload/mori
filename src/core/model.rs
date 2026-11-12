use serde::{Deserialize, Serialize};

use crate::i18n::Language;

use super::parse;

pub type MemoId = String;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memo {
    pub id: MemoId,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub deleted_at: Option<i64>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(skip)]
    pub tags: Vec<String>,
    #[serde(skip)]
    pub refs: Vec<MemoId>,
    #[serde(skip)]
    pub char_count: usize,
    #[serde(skip)]
    pub has_link: bool,
}

fn default_source() -> String {
    "app".into()
}

impl Memo {
    pub fn refresh_derived(&mut self) {
        self.tags = parse::extract_tags(&self.content);
        self.refs = parse::extract_refs(&self.content);
        self.char_count = parse::plain_char_count(&self.content);
        self.has_link = parse::has_link(&self.content);
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// Whether the memo carries `tag` itself or one of its descendants.
    pub fn has_tag_or_child(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| tag_matches(t, tag))
    }

    pub fn snippet(&self, max_chars: usize) -> String {
        parse::snippet(&self.content, max_chars)
    }
}

/// `#a/b/c` matches the filters `a`, `a/b` and `a/b/c`.
pub fn tag_matches(tag: &str, filter: &str) -> bool {
    tag == filter
        || (tag.len() > filter.len()
            && tag.starts_with(filter)
            && tag.as_bytes()[filter.len()] == b'/')
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TagMeta {
    pub pinned: bool,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AiConfig {
    pub base_url: String,
    pub api_key: String,
    pub chat_model: String,
    pub embedding_model: String,
    pub temperature: f32,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            chat_model: "gpt-4o-mini".into(),
            embedding_model: String::new(),
            temperature: 0.7,
        }
    }
}

impl AiConfig {
    pub fn is_configured(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.chat_model.trim().is_empty()
    }

    pub fn uses_remote_embeddings(&self) -> bool {
        self.is_configured() && !self.embedding_model.trim().is_empty()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ReviewScope {
    #[default]
    All,
    IncludeTags,
    ExcludeTags,
    Untagged,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ReviewRange {
    #[default]
    All,
    OneYear,
    SixMonths,
    ThreeMonths,
    OneMonth,
}

impl ReviewRange {
    pub const ALL: [ReviewRange; 5] = [
        ReviewRange::All,
        ReviewRange::OneYear,
        ReviewRange::SixMonths,
        ReviewRange::ThreeMonths,
        ReviewRange::OneMonth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ReviewRange::All => t!("全部时间"),
            ReviewRange::OneYear => t!("1 年内"),
            ReviewRange::SixMonths => t!("6 个月内"),
            ReviewRange::ThreeMonths => t!("3 个月内"),
            ReviewRange::OneMonth => t!("1 个月内"),
        }
    }

    pub fn days(self) -> Option<i64> {
        match self {
            ReviewRange::All => None,
            ReviewRange::OneYear => Some(365),
            ReviewRange::SixMonths => Some(182),
            ReviewRange::ThreeMonths => Some(91),
            ReviewRange::OneMonth => Some(30),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReviewConfig {
    pub scope: ReviewScope,
    pub tags: Vec<String>,
    pub range: ReviewRange,
    pub count: usize,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            scope: ReviewScope::All,
            tags: Vec::new(),
            range: ReviewRange::All,
            count: 8,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CustomPerspective {
    pub name: String,
    pub prompt: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => t!("跟随系统"),
            Appearance::Light => t!("浅色"),
            Appearance::Dark => t!("深色"),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum BodyFont {
    #[default]
    System,
    PingFang,
    Songti,
    Kaiti,
    Mono,
}

impl BodyFont {
    pub const ALL: [BodyFont; 5] = [
        BodyFont::System,
        BodyFont::PingFang,
        BodyFont::Songti,
        BodyFont::Kaiti,
        BodyFont::Mono,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BodyFont::System => t!("默认"),
            BodyFont::PingFang => t!("苹方"),
            BodyFont::Songti => t!("宋体"),
            BodyFont::Kaiti => t!("楷体"),
            BodyFont::Mono => t!("等宽"),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum LineSpacing {
    Compact,
    #[default]
    Default,
    Loose,
}

impl LineSpacing {
    pub const ALL: [LineSpacing; 3] = [LineSpacing::Compact, LineSpacing::Default, LineSpacing::Loose];

    pub fn label(self) -> &'static str {
        match self {
            LineSpacing::Compact => t!("紧凑"),
            LineSpacing::Default => t!("默认"),
            LineSpacing::Loose => t!("宽松"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub ai: AiConfig,
    pub review: ReviewConfig,
    pub appearance: Appearance,
    pub body_font: BodyFont,
    pub body_font_size: f32,
    pub line_spacing: LineSpacing,
    pub show_source: bool,
    pub show_agent_button: bool,
    pub fold_long_memos: bool,
    pub show_backlinks: bool,
    pub show_heatmap: bool,
    pub sort_by_updated: bool,
    pub sort_ascending: bool,
    pub lock_enabled: bool,
    pub lock_hash: String,
    pub lock_salt: String,
    pub api_enabled: bool,
    pub api_port: u16,
    pub api_token: String,
    pub custom_perspectives: Vec<CustomPerspective>,
    pub nickname: String,
    pub font_size: f32,
    pub language: Language,
    /// User choice for wide windows; narrow windows always tuck the sidebar away.
    pub sidebar_hidden: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ai: AiConfig::default(),
            review: ReviewConfig::default(),
            appearance: Appearance::System,
            body_font: BodyFont::System,
            body_font_size: 14.,
            line_spacing: LineSpacing::Default,
            show_source: true,
            show_agent_button: true,
            fold_long_memos: true,
            show_backlinks: true,
            show_heatmap: true,
            sort_by_updated: false,
            sort_ascending: false,
            lock_enabled: false,
            lock_hash: String::new(),
            lock_salt: String::new(),
            api_enabled: false,
            api_port: 17321,
            api_token: String::new(),
            custom_perspectives: Vec::new(),
            nickname: "我".into(),
            font_size: 15.,
            sidebar_hidden: false,
            language: Language::ZhHans,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MemoVersion {
    pub id: i64,
    pub content: String,
    pub saved_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InsightRecord {
    pub id: i64,
    pub created_at: i64,
    pub perspective: String,
    pub scope: String,
    pub memo_ids: Vec<MemoId>,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatRecord {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub refs: Vec<MemoId>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryFragment {
    pub category: String,
    pub content: String,
    #[serde(default)]
    pub memo_ids: Vec<MemoId>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MemoryProfile {
    pub profile: String,
    pub document: String,
    pub fragments: Vec<MemoryFragment>,
    pub generated_at: i64,
    pub memo_count: usize,
}

impl MemoryProfile {
    pub fn is_empty(&self) -> bool {
        self.profile.trim().is_empty() && self.document.trim().is_empty()
    }

    pub fn to_markdown(&self) -> String {
        format!(
            "# 我的记忆档案\n\n## 用户画像\n\n{}\n\n## 记忆文档\n\n{}\n",
            self.profile.trim(),
            self.document.trim()
        )
    }
}
