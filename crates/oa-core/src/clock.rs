//! 时间语义集中在这里，避免再依赖 SQLite 的 `date()/strftime()`：
//! - 业务日期（考勤、合同到期、报表月份）用**本地时区**，符合办公场景直觉；
//! - 会话/审计时间戳用 **UTC**，与 SQLite `CURRENT_TIMESTAMP` 和 Postgres 的
//!   `to_char(now() AT TIME ZONE 'UTC', ...)` 保持同一种字符串格式，跨库可直接比较。

use chrono::{Datelike, Duration, Local, Utc};

const TS_FMT: &str = "%Y-%m-%d %H:%M:%S";
const DATE_FMT: &str = "%Y-%m-%d";

/// UTC 时间戳（会话有效期、审计排序）
pub fn utc_now() -> String {
    Utc::now().format(TS_FMT).to_string()
}

pub fn utc_after_days(days: i64) -> String {
    (Utc::now() + Duration::days(days)).format(TS_FMT).to_string()
}

/// 本地业务日期
pub fn local_today() -> String {
    Local::now().format(DATE_FMT).to_string()
}

pub fn date_offset(days: i64) -> String {
    (Local::now() + Duration::days(days)).format(DATE_FMT).to_string()
}

pub fn local_year() -> i64 {
    Local::now().year() as i64
}

pub fn local_hhmm() -> String {
    Local::now().format("%H:%M").to_string()
}

pub fn local_month() -> String {
    Local::now().format("%Y-%m").to_string()
}

pub fn local_datetime() -> String {
    Local::now().format(TS_FMT).to_string()
}

/// 近 n 个月的 `YYYY-MM` 标签（升序，含当月）
pub fn month_labels(n: usize) -> Vec<String> {
    let now = Local::now();
    let mut year = now.year();
    let mut month = now.month() as i32;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(format!("{year:04}-{month:02}"));
        month -= 1;
        if month == 0 {
            month = 12;
            year -= 1;
        }
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_labels_are_sorted_and_contiguous() {
        let labels = month_labels(6);
        assert_eq!(labels.len(), 6);
        assert!(labels.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(labels[5], local_month());
    }

    #[test]
    fn timestamps_have_sqlite_compatible_format() {
        let now = utc_now();
        assert_eq!(now.len(), 19, "{now}");
        assert_eq!(&now[4..5], "-");
        assert_eq!(&now[10..11], " ");
    }
}
