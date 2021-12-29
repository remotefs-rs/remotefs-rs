//! ## Parser
//!
//! parser utils

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
use std::time::{Duration, SystemTime};

/// ### parse_datetime
///
/// Parse date time string representation and transform it into `SystemTime`
pub fn parse_datetime(tm: &str, fmt: &str) -> Result<SystemTime, ParseError> {
    match NaiveDateTime::parse_from_str(tm, fmt) {
        Ok(dt) => {
            let sys_time: SystemTime = SystemTime::UNIX_EPOCH;
            Ok(sys_time
                .checked_add(Duration::from_secs(dt.timestamp() as u64))
                .unwrap_or(SystemTime::UNIX_EPOCH))
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod test {

    use super::*;

    use pretty_assertions::assert_eq;

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
