use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, TimeZone};

pub fn now() -> i64 {
    Local::now().timestamp()
}

pub fn local(ts: i64) -> DateTime<Local> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(Local::now)
}

pub fn date_of(ts: i64) -> NaiveDate {
    local(ts).date_naive()
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

pub fn fmt_datetime(ts: i64) -> String {
    local(ts).format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn fmt_minute(ts: i64) -> String {
    local(ts).format("%Y-%m-%d %H:%M").to_string()
}

pub fn fmt_date(ts: i64) -> String {
    local(ts).format("%Y-%m-%d").to_string()
}

pub fn fmt_day(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

pub fn fmt_month(date: NaiveDate) -> String {
    match crate::i18n::language() {
        crate::i18n::Language::En => date.format("%B %Y").to_string(),
        _ => date.format("%Y 年 %-m 月").to_string(),
    }
}

pub fn relative_day(ts: i64) -> String {
    let days = (today() - date_of(ts)).num_days();
    let clock = local(ts).format("%H:%M").to_string();
    match days {
        0 => t!("今日 {}", clock),
        1 => t!("昨日 {}", clock),
        2..=6 => t!("{} 天前", days),
        _ => fmt_minute(ts),
    }
}

pub fn start_of_day(date: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .map(|d| d.timestamp())
        .unwrap_or(0)
}

pub fn parse_datetime(text: &str) -> Option<i64> {
    let text = text.trim();
    for fmt in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y/%m/%d %H:%M:%S",
        "%Y/%m/%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%SZ",
        "%Y年%m月%d日 %H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(text, fmt) {
            return Local.from_local_datetime(&dt).earliest().map(|d| d.timestamp());
        }
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return Some(dt.timestamp());
    }
    for fmt in ["%Y-%m-%d", "%Y/%m/%d", "%Y年%m月%d日"] {
        if let Ok(d) = NaiveDate::parse_from_str(text, fmt) {
            return Some(start_of_day(d) + 12 * 3600);
        }
    }
    None
}

pub fn parse_date(text: &str) -> Option<NaiveDate> {
    let text = text.trim();
    for fmt in ["%Y-%m-%d", "%Y/%m/%d", "%Y%m%d"] {
        if let Ok(d) = NaiveDate::parse_from_str(text, fmt) {
            return Some(d);
        }
    }
    None
}

pub fn month_start(date: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap()
}

pub fn add_months(date: NaiveDate, months: i32) -> NaiveDate {
    let total = date.year() * 12 + date.month0() as i32 + months;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) as u32 + 1;
    NaiveDate::from_ymd_opt(year, month, 1).unwrap()
}

pub fn days_in_month(date: NaiveDate) -> u32 {
    let next = add_months(month_start(date), 1);
    (next - month_start(date)).num_days() as u32
}
