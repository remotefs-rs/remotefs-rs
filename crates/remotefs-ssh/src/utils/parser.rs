//! ## Parser
//!
//! parser utils

use std::time::{Duration, SystemTime};

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
use chrono::format::ParseError;
use chrono::prelude::*;

/// Convert ls syntax time to System Time
/// ls time has two possible syntax:
/// 1. if year is current: %b %d %H:%M (e.g. Nov 5 13:46)
/// 2. else: %b %d %Y (e.g. Nov 5 2019)
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
        .checked_add(Duration::from_secs(datetime.timestamp() as u64))
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
        // bad cases
        assert!(parse_lstime("Oma 31 2018", "%b %d %Y", "%b %d %H:%M").is_err());
        assert!(parse_lstime("Feb 31 2018", "%b %d %Y", "%b %d %H:%M").is_err());
        assert!(parse_lstime("Feb 15 25:32", "%b %d %Y", "%b %d %H:%M").is_err());
    }
}
