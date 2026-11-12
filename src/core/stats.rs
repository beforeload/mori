use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Datelike, NaiveDate, Timelike};

use super::model::Memo;
use super::time;

#[derive(Clone, Debug, Default)]
pub struct DayStat {
    pub count: usize,
    pub chars: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Overview {
    pub memos: usize,
    pub tags: usize,
    pub days: usize,
    pub chars: usize,
    pub streak: usize,
}

#[derive(Clone, Debug, Default)]
pub struct MonthStat {
    pub memos: usize,
    pub chars: usize,
    pub days: usize,
    pub max_day_memos: usize,
    pub max_day_chars: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Distributions {
    pub hours: [usize; 24],
    pub weekdays: [usize; 7],
    pub lengths: Vec<(String, usize)>,
    pub sources: Vec<(String, usize)>,
    pub media: Vec<(String, usize)>,
}

pub fn per_day<'a>(memos: impl Iterator<Item = &'a Memo>) -> HashMap<NaiveDate, DayStat> {
    let mut map: HashMap<NaiveDate, DayStat> = HashMap::new();
    for memo in memos {
        let entry = map.entry(time::date_of(memo.created_at)).or_default();
        entry.count += 1;
        entry.chars += memo.char_count;
    }
    map
}

pub fn overview<'a>(memos: impl Iterator<Item = &'a Memo> + Clone) -> Overview {
    let days = per_day(memos.clone());
    let mut tags: HashSet<&str> = HashSet::new();
    let mut count = 0;
    let mut chars = 0;
    for memo in memos {
        count += 1;
        chars += memo.char_count;
        for tag in &memo.tags {
            for prefix in super::tags::tag_prefixes(tag) {
                tags.insert(prefix);
            }
        }
    }
    let mut streak = 0;
    let mut day = time::today();
    if !days.contains_key(&day) {
        day = day.pred_opt().unwrap();
    }
    while days.contains_key(&day) {
        streak += 1;
        day = day.pred_opt().unwrap();
    }
    Overview {
        memos: count,
        tags: tags.len(),
        days: days.len(),
        chars,
        streak,
    }
}

/// Heat level 0-5 for a day count, relative to the busiest day shown.
pub fn heat_level(count: usize, max: usize) -> usize {
    if count == 0 {
        return 0;
    }
    if max <= 5 {
        return count.min(5);
    }
    let ratio = count as f32 / max as f32;
    ((ratio * 5.0).ceil() as usize).clamp(1, 5)
}

pub fn month_stat(days: &HashMap<NaiveDate, DayStat>, month: NaiveDate) -> MonthStat {
    let start = time::month_start(month);
    let end = time::add_months(start, 1);
    let mut stat = MonthStat::default();
    for (date, day) in days {
        if *date >= start && *date < end {
            stat.memos += day.count;
            stat.chars += day.chars;
            stat.days += 1;
            stat.max_day_memos = stat.max_day_memos.max(day.count);
            stat.max_day_chars = stat.max_day_chars.max(day.chars);
        }
    }
    stat
}

/// Memo count per month of `year`.
pub fn year_months(days: &HashMap<NaiveDate, DayStat>, year: i32) -> [DayStat; 12] {
    let mut months: [DayStat; 12] = Default::default();
    for (date, day) in days {
        if date.year() == year {
            let m = &mut months[date.month0() as usize];
            m.count += day.count;
            m.chars += day.chars;
        }
    }
    months
}

pub fn distributions<'a>(memos: impl Iterator<Item = &'a Memo>) -> Distributions {
    let mut d = Distributions::default();
    let mut lengths: BTreeMap<usize, usize> = BTreeMap::new();
    let mut sources: BTreeMap<String, usize> = BTreeMap::new();
    let (mut image, mut link, mut reference, mut plain) = (0, 0, 0, 0);
    for memo in memos {
        let dt = time::local(memo.created_at);
        d.hours[dt.hour() as usize] += 1;
        d.weekdays[dt.weekday().num_days_from_monday() as usize] += 1;
        let bucket = match memo.char_count {
            0..=20 => 0,
            21..=50 => 1,
            51..=100 => 2,
            101..=200 => 3,
            201..=500 => 4,
            _ => 5,
        };
        *lengths.entry(bucket).or_default() += 1;
        *sources.entry(source_label(&memo.source).to_string()).or_default() += 1;
        if !memo.images.is_empty() {
            image += 1;
        }
        if memo.has_link {
            link += 1;
        }
        if !memo.refs.is_empty() {
            reference += 1;
        }
        if memo.images.is_empty() && !memo.has_link && memo.refs.is_empty() {
            plain += 1;
        }
    }
    let labels = ["≤20", "21-50", "51-100", "101-200", "201-500", ">500"];
    d.lengths = (0..6)
        .map(|i| (labels[i].to_string(), *lengths.get(&i).unwrap_or(&0)))
        .collect();
    d.sources = sources.into_iter().collect();
    d.media = vec![
        (t!("纯文字").into(), plain),
        (t!("图片").into(), image),
        (t!("链接").into(), link),
        (t!("引用").into(), reference),
    ];
    d
}

pub fn source_label(source: &str) -> &str {
    match source {
        "app" => t!("应用内"),
        "import" => t!("导入"),
        "api" => "API",
        "mcp" => "MCP",
        "ai" => t!("AI 对话"),
        other => other,
    }
}

pub fn weekday_label(i: usize) -> &'static str {
    match crate::i18n::language() {
        crate::i18n::Language::En => ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][i % 7],
        _ => ["一", "二", "三", "四", "五", "六", "日"][i % 7],
    }
}

/// Monday of the week containing `date`.
pub fn week_start(date: NaiveDate) -> NaiveDate {
    let offset = date.weekday().num_days_from_monday() as i64;
    date - chrono::Duration::days(offset)
}
