// SPDX-License-Identifier: MIT
//! Small pieces of Python's standard library that the installer's behaviour depends on.
use crate::error::{Error, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// A fixed regular expression of this program.
pub fn pattern(text: &str) -> regex::Regex {
    regex::Regex::new(text).unwrap_or_else(|error| unreachable!("{text}: {error}"))
}

/// `str.isspace()` for one character.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// `str.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `str.split()` without arguments.
pub fn split(text: &str) -> Vec<&str> {
    text.split(is_space)
        .filter(|part| !part.is_empty())
        .collect()
}

/// `str.splitlines()`.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            lines.push(&text[start..index]);
            start = index + c.len_utf8();
            if c == '\r'
                && let Some((next, '\n')) = chars.peek().copied()
            {
                chars.next();
                start = next + 1;
            }
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// `bytes.decode()`: strict UTF-8 with Python's wording of the failure.
pub fn decode(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).map_err(|error| {
        let position = error.utf8_error().valid_up_to();
        let bytes = error.as_bytes();
        let byte = bytes.get(position).copied().unwrap_or(0);
        let reason = match error.utf8_error().error_len() {
            None => "unexpected end of data",
            Some(_) if matches!(byte, 0xc2..=0xf4) => "invalid continuation byte",
            Some(_) => "invalid start byte",
        };
        Error::Value(format!(
            "'utf-8' codec can't decode byte 0x{byte:02x} in position {position}: {reason}"
        ))
    })
}

/// `int(text)` for the decimal texts the installer converts.
pub fn int(text: &str) -> Result<i64> {
    let invalid = || {
        Error::Value(format!(
            "invalid literal for int() with base 10: {}",
            crate::error::repr(text)
        ))
    };
    let digits = strip(text);
    let (negative, digits) = match digits.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, digits.strip_prefix('+').unwrap_or(digits)),
    };
    // Python allows single underscores between digits.
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return Err(invalid());
    }
    let plain: String = digits.chars().filter(|c| *c != '_').collect();
    if !plain.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    // Python's integers are unbounded; far out of range is all a caller needs to know.
    let value: i64 = plain.parse().unwrap_or(i64::MAX);
    Ok(if negative { -value } else { value })
}

/// Broken-down UTC time from seconds since the epoch.
pub struct Utc {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Monday is 0.
    pub weekday: u32,
}

impl Utc {
    pub fn now() -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        Self::from_epoch(i64::try_from(seconds).unwrap_or(0))
    }

    pub fn from_epoch(seconds: i64) -> Self {
        let days = seconds.div_euclid(86_400);
        let rest = seconds.rem_euclid(86_400);
        // Howard Hinnant's civil-from-days.
        let shifted = days + 719_468;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_index + 2) / 5 + 1;
        let month = if month_index < 10 {
            month_index + 3
        } else {
            month_index - 9
        };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        Self {
            year,
            month: month as u32,
            day: day as u32,
            hour: (rest / 3600) as u32,
            minute: (rest % 3600 / 60) as u32,
            second: (rest % 60) as u32,
            weekday: (days + 3).rem_euclid(7) as u32,
        }
    }

    /// `strftime("%Y%m%dT%H%M%S")`.
    pub fn stamp(&self) -> String {
        format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// The date format of HTTP headers.
    pub fn http_date(&self) -> String {
        const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            DAYS[self.weekday as usize % 7],
            self.day,
            MONTHS[(self.month as usize + 11) % 12],
            self.year,
            self.hour,
            self.minute,
            self.second
        )
    }
}

/// Random bytes from the kernel, as `os.urandom`.
pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    let mut filled = 0;
    while filled < N {
        filled +=
            rustix::rand::getrandom(&mut bytes[filled..], rustix::rand::GetRandomFlags::empty())?;
    }
    Ok(bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `uuid.uuid4().hex`.
pub fn uuid4_hex() -> Result<String> {
    let mut bytes = random_bytes::<16>()?;
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    Ok(hex(&bytes))
}

/// `secrets.token_urlsafe(24)`: 24 random bytes as unpadded URL-safe Base64.
pub fn token_urlsafe() -> Result<String> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = random_bytes::<24>()?;
    let mut out = String::with_capacity(32);
    for chunk in bytes.chunks(3) {
        let group = u32::from(chunk[0]) << 16 | u32::from(chunk[1]) << 8 | u32::from(chunk[2]);
        for shift in [18, 12, 6, 0] {
            out.push(char::from(ALPHABET[(group >> shift) as usize & 63]));
        }
    }
    Ok(out)
}

/// The random part of a `tempfile` name.
pub fn temp_suffix() -> Result<String> {
    const ALPHABET: &[u8; 37] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let bytes = random_bytes::<8>()?;
    Ok(bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn text_helpers_follow_python() {
        assert_eq!(strip(" \x1c a b\u{a0}\n"), "a b");
        assert_eq!(split(" GET  /x\tHTTP/1.1\r\n"), ["GET", "/x", "HTTP/1.1"]);
        assert_eq!(splitlines("a\r\nb\rc\n\nd"), ["a", "b", "c", "", "d"]);
        assert_eq!(splitlines("a\n"), ["a"]);
        assert_eq!(int(" +1_0 ").unwrap(), 10);
        assert_eq!(
            int("x").unwrap_err().to_string(),
            "invalid literal for int() with base 10: 'x'"
        );
        assert_eq!(
            decode(vec![b'a', 0xff]).unwrap_err().to_string(),
            "'utf-8' codec can't decode byte 0xff in position 1: invalid start byte"
        );
        assert_eq!(
            decode(vec![0xc3, b'(']).unwrap_err().to_string(),
            "'utf-8' codec can't decode byte 0xc3 in position 0: invalid continuation byte"
        );
        assert_eq!(
            decode(vec![0xe2, 0x82]).unwrap_err().to_string(),
            "'utf-8' codec can't decode byte 0xe2 in position 0: unexpected end of data"
        );
    }

    #[test]
    fn time_and_random_formats() {
        let time = Utc::from_epoch(1_791_837_045);
        assert_eq!(time.stamp(), "20261012T203045");
        assert_eq!(time.http_date(), "Mon, 12 Oct 2026 20:30:45 GMT");
        assert_eq!(Utc::from_epoch(951_782_400).stamp(), "20000229T000000");
        let id = uuid4_hex().unwrap();
        assert_eq!((id.len(), &id[12..13]), (32, "4"));
        assert!(matches!(&id[16..17], "8" | "9" | "a" | "b"));
        assert_eq!(token_urlsafe().unwrap().len(), 32);
        assert_eq!(temp_suffix().unwrap().len(), 8);
    }
}
