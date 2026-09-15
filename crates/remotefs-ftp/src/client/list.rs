//! Decoding and validation of `LIST` output shared by both clients.

use std::path::{Component, Path};

use remotefs::File;
use remotefs::fs::{
    FileType, Metadata, RemoteError, RemoteErrorType, RemoteResult, UnixPex, UnixPexClass,
};
use suppaftp::list::{File as FtpFile, ListParser, PosixPexQuery};
use suppaftp::{FtpError, FtpResult};

use crate::utils::path as path_utils;
/// Decodes raw `LIST` bytes without replacing invalid UTF-8.
pub(crate) fn decode_list_bytes(bytes: Vec<u8>) -> FtpResult<Vec<String>> {
    String::from_utf8(bytes)
        .map_err(|_| FtpError::BadResponse)
        .map(|listing| {
            listing
                .lines()
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
}

pub(crate) fn parse_list_lines(path: &Path, lines: Vec<String>) -> RemoteResult<Vec<File>> {
    let entries = lines
        .into_iter()
        .enumerate()
        // Some servers prepend the POSIX block count; it is not a file entry.
        .filter(|(index, line)| {
            !(*index == 0
                && line.strip_prefix("total ").is_some_and(|count| {
                    !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit())
                }))
        })
        .map(|(_, line)| {
            // The generic parser also tries MLSD and accepts arbitrary text as
            // a filename. LIST uses POSIX or DOS entries, not MLSD facts.
            reject_ambiguous_list_name(&line)?;
            let f = parse_list_entry(&line)
                .map_err(|err| RemoteError::with_source(RemoteErrorType::ProtocolError, err))?;
            if matches!(f.name(), "." | "..") {
                return Ok(None);
            }
            validate_list_child_name(f.name())?;
            let file_type = if f.is_symlink() {
                FileType::Symlink
            } else if f.is_directory() {
                FileType::Directory
            } else {
                FileType::File
            };
            let mut metadata = Metadata::default()
                .file_type(file_type)
                .size(f.size() as u64)
                .modified(f.modified())
                .mode(query_unix_pex(&f));
            if let Some(gid) = f.gid() {
                metadata = metadata.gid(gid);
            }
            if let Some(uid) = f.uid() {
                metadata = metadata.uid(uid);
            }
            if let Some(target) = f.symlink() {
                metadata = metadata.symlink(path_utils::absolutize(path, target));
            }
            Ok(Some(File::new(path.join(f.name()), metadata)))
        })
        .collect::<RemoteResult<Vec<_>>>()?;
    Ok(entries.into_iter().flatten().collect())
}

/// Rejects listing rows whose format cannot preserve the exact server name.
fn reject_ambiguous_list_name(line: &str) -> RemoteResult<()> {
    reject_ambiguous_posix_name_boundary(line)?;
    if line.starts_with('l') && line.matches(" -> ").count() > 1 {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a symlink name or target containing multiple ` -> ` separators",
        ));
    }
    if !line.starts_with('-')
        && !line.starts_with('l')
        && !line.starts_with('d')
        && looks_like_dos_listing(line)
    {
        reject_ambiguous_dos_name_boundary(line)?;
    }
    Ok(())
}

/// Rejects POSIX rows whose whitespace boundary could hide a leading-space name.
fn reject_ambiguous_posix_name_boundary(line: &str) -> RemoteResult<()> {
    let is_special = matches!(
        line.as_bytes().first(),
        Some(b'b' | b'c' | b'p' | b's' | b'D')
    );
    let normalized = if is_special {
        normalize_special_posix_line(line)
    } else {
        line.to_owned()
    };
    let spans = field_spans(&normalized);
    let is_posix = matches!(
        line.as_bytes().first(),
        Some(b'-' | b'l' | b'd' | b'b' | b'c' | b'p' | b's' | b'D')
    );
    let has_ambiguous_boundary =
        is_posix && spans.len() >= 9 && spans[7].1.saturating_add(1) < spans[8].0;
    if has_ambiguous_boundary {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a filename with an ambiguous leading-space boundary",
        ));
    }
    if is_posix
        && spans
            .get(7)
            .is_some_and(|(_, end)| contains_unicode_whitespace(&normalized[..*end]))
    {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned Unicode whitespace in its POSIX metadata",
        ));
    }
    let name_suffix = spans.get(7).map_or("", |(_, end)| &normalized[*end..]);
    if is_posix && !name_suffix.is_empty() && name_suffix.chars().all(char::is_whitespace) {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a filename made only of whitespace",
        ));
    }
    if is_posix
        && spans.len() >= 8
        && has_unicode_whitespace_after_ascii_padding(&normalized[spans[7].1..])
    {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a filename with an ambiguous Unicode-whitespace boundary",
        ));
    }
    Ok(())
}

/// Returns whether a row starts with the date/time shape used by DOS `LIST`.
fn looks_like_dos_listing(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() >= 8
        && bytes[2] == b'-'
        && bytes[5] == b'-'
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[6].is_ascii_digit()
        && bytes[7].is_ascii_digit()
}

/// Rejects DOS rows whose metadata separator could hide a leading-space name.
fn reject_ambiguous_dos_name_boundary(line: &str) -> RemoteResult<()> {
    // Windows FTP listings conventionally align names at byte column 39.
    const PADDED_METADATA_COLUMN: usize = 24;
    const PADDED_NAME_COLUMN: usize = 39;

    let Some(time_start) = line[8..]
        .find(|character: char| !character.is_ascii_whitespace())
        .map(|index| index + 8)
    else {
        return Ok(());
    };
    let timestamp = &line[time_start..];
    let timestamp_prefix_end = timestamp
        .char_indices()
        .nth(8)
        .map_or(timestamp.len(), |(index, _)| index);
    let timestamp_prefix = &timestamp[..timestamp_prefix_end];
    let Some(time_end) = ["AM", "PM"].iter().find_map(|marker| {
        timestamp_prefix
            .find(marker)
            .map(|index| time_start + index + marker.len())
    }) else {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned an unsupported DOS timestamp",
        ));
    };
    let rest = &line[time_end..];
    if rest
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace() && !character.is_ascii_whitespace())
    {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a DOS filename with an ambiguous Unicode-whitespace boundary",
        ));
    }
    let Some(metadata_start) = rest.find(|character: char| !character.is_ascii_whitespace()) else {
        return Ok(());
    };
    let metadata = &rest[metadata_start..];
    let metadata_end = metadata
        .find(|character: char| character.is_ascii_whitespace())
        .unwrap_or(metadata.len());
    let metadata_token = &metadata[..metadata_end];
    let after_metadata = &metadata[metadata_end..];
    if after_metadata.chars().any(char::is_whitespace)
        && after_metadata.chars().all(char::is_whitespace)
    {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a filename made only of whitespace",
        ));
    }
    if has_unicode_whitespace_after_ascii_padding(after_metadata) {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a filename with an ambiguous Unicode-whitespace boundary",
        ));
    }
    let separator = after_metadata
        .bytes()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count();
    let metadata_column = time_end + metadata_start;
    let name_start = time_end + metadata_start + metadata_end + separator;
    let is_supported_padded_layout = metadata_token == "<DIR>"
        && metadata_column == PADDED_METADATA_COLUMN
        && name_start == PADDED_NAME_COLUMN;
    if separator > 1 && !is_supported_padded_layout {
        return Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            "LIST returned a DOS filename with an ambiguous leading-space boundary",
        ));
    }
    Ok(())
}

/// Detects a non-ASCII whitespace character after the format's ASCII padding.
fn has_unicode_whitespace_after_ascii_padding(text: &str) -> bool {
    text.trim_start_matches(|character: char| character.is_ascii_whitespace())
        .chars()
        .next()
        .is_some_and(|character| character.is_whitespace())
}

/// Returns whether text contains Unicode whitespace rather than ASCII padding.
fn contains_unicode_whitespace(text: &str) -> bool {
    text.chars()
        .any(|character| character.is_whitespace() && !character.is_ascii_whitespace())
}

/// Parses a POSIX or DOS `LIST` entry, accepting POSIX special files as files.
fn parse_list_entry(line: &str) -> Result<FtpFile, suppaftp::list::ParseError> {
    let is_special = matches!(
        line.as_bytes().first(),
        Some(b'b' | b'c' | b'p' | b's' | b'D')
    );
    if is_special {
        let mut normalized = normalize_special_posix_line(line);
        normalized.replace_range(..1, "-");
        ListParser::parse_posix(&normalized).or_else(|_| ListParser::parse_dos(line))
    } else {
        ListParser::parse_posix(line).or_else(|_| ListParser::parse_dos(line))
    }
}

/// Normalizes device-node major/minor fields to the regular-file size format.
fn normalize_special_posix_line(line: &str) -> String {
    let spans = field_spans(line);
    let is_device = spans.len() > 5
        && line
            .as_bytes()
            .first()
            .is_some_and(|byte| matches!(byte, b'b' | b'c'))
        && line[spans[4].0..spans[4].1].ends_with(',')
        && line[spans[4].0..spans[4].1]
            .trim_end_matches(',')
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        && line[spans[5].0..spans[5].1]
            .bytes()
            .all(|byte| byte.is_ascii_digit());
    if !is_device {
        return line.to_owned();
    }

    let mut normalized = line.to_owned();
    normalized.replace_range(spans[4].0..spans[5].1, "0");
    normalized
}

/// Returns byte ranges for the whitespace-separated fields of a LIST line.
fn field_spans(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, byte) in line.bytes().enumerate() {
        if byte.is_ascii_whitespace() {
            if let Some(start) = start.take() {
                spans.push((start, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(start) = start {
        spans.push((start, line.len()));
    }
    spans
}

/// Ensures a server-supplied listing name cannot escape its listed directory.
pub(crate) fn validate_list_child_name(name: &str) -> RemoteResult<()> {
    let path = Path::new(name);
    let has_path_separator = name
        .bytes()
        .any(|byte| byte == b'/' || (cfg!(target_os = "windows") && byte == b'\\'));
    let is_direct_child = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.bytes().any(|byte| byte.is_ascii_control())
        && !has_path_separator
        && path.components().count() == 1
        && matches!(path.components().next(), Some(Component::Normal(_)));
    if is_direct_child {
        Ok(())
    } else {
        Err(RemoteError::with_message(
            RemoteErrorType::ProtocolError,
            format!("LIST returned an unsafe child name: {name:?}"),
        ))
    }
}

/// Returns unix pex from ftp file pex
fn query_unix_pex(f: &FtpFile) -> UnixPex {
    UnixPex::new(
        UnixPexClass::new(
            f.can_read(PosixPexQuery::Owner),
            f.can_write(PosixPexQuery::Owner),
            f.can_execute(PosixPexQuery::Owner),
        ),
        UnixPexClass::new(
            f.can_read(PosixPexQuery::Group),
            f.can_write(PosixPexQuery::Group),
            f.can_execute(PosixPexQuery::Group),
        ),
        UnixPexClass::new(
            f.can_read(PosixPexQuery::Others),
            f.can_write(PosixPexQuery::Others),
            f.can_execute(PosixPexQuery::Others),
        ),
    )
}
