//! ## fmt
//!
//! format utilities

use std::time::SystemTime;

use jiff::Timestamp;
use jiff::tz::TimeZone;

/// Format time using fmt string in utc time.
///
/// Times outside the range jiff supports show its earliest or latest timestamp.
pub fn fmt_time_utc(time: SystemTime, fmt: &str) -> String {
    let timestamp = Timestamp::try_from(time).unwrap_or_else(|_| {
        if time > SystemTime::UNIX_EPOCH {
            Timestamp::MAX
        } else {
            Timestamp::MIN
        }
    });
    format!("{}", timestamp.to_zoned(TimeZone::UTC).strftime(fmt))
}

#[cfg(test)]
mod test {

    use std::time::Duration;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_fmt_time() {
        let system_time: SystemTime = SystemTime::UNIX_EPOCH;
        assert_eq!(
            fmt_time_utc(system_time, "%Y-%m-%d %H:%M"),
            String::from("1970-01-01 00:00")
        );
        // times past the latest supported date show that date instead of panicking
        let far_future = SystemTime::UNIX_EPOCH + Duration::from_secs(u64::MAX / 4);
        assert_eq!(
            fmt_time_utc(far_future, "%Y-%m-%d %H:%M"),
            String::from("9999-12-30 22:00")
        );
        // times before the earliest supported date show that date instead of panicking
        let far_past = SystemTime::UNIX_EPOCH - Duration::from_secs(u64::MAX / 4);
        assert_eq!(fmt_time_utc(far_past, "%Y"), String::from("-9999"));
    }
}
