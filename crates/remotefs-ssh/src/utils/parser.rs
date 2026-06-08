//! ## Parser
//!
//! parser utils

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use chrono::format::ParseError;
use chrono::prelude::*;

/// Convert `ls` syntax time to [`SystemTime`].
///
/// `ls` time has two possible syntaxes:
/// 1. if year is current: `%b %d %H:%M` (e.g. `Nov 5 13:46`)
/// 2. else: `%b %d %Y` (e.g. `Nov 5 2019`)
///
/// **Note:** this parser is best-effort and timezone-ambiguous. `ls` prints
/// times in the server's local timezone without a UTC offset, so the returned
/// [`SystemTime`] is interpreted as if it were UTC. Prefer
/// [`parse_stat_epoch`] (which consumes the timezone-free epoch emitted by
/// `stat`) whenever available; this function is retained only as a fallback
/// for shells/hosts where `stat` is unavailable.
pub fn parse_lstime(tm: &str, fmt_year: &str, fmt_hours: &str) -> Result<SystemTime, ParseError> {
    let datetime: NaiveDateTime = match NaiveDate::parse_from_str(tm, fmt_year) {
        Ok(date) => {
            // Case 2.
            // Return NaiveDateTime from NaiveDate with time 00:00:00
            date.and_hms_opt(0, 0, 0).unwrap()
        }
        Err(_) => {
            // Might be case 1.
            // We need to add Current Year at the end of the string
            let this_year: i32 = Utc::now().year();
            let date_time_str: String = format!("{tm} {this_year}");
            // Now parse
            NaiveDateTime::parse_from_str(
                date_time_str.as_ref(),
                format!("{fmt_hours} %Y").as_ref(),
            )?
        }
    };
    // Convert datetime to system time
    let sys_time: SystemTime = SystemTime::UNIX_EPOCH;
    Ok(sys_time
        .checked_add(Duration::from_secs(datetime.and_utc().timestamp() as u64))
        .unwrap_or(SystemTime::UNIX_EPOCH))
}

/// Parse the output of `stat -c %Y <path>` (GNU) or `stat -f %m <path>` (BSD).
///
/// Both commands emit a single line containing the Unix epoch in seconds.
/// Returns [`None`] if the output cannot be parsed.
pub fn parse_stat_epoch(output: &str) -> Option<SystemTime> {
    let secs: u64 = output.trim().parse().ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

/// Parse the output of `stat -c '%Y %n'` (GNU) or `stat -f '%m %N'` (BSD) for
/// a set of paths. Each line has the shape `<epoch> <name>`.
///
/// Returns a map from basename to [`SystemTime`]. Lines that fail to parse are
/// silently skipped so that a partial result still enriches the caller.
pub fn parse_stat_listing(output: &str) -> HashMap<String, SystemTime> {
    output
        .lines()
        .filter_map(|line| {
            let (epoch, name) = line.trim().split_once(char::is_whitespace)?;
            let secs: u64 = epoch.trim().parse().ok()?;
            let time = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs))?;
            let basename = Path::new(name.trim())
                .file_name()
                .map(|x| x.to_string_lossy().to_string())?;
            Some((basename, time))
        })
        .collect()
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::utils::fmt::fmt_time_utc;

    #[test]
    fn should_parse_lstime() {
        // Good cases
        assert_eq!(
            fmt_time_utc(
                parse_lstime("Nov 5 16:32", "%b %d %Y", "%b %d %H:%M")
                    .ok()
                    .unwrap(),
                "%m %d %M"
            )
            .as_str(),
            "11 05 32"
        );
        assert_eq!(
            fmt_time_utc(
                parse_lstime("Dec 2 21:32", "%b %d %Y", "%b %d %H:%M")
                    .ok()
                    .unwrap(),
                "%m %d %M"
            )
            .as_str(),
            "12 02 32"
        );
        assert_eq!(
            parse_lstime("Nov 5 2018", "%b %d %Y", "%b %d %H:%M")
                .ok()
                .unwrap()
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .unwrap(),
            Duration::from_secs(1541376000)
        );
        assert_eq!(
            parse_lstime("Mar 18 2018", "%b %d %Y", "%b %d %H:%M")
                .ok()
                .unwrap()
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .unwrap(),
            Duration::from_secs(1521331200)
        );
        // bad cases
        assert!(parse_lstime("Oma 31 2018", "%b %d %Y", "%b %d %H:%M").is_err());
        assert!(parse_lstime("Feb 31 2018", "%b %d %Y", "%b %d %H:%M").is_err());
        assert!(parse_lstime("Feb 15 25:32", "%b %d %Y", "%b %d %H:%M").is_err());
    }

    #[test]
    fn should_parse_stat_epoch_gnu() {
        // `stat -c %Y` on Linux emits a bare epoch followed by a newline.
        let parsed = parse_stat_epoch("1704067200\n").expect("valid epoch");
        assert_eq!(
            parsed
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1704067200
        );
    }

    #[test]
    fn should_parse_stat_epoch_bsd() {
        // `stat -f %m` on macOS/BSD emits a bare epoch without trailing
        // whitespace when piped.
        let parsed = parse_stat_epoch("1541376000").expect("valid epoch");
        assert_eq!(
            parsed
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1541376000
        );
    }

    #[test]
    fn should_fail_to_parse_stat_epoch_on_error() {
        // When stat fails (e.g. command not found or non-zero exit), the
        // output is empty or contains an error message; parsing must yield
        // `None` so callers can fall back.
        assert!(parse_stat_epoch("").is_none());
        assert!(parse_stat_epoch("stat: cannot stat 'x': No such file").is_none());
    }

    #[test]
    fn should_parse_stat_listing_gnu() {
        let output = "1704067200 /tmp/a.txt\n1541376000 /tmp/b.txt\n";
        let map = parse_stat_listing(output);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("a.txt")
                .unwrap()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1704067200
        );
        assert_eq!(
            map.get("b.txt")
                .unwrap()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1541376000
        );
    }

    #[test]
    fn should_parse_stat_listing_bsd() {
        // BSD `stat -f '%m %N'` for bare names still emits the same shape.
        let output = "1704067200 a.txt\n1541376000 b.txt\n";
        let map = parse_stat_listing(output);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("a.txt")
                .unwrap()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1704067200
        );
    }

    #[test]
    fn should_skip_unparseable_stat_listing_lines() {
        let output =
            "1704067200 /tmp/a.txt\nstat: cannot stat '/tmp/missing'\n1541376000 /tmp/b.txt\n";
        let map = parse_stat_listing(output);
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("a.txt"));
        assert!(map.contains_key("b.txt"));
    }

    #[test]
    fn should_return_empty_stat_listing_on_total_failure() {
        assert!(parse_stat_listing("").is_empty());
    }
}
