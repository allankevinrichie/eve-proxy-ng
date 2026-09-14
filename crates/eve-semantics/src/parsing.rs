//! Game text parsing: locale-tolerant numbers, distances, tab-separated
//! row cells, markup stripping.
//!
//! The reference fixtures span German/French/Scandinavian locales
//! (2020–2025); the China clients add CJK text around the numbers. All
//! digit-group separators are treated uniformly with the rule "a final
//! group shorter than three digits is a truncated fraction".

/// Digit group separators seen across client locales.
const GROUP_SEPARATORS: [char; 6] = [',', '.', '’', '\'', ' ', '\u{00A0}'];
/// Narrow no-break space, used by some locales.
const NARROW_NBSP: char = '\u{202F}';

/// Parse a number, dropping a trailing fraction (the last group shorter
/// than three digits when preceded by a separator).
///
/// `"2,856"` → 2856, `"6.621"` → 6621, `"3.444.555,6"` → 3444555.
pub fn parse_number_truncating_fraction(text: &str) -> Option<i64> {
    let text = text.trim().replace(NARROW_NBSP, " ");
    let mut groups: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut saw_separator = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if GROUP_SEPARATORS.contains(&ch) {
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
                saw_separator = true;
            }
        } else if ch == '-' && current.is_empty() && groups.is_empty() {
            current.push(ch);
        } else if saw_separator {
            // First non-digit non-separator ends the number.
            break;
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    if groups.is_empty() {
        return None;
    }
    // A single group is a plain integer regardless of trailing separators
    // (`"16 km"`); the fraction rule only applies to multi-group numbers.
    if groups.len() == 1 {
        return groups.remove(0).parse::<i64>().ok();
    }
    // Drop the fraction: last group shorter than 3 digits.
    let (digits, _fraction) = match groups.split_last() {
        Some((last, head)) if last.len() < 3 => (head, Some(last)),
        _ => (groups.as_slice(), None),
    };
    let joined: String = digits.concat();
    if joined.is_empty() || joined == "-" {
        return None;
    }
    joined.parse::<i64>().ok()
}

/// Parse a number as `f64`, recognizing the single-decimal form
/// (`"2.6"`, `"6,5"`) in addition to grouped thousands.
pub fn parse_number_f64(text: &str) -> Option<f64> {
    let text = text.trim().replace(NARROW_NBSP, " ");
    let mut groups: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if GROUP_SEPARATORS.contains(&ch) {
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
        } else {
            break;
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    // Single decimal: one separator, short fraction, short integer part.
    if groups.len() == 2 && groups[1].len() <= 2 && groups[0].len() <= 3 {
        let integer: f64 = groups[0].parse().ok()?;
        let fraction: f64 = format!("0.{}", groups[1]).parse().ok()?;
        return Some(integer + fraction);
    }
    let integer = parse_number_truncating_fraction(text.trim())?;
    Some(integer as f64)
}

/// Parse an overview distance text into meters.
///
/// `"2,856 m"` → 2856, `"16 km"` → 16000, `"2 980 m"` → 2980,
/// `"2.6 AU"` → 388952463820, `" 3.444.555,6 m "` → 3444555.
pub fn parse_distance_meters(text: &str) -> Option<i64> {
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    let unit = tokens.pop()?;
    // Everything before the unit is the number (space-separated thousands
    // are part of it).
    let number: String = tokens.concat();
    if number.is_empty() {
        return None;
    }
    let multiplier = match unit {
        "m" => 1.0,
        "km" => 1_000.0,
        "AU" | "au" => 149_597_870_700.0,
        _ => return None,
    };
    let value = parse_number_f64(&number)?;
    let meters = value * multiplier;
    if meters.abs() < 9.2e18 {
        Some(meters.round() as i64)
    } else {
        None
    }
}

/// Split a tab-tag row (`"Name<t><right>200<t>..."`) into cell texts with
/// markup stripped.
pub fn split_row_cells(text: &str) -> Vec<String> {
    text.split("<t>")
        .map(strip_markup)
        .map(|cell| cell.trim().to_string())
        .filter(|cell| !cell.is_empty())
        .collect()
}

/// Remove simple `<tag>`/`</tag>` wrappers and decode the entities the
/// client emits (`&nbsp;` becomes a space, not noise).
pub fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let Some(close) = rest[open..].find('>') else {
            break;
        };
        out.push_str(&rest[..open]);
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'");
    decoded.trim().to_string()
}

/// Parse a capacity gauge text such as `"1,211.9/5,000.0 m³"` or
/// `"(33.3) 53.6/450.0 m³"` into `(used, total)` (fractions truncated).
pub fn parse_capacity_gauge(text: &str) -> Option<(i64, i64)> {
    // Drop a leading "(33.3)" drone-bay-style prefix.
    let body = match text.find('(') {
        Some(open) if text[open..].starts_with('(') => {
            let close = text[open..].find(')')? + open;
            &text[close + 1..]
        }
        _ => text,
    };
    let body = body.trim();
    let mut parts = body.split('/');
    let used = parse_number_truncating_fraction(parts.next()?)?;
    let total = parse_number_truncating_fraction(parts.next()?)?;
    Some((used, total))
}

/// Parse an in-game clock text (`"14:07"`) into minutes since midnight.
pub fn parse_clock_minutes(text: &str) -> Option<i64> {
    let mut parts = text.trim().split(':');
    let hours: i64 = parts.next()?.trim().parse().ok()?;
    let minutes: i64 = parts.next()?.trim().parse().ok()?;
    if (0..24).contains(&hours) && (0..60).contains(&minutes) {
        Some(hours * 60 + minutes)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_fixtures_from_reference_tests() {
        assert_eq!(parse_distance_meters("2,856 m"), Some(2_856));
        assert_eq!(parse_distance_meters("16 km"), Some(16_000));
        assert_eq!(parse_distance_meters("6.621 m"), Some(6_621));
        assert_eq!(parse_distance_meters("2 980 m"), Some(2_980));
        assert_eq!(parse_distance_meters(" 3.444.555,6 m "), Some(3_444_555));
        assert_eq!(parse_distance_meters("5\u{00A0}000,0 m"), Some(5_000));
        assert_eq!(parse_distance_meters("3 m"), Some(3));
        assert_eq!(parse_distance_meters("nothing"), None);
    }

    #[test]
    fn plain_integers_stay_exact() {
        assert_eq!(parse_number_truncating_fraction("42"), Some(42));
        assert_eq!(parse_number_truncating_fraction("-7"), Some(-7));
        assert_eq!(parse_number_truncating_fraction("12,5"), Some(12));
    }

    #[test]
    fn capacity_gauge_fixtures() {
        assert_eq!(parse_capacity_gauge("1,211.9/5,000.0 m³"), Some((1_211, 5_000)));
        assert_eq!(parse_capacity_gauge("(33.3) 53.6/450.0 m³"), Some((53, 450)));
        assert_eq!(parse_capacity_gauge("0/5\u{00A0}000,0 m³"), Some((0, 5_000)));
        assert_eq!(parse_capacity_gauge("0/5'000.0 m³"), Some((0, 5_000)));
    }

    #[test]
    fn row_cell_splitting() {
        let cells = split_row_cells(
            "Condensed Scordite<t><right>200<t>Scordite<t><t><t><right>30 m3<t><right>2.290,00 ISK",
        );
        assert_eq!(
            cells,
            vec!["Condensed Scordite", "200", "Scordite", "30 m3", "2.290,00 ISK"]
        );
    }

    #[test]
    fn markup_stripping() {
        assert_eq!(strip_markup("<right>200"), "200");
        assert_eq!(strip_markup("a &amp; b&nbsp;c"), "a & b c");
    }

    #[test]
    fn clock_parsing() {
        assert_eq!(parse_clock_minutes("14:07"), Some(847));
        assert_eq!(parse_clock_minutes("25:00"), None);
    }
}
