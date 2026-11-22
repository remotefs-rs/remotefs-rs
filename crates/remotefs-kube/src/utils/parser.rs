//! ## Parser
//!
//! parser utils

use std::time::{Duration, SystemTime};

use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;

/// Convert ls syntax time to System Time
/// ls time has two possible syntax:
/// 1. if year is current: %b %d %H:%M (e.g. Nov 5 13:46)
/// 2. else: %b %d %Y (e.g. Nov 5 2019)
pub fn parse_lstime(tm: &str, fmt_year: &str, fmt_hours: &str) -> Result<SystemTime, jiff::Error> {
    let datetime: DateTime = match Date::strptime(fmt_year, tm) {
        Ok(date) => {
            // Case 2.
            // Return DateTime from Date with time 00:00:00
            date.at(0, 0, 0, 0)
        }
        Err(_) => {
            // Might be case 1.
            // We need to add Current Year at the end of the string
            let this_year = Timestamp::now().to_zoned(TimeZone::UTC).year();
            let date_time_str = format!("{tm} {this_year}");
            // Now parse
            DateTime::strptime(format!("{fmt_hours} %Y"), date_time_str)?
        }
    };
    // Convert datetime to system time; dates before 1970 become the Unix epoch
    let secs = datetime.to_zoned(TimeZone::UTC)?.timestamp().as_second();
    Ok(u64::try_from(secs)
        .ok()
        .and_then(|secs| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs)))
        .unwrap_or(SystemTime::UNIX_EPOCH))
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
        // padded days, single-digit hours and minutes
        assert_eq!(
            parse_lstime("Nov  5 2018", "%b %d %Y", "%b %d %H:%M")
                .ok()
                .unwrap()
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .unwrap(),
            Duration::from_secs(1541376000)
        );
        assert_eq!(
            fmt_time_utc(
                parse_lstime("Nov  5 6:2", "%b %d %Y", "%b %d %H:%M")
                    .ok()
                    .unwrap(),
                "%m %d %H %M"
            )
            .as_str(),
            "11 05 06 02"
        );
        // dates before 1970 become the Unix epoch
        assert_eq!(
            parse_lstime("Dec 31 1969", "%b %d %Y", "%b %d %H:%M")
                .ok()
                .unwrap(),
            SystemTime::UNIX_EPOCH
        );
        // bad cases
        assert!(parse_lstime("Oma 31 2018", "%b %d %Y", "%b %d %H:%M").is_err());
        assert!(parse_lstime("Feb 31 2018", "%b %d %Y", "%b %d %H:%M").is_err());
        assert!(parse_lstime("Feb 15 25:32", "%b %d %Y", "%b %d %H:%M").is_err());
    }
}
