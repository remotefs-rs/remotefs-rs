//! ## Parser
//!
//! parser utils

use std::time::{Duration, SystemTime};

use chrono::format::ParseError;
use chrono::prelude::*;

/// ### parse_datetime
///
/// Parse date time string representation and transform it into `SystemTime`
pub fn parse_datetime(tm: &str, fmt: &str) -> Result<SystemTime, ParseError> {
    match NaiveDateTime::parse_from_str(tm, fmt) {
        Ok(dt) => {
            let sys_time: SystemTime = SystemTime::UNIX_EPOCH;
            Ok(sys_time
                .checked_add(Duration::from_secs(dt.and_utc().timestamp() as u64))
                .unwrap_or(SystemTime::UNIX_EPOCH))
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_parse_datetime() {
        assert_eq!(
            parse_datetime("04-08-14  03:09PM", "%d-%m-%y %I:%M%p")
                .ok()
                .unwrap()
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .unwrap(),
            Duration::from_secs(1407164940)
        );
        // Not enough argument for datetime
        assert!(parse_datetime("04-08-14", "%d-%m-%y").is_err());
    }
}
