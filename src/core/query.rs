use chrono::{Datelike, NaiveDate};

use super::model::{Memo, tag_matches};
use super::store::Store;
use super::time;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickFilter {
    HasImage,
    HasLink,
    HasRef,
    NoTag,
    OnThisDay,
}

impl QuickFilter {
    pub const ALL: [QuickFilter; 5] = [
        QuickFilter::HasImage,
        QuickFilter::HasLink,
        QuickFilter::HasRef,
        QuickFilter::NoTag,
        QuickFilter::OnThisDay,
    ];

    pub fn label(self) -> &'static str {
        match self {
            QuickFilter::HasImage => t!("有图片"),
            QuickFilter::HasLink => t!("有链接"),
            QuickFilter::HasRef => t!("有批注引用"),
            QuickFilter::NoTag => t!("无标签"),
            QuickFilter::OnThisDay => t!("往年今日"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    CreatedDesc,
    CreatedAsc,
    UpdatedDesc,
    UpdatedAsc,
}

impl SortOrder {
    pub const ALL: [SortOrder; 4] = [
        SortOrder::CreatedDesc,
        SortOrder::CreatedAsc,
        SortOrder::UpdatedDesc,
        SortOrder::UpdatedAsc,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortOrder::CreatedDesc => t!("创建时间 · 新→旧"),
            SortOrder::CreatedAsc => t!("创建时间 · 旧→新"),
            SortOrder::UpdatedDesc => t!("编辑时间 · 新→旧"),
            SortOrder::UpdatedAsc => t!("编辑时间 · 旧→新"),
        }
    }

    pub fn by_updated(self) -> bool {
        matches!(self, SortOrder::UpdatedDesc | SortOrder::UpdatedAsc)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoFilter {
    pub tag: Option<String>,
    pub day: Option<NaiveDate>,
    pub keywords: String,
    pub quick: Option<QuickFilter>,
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
    pub include_tags: Vec<String>,
    pub exclude_tags: Vec<String>,
    pub source: Option<String>,
}

impl MemoFilter {
    pub fn is_empty(&self) -> bool {
        *self == MemoFilter::default()
    }

    pub fn has_advanced(&self) -> bool {
        self.date_from.is_some()
            || self.date_to.is_some()
            || !self.include_tags.is_empty()
            || !self.exclude_tags.is_empty()
    }

    pub fn keyword_list(&self) -> Vec<String> {
        self.keywords
            .split_whitespace()
            .map(|s| s.to_string())
            .collect()
    }

    pub fn matches(&self, memo: &Memo, keywords: &[String]) -> bool {
        if memo.is_deleted() {
            return false;
        }
        if let Some(tag) = &self.tag
            && !memo.has_tag_or_child(tag)
        {
            return false;
        }
        if let Some(source) = &self.source
            && memo.source != *source
        {
            return false;
        }
        let date = time::date_of(memo.created_at);
        if let Some(day) = self.day
            && date != day
        {
            return false;
        }
        if let Some(from) = self.date_from
            && date < from
        {
            return false;
        }
        if let Some(to) = self.date_to
            && date > to
        {
            return false;
        }
        if !self.include_tags.is_empty()
            && !self
                .include_tags
                .iter()
                .any(|t| memo.tags.iter().any(|mt| tag_matches(mt, t)))
        {
            return false;
        }
        if self
            .exclude_tags
            .iter()
            .any(|t| memo.tags.iter().any(|mt| tag_matches(mt, t)))
        {
            return false;
        }
        if let Some(quick) = self.quick {
            let ok = match quick {
                QuickFilter::HasImage => !memo.images.is_empty(),
                QuickFilter::HasLink => memo.has_link,
                QuickFilter::HasRef => !memo.refs.is_empty(),
                QuickFilter::NoTag => memo.tags.is_empty(),
                QuickFilter::OnThisDay => {
                    let today = time::today();
                    date.year() < today.year()
                        && date.month() == today.month()
                        && date.day() == today.day()
                }
            };
            if !ok {
                return false;
            }
        }
        if !keywords.is_empty() {
            let haystack = memo.content.to_lowercase();
            if !keywords
                .iter()
                .all(|k| haystack.contains(&k.to_lowercase()))
            {
                return false;
            }
        }
        true
    }
}

pub fn filter_memos<'a>(store: &'a Store, filter: &MemoFilter, sort: SortOrder) -> Vec<&'a Memo> {
    let keywords = filter.keyword_list();
    let mut list: Vec<&Memo> = store
        .active_memos()
        .filter(|m| filter.matches(m, &keywords))
        .collect();
    sort_memos(&mut list, sort);
    list
}

pub fn sort_memos(list: &mut [&Memo], sort: SortOrder) {
    list.sort_by(|a, b| {
        let pinned = b.pinned.cmp(&a.pinned);
        if pinned != std::cmp::Ordering::Equal {
            return pinned;
        }
        match sort {
            SortOrder::CreatedDesc => b.created_at.cmp(&a.created_at),
            SortOrder::CreatedAsc => a.created_at.cmp(&b.created_at),
            SortOrder::UpdatedDesc => b.updated_at.cmp(&a.updated_at),
            SortOrder::UpdatedAsc => a.updated_at.cmp(&b.updated_at),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::store::{NewMemo, new_id};

    #[test]
    fn source_and_tag_filters_combine() {
        let dir = std::env::temp_dir().join(format!("mori-query-{}", new_id()));
        let mut store = Store::open(&dir).unwrap();
        for (content, source) in [("甲 #学习", "app"), ("乙 #学习", "mcp"), ("丙 #生活", "mcp")] {
            store
                .create_memo(NewMemo { content: content.into(), created_at: None, images: vec![], source: source.into() })
                .unwrap();
        }
        let contents = |filter: &MemoFilter| -> Vec<String> {
            let mut v: Vec<String> = filter_memos(&store, filter, SortOrder::CreatedDesc)
                .iter()
                .map(|m| m.content.clone())
                .collect();
            v.sort();
            v
        };
        let mcp = MemoFilter { source: Some("mcp".into()), ..Default::default() };
        assert_eq!(contents(&mcp), vec!["丙 #生活", "乙 #学习"]);
        let mcp_study = MemoFilter { tag: Some("学习".into()), ..mcp };
        assert_eq!(contents(&mcp_study), vec!["乙 #学习"]);
        assert!(!mcp_study.is_empty());
    }
}
