//! Display formatting. Pure functions. Port of `lib/format.ts`; the tests are
//! `tests/unit/settings/lib.test.ts`.
//!
//! Dates follow the examples in the TS doc comments (`Today 14:32`,
//! `28 Sep 2025 14:02`, `3 October 2026`) rather than the system locale.

use chrono::{Datelike, Local, NaiveDate, TimeZone};

/// `Number.prototype.toFixed`: rounds half away from zero on exact ties.
pub fn to_fixed(value: f64, digits: usize) -> String {
    let scale = 10f64.powi(digits as i32);
    let scaled = value * scale;
    // Only binary-exact halves (1.25, 0.125, …) are ties; Rust would round those to even.
    if scaled.fract().abs() == 0.5 {
        let rounded = scaled.round() / scale;
        return format!("{rounded:.digits$}");
    }
    format!("{value:.digits$}")
}

/// `String(Number(text))` for the short decimals these helpers produce: no trailing zeros.
fn js_number(text: &str) -> String {
    let value: f64 = text.parse().unwrap_or(0.0);
    let mut out = format!("{value}");
    if out == "-0" {
        out = "0".into();
    }
    out
}

/// `Math.round`.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

pub fn format_bytes(bytes: f64) -> String {
    if !bytes.is_finite() || bytes <= 0.0 {
        return "0 MB".into();
    }
    if bytes >= 1e9 {
        return format!("{} GB", to_fixed(bytes / 1e9, 1));
    }
    if bytes >= 1e6 {
        return format!("{} MB", js_round(bytes / 1e6));
    }
    if bytes >= 1e3 {
        return format!("{} KB", js_round(bytes / 1e3));
    }
    format!("{} B", js_round(bytes))
}

/// Device memory, in binary units as Windows reports it: `12 GB`.
pub fn format_memory(bytes: f64) -> String {
    let gb = bytes / 1024f64.powi(3);
    if gb >= 1.0 {
        format!("{} GB", js_number(&to_fixed(gb, 1)))
    } else {
        format!("{} MB", js_round(bytes / 1024f64.powi(2)))
    }
}

pub fn format_speed(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return String::new();
    }
    if bytes_per_second >= 1e6 {
        return format!("{} MB/s", to_fixed(bytes_per_second / 1e6, 1));
    }
    format!("{} KB/s", js_round(bytes_per_second / 1e3).max(1.0))
}

/// A list price: `$0.04`, `$0.111`, `$0.30`, `$1`.
pub fn format_price(usd: f64) -> String {
    if usd == 0.0 {
        return "Free".into();
    }
    if usd.fract() == 0.0 {
        return format!("${usd}");
    }
    let fixed = to_fixed(usd, 3);
    match fixed.strip_suffix('0') {
        Some(short) => format!("${short}"),
        None => format!("${fixed}"),
    }
}

/// The cost of one short request, down to fractions of a cent: `0.011¢`, `1.2¢`, `$0.25`.
pub fn format_cost(usd: f64) -> String {
    if !usd.is_finite() || usd <= 0.0 {
        return "0¢".into();
    }
    let cents = usd * 100.0;
    if cents >= 10.0 {
        return format!("${}", to_fixed(usd, 2));
    }
    if cents >= 1.0 {
        return format!("{}¢", to_fixed(cents, 1));
    }
    // Two significant digits, without exponent notation.
    let digits = (1.0 - cents.log10().floor()).clamp(2.0, 8.0) as usize;
    format!("{}¢", js_number(&to_fixed(cents, digits)))
}

pub fn format_ms(ms: f64) -> String {
    format!("{} ms", group_thousands(js_round(ms) as i64))
}

fn group_thousands(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if value < 0 { format!("-{out}") } else { out }
}

/// Audio or elapsed time: `0.8 s`, `12.4 s`, `1 min 5 s`.
pub fn format_duration(ms: f64) -> String {
    let seconds = ms / 1000.0;
    if seconds < 60.0 {
        return format!("{} s", to_fixed(seconds, 1));
    }
    let minutes = (seconds / 60.0).floor();
    let rest = js_round(seconds - minutes * 60.0);
    if rest != 0.0 {
        format!("{minutes} min {rest} s")
    } else {
        format!("{minutes} min")
    }
}

/// `Today 14:32`, `Yesterday 09:10`, `28 Sep 14:02`, `28 Sep 2025 14:02`.
pub fn format_when(epoch_ms: i64, now_ms: i64) -> String {
    let (Some(when), Some(now)) = (
        Local.timestamp_millis_opt(epoch_ms).single(),
        Local.timestamp_millis_opt(now_ms).single(),
    ) else {
        return String::new();
    };
    let days = (now.date_naive() - when.date_naive()).num_days();
    let time = when.format("%H:%M").to_string();
    if days == 0 {
        return format!("Today {time}");
    }
    if days == 1 {
        return format!("Yesterday {time}");
    }
    if when.year() == now.year() {
        format!("{} {time}", when.format("%-d %b"))
    } else {
        format!("{} {time}", when.format("%-d %b %Y"))
    }
}

/// `2026-10-03` → `3 October 2026`. Falls back to the input when it does not parse.
pub fn format_date(iso: &str) -> String {
    match NaiveDate::parse_from_str(iso, "%Y-%m-%d") {
        Ok(date) => date.format("%-d %B %Y").to_string(),
        Err(_) => iso.to_string(),
    }
}

/// `plural(1, "problem", None)` → `1 problem`; `plural(3, "entry", Some("entries"))` → `3 entries`.
pub fn plural(count: usize, one: &str, many: Option<&str>) -> String {
    if count == 1 {
        format!("{count} {one}")
    } else {
        match many {
            Some(many) => format!("{count} {many}"),
            None => format!("{count} {one}s"),
        }
    }
}

/// Display name for a prompt key; user prompts show their key as written.
pub fn prompt_label(name: &str) -> &str {
    match name {
        "clean" => "Clean",
        "email" => "Email",
        "casual" => "Casual",
        "code_comment" => "Code comment",
        "verbatim" => "Verbatim",
        other => other,
    }
}

/// A prompt key for the config file from a name the user typed: `Stand-up notes` → `stand_up_notes`.
pub fn prompt_key(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

/// Catalog descriptions come without a full stop; add one so sentences can follow.
pub fn sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.ends_with(['.', '!', '?', '…']) {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

/// `errorMessage`: replies already carry their message as a string.
pub fn error_message(err: &str) -> String {
    if err.is_empty() {
        "Something went wrong".into()
    } else {
        err.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes_and_speeds() {
        assert_eq!(format_bytes(475_491_840.0), "475 MB");
        assert_eq!(format_bytes(1_625_935_520.0), "1.6 GB");
        assert_eq!(format_bytes(0.0), "0 MB");
        assert_eq!(format_speed(12_300_000.0), "12.3 MB/s");
        assert_eq!(format_speed(400_000.0), "400 KB/s");
        assert_eq!(format_speed(0.0), "");
        assert_eq!(format_memory(12.0 * 1024f64.powi(3)), "12 GB");
    }

    #[test]
    fn formats_list_prices_as_the_spec_writes_them() {
        assert_eq!(format_price(0.04), "$0.04");
        assert_eq!(format_price(0.111), "$0.111");
        assert_eq!(format_price(0.3), "$0.30");
        assert_eq!(format_price(0.075), "$0.075");
        assert_eq!(format_price(1.0), "$1");
        assert_eq!(format_price(0.0), "Free");
    }

    #[test]
    fn formats_request_costs_down_to_fractions_of_a_cent() {
        assert_eq!(format_cost(0.04 / 360.0), "0.011¢");
        assert_eq!(format_cost(0.000002), "0.0002¢");
        assert_eq!(format_cost(0.012), "1.2¢");
        assert_eq!(format_cost(0.25), "$0.25");
        assert_eq!(format_cost(0.0), "0¢");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(800.0), "0.8 s");
        assert_eq!(format_duration(65_000.0), "1 min 5 s");
        assert_eq!(format_duration(300_000.0), "5 min");
        assert_eq!(format_ms(1234.4), "1,234 ms");
    }

    #[test]
    fn labels_recent_times_relative_to_today() {
        let at = |y, m, d, h, min| {
            Local
                .with_ymd_and_hms(y, m, d, h, min, 0)
                .single()
                .unwrap()
                .timestamp_millis()
        };
        let now = at(2026, 10, 3, 15, 0);
        assert!(format_when(at(2026, 10, 3, 9, 5), now).starts_with("Today "));
        assert!(format_when(at(2026, 10, 2, 23, 50), now).starts_with("Yesterday "));
        let older = format_when(at(2026, 9, 28, 14, 2), now);
        assert!(!older.contains("Today") && !older.contains("Yesterday"));
        assert!(format_when(at(2025, 9, 28, 14, 2), now).contains("2025"));
        assert_eq!(format_date("2026-10-03"), "3 October 2026");
        assert_eq!(format_date("soon"), "soon");
    }

    #[test]
    fn handles_prompt_names_and_small_text_helpers() {
        assert_eq!(prompt_label("code_comment"), "Code comment");
        assert_eq!(prompt_label("standup"), "standup");
        assert_eq!(prompt_key(" Stand-up notes! "), "stand_up_notes");
        assert_eq!(prompt_key("***"), "");
        assert_eq!(plural(1, "problem", None), "1 problem");
        assert_eq!(plural(3, "entry", Some("entries")), "3 entries");
        assert_eq!(sentence("Lowest price"), "Lowest price.");
        assert_eq!(sentence("Done already."), "Done already.");
    }
}
