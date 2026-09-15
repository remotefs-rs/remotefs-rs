//! Shell command builders and output parsers shared by the SCP clients.
//!
//! Everything here is pure: it builds the command strings the SCP clients run
//! over an SSH exec channel and parses their output. No I/O happens here so
//! the blocking and asynchronous SCP clients share one implementation.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use lazy_regex::{Lazy, Regex};
use remotefs::File;
use remotefs::fs::{FileType, Metadata, SetMetadata, UnixPex, UnixPexClass};

use crate::utils::{fmt as fmt_utils, parser as parser_utils};

/// NOTE: about this damn regex <https://stackoverflow.com/questions/32480890/is-there-a-regex-to-parse-the-values-from-an-ftp-directory-listing>
static LS_RE: Lazy<Regex> = lazy_regex!(
    r#"^(?<sym_dir>[\-ld])(?<pex>[\-rwxsStT]{9})(?<sec_ctx>\.|\+|\@)?\s+(?<n_links>\d+)\s+(?<uid>.+)\s+(?<gid>.+)\s+(?<size>\d+)\s+(?<date_time>\w{3}\s+\d{1,2}\s+(?:\d{1,2}:\d{1,2}|\d{4}))\s+(?<name>.+)$"#
);

/// Which `stat(1)` format flags the remote host accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatFlavor {
    /// GNU coreutils: `stat -c '<fmt>'`.
    Gnu,
    /// BSD / macOS: `stat -f '<fmt>'`.
    Bsd,
    /// Neither flavor is available; fall back to parsing `ls` output.
    Unsupported,
}

/// Default mode used when `create_dir` receives no mode.
pub(crate) const DEFAULT_DIR_MODE: u32 = 0o755;
/// Default mode used when `create` receives no mode.
pub(crate) const DEFAULT_FILE_MODE: u32 = 0o644;

/// Quotes a path for a POSIX shell command line.
pub(crate) fn quote(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\'', "'\\''");
    format!("'{path}'")
}

/// Command that succeeds only on hosts with GNU `stat`.
pub(crate) const STAT_PROBE_GNU: &str = "stat --version >/dev/null 2>&1";
/// Command that succeeds only on hosts with BSD `stat`.
pub(crate) const STAT_PROBE_BSD: &str = "stat -f %m / >/dev/null 2>&1";

/// Resolves the stat flavor from the two probe results.
pub(crate) fn stat_flavor(gnu_ok: bool, bsd_ok: bool) -> StatFlavor {
    if gnu_ok {
        StatFlavor::Gnu
    } else if bsd_ok {
        StatFlavor::Bsd
    } else {
        StatFlavor::Unsupported
    }
}

/// `stat` command printing the Unix mtime of one path, if the flavor allows it.
pub(crate) fn mtime_command(flavor: StatFlavor, path: &Path) -> Option<String> {
    let flag = match flavor {
        StatFlavor::Gnu => "-c %Y",
        StatFlavor::Bsd => "-f %m",
        StatFlavor::Unsupported => return None,
    };
    Some(format!("stat {flag} {path}", path = quote(path)))
}

/// Batched `stat` command printing `<mtime> <name>` per entry of `dir`.
pub(crate) fn mtimes_command(flavor: StatFlavor, dir: &Path, names: &[String]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let fmt = match flavor {
        StatFlavor::Gnu => "-c '%Y %n'",
        StatFlavor::Bsd => "-f '%m %N'",
        StatFlavor::Unsupported => return None,
    };
    let args = names
        .iter()
        .map(|name| quote(&dir.join(name)))
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!("stat {fmt} {args}"))
}

/// Parses the batched `stat` output into a basename to mtime map.
pub(crate) fn parse_mtimes(output: &str) -> HashMap<String, SystemTime> {
    parser_utils::parse_stat_listing(output)
}

/// Parses a single `stat` epoch output.
pub(crate) fn parse_mtime(output: &str) -> Option<SystemTime> {
    parser_utils::parse_stat_epoch(output)
}

/// `ls -la` listing command for a directory.
pub(crate) fn list_command(dir: &Path) -> String {
    let path = format!("{dir}/", dir = dir.display());
    format!("unset LANG; ls -la {}", quote(Path::new(&path)))
}

/// `ls -l` / `ls -ld` command used by `stat`.
pub(crate) fn stat_command(path: &Path, is_dir: bool) -> String {
    if is_dir {
        format!("ls -ld {path}", path = quote(path))
    } else {
        format!("ls -l {path}", path = quote(path))
    }
}

/// `test -d` command.
pub(crate) fn is_dir_command(path: &Path) -> String {
    format!("test -d {path}", path = quote(path))
}

/// `test -e` command.
pub(crate) fn exists_command(path: &Path) -> String {
    format!("test -e {path} || test -L {path}", path = quote(path))
}

/// `rm -f` command.
pub(crate) fn remove_file_command(path: &Path) -> String {
    format!("rm -f {path}", path = quote(path))
}

/// `rmdir` command.
pub(crate) fn remove_dir_command(path: &Path) -> String {
    format!("rmdir {path}", path = quote(path))
}

/// `rm -rf` command.
pub(crate) fn remove_dir_all_command(path: &Path) -> String {
    format!("rm -rf {path}", path = quote(path))
}

/// `mkdir -m` command.
pub(crate) fn mkdir_command(path: &Path, mode: Option<UnixPex>) -> String {
    let mode = mode.map(u32::from).unwrap_or(DEFAULT_DIR_MODE);
    format!("mkdir -m {mode:o} {path}", path = quote(path))
}

/// `ln -s` command creating `path` pointing at `target`.
pub(crate) fn symlink_command(path: &Path, target: &Path) -> String {
    format!(
        "ln -s {target} {path}",
        target = quote(target),
        path = quote(path)
    )
}

/// `cp -rf` command.
pub(crate) fn copy_command(src: &Path, dest: &Path) -> String {
    format!("cp -rf {src} {dest}", src = quote(src), dest = quote(dest))
}

/// `mv -f` command.
pub(crate) fn rename_command(src: &Path, dest: &Path) -> String {
    format!("mv -f {src} {dest}", src = quote(src), dest = quote(dest))
}

/// Commands applying the requested metadata changes, in order.
pub(crate) fn set_metadata_commands(path: &Path, metadata: &SetMetadata) -> Vec<String> {
    let path = quote(path);
    let mut commands = Vec::new();
    if let Some(mode) = metadata.mode {
        commands.push(format!("chmod {mode:o} {path}", mode = u32::from(mode)));
    }
    if let Some(uid) = metadata.uid {
        let gid = metadata
            .gid
            .map(|gid| format!(":{gid}"))
            .unwrap_or_default();
        commands.push(format!("chown {uid}{gid} {path}"));
    }
    if let Some(accessed) = metadata.accessed {
        commands.push(format!(
            "touch -a -t {time} {path}",
            time = fmt_utils::fmt_time_utc(accessed, "%Y%m%d%H%M.%S")
        ));
    }
    if let Some(modified) = metadata.modified {
        commands.push(format!(
            "touch -m -t {time} {path}",
            time = fmt_utils::fmt_time_utc(modified, "%Y%m%d%H%M.%S")
        ));
    }
    commands
}

/// Parses a whole `ls -la` listing of `dir`, dropping `.`/`..` and special files.
pub(crate) fn parse_listing(dir: &Path, output: &str) -> Vec<File> {
    output
        .lines()
        .filter_map(|line| parse_ls_line(dir, line))
        .collect()
}

/// Parses one `ls -l` line into a [`File`] located under `dir`.
pub(crate) fn parse_ls_line(dir: &Path, line: &str) -> Option<File> {
    trace!("Parsing LS line: '{line}'");
    let captures = LS_RE.captures(line)?;
    if captures.len() < 8 {
        return None;
    }
    let (is_dir, is_symlink) = match &captures["sym_dir"] {
        "-" => (false, false),
        "l" => (false, true),
        "d" => (true, false),
        _ => return None,
    };
    if captures["pex"].len() < 9 {
        return None;
    }
    let pex = |range: Range<usize>| {
        let mut count: u8 = 0;
        for (i, c) in captures["pex"][range].chars().enumerate() {
            if c != '-' {
                count += match i {
                    0 => 4,
                    1 => 2,
                    2 => 1,
                    _ => 0,
                };
            }
        }
        count
    };
    let mode = UnixPex::new(
        UnixPexClass::from(pex(0..3)),
        UnixPexClass::from(pex(3..6)),
        UnixPexClass::from(pex(6..9)),
    );
    let modified = parser_utils::parse_lstime(&captures["date_time"], "%b %d %Y", "%b %d %H:%M")
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let uid = captures["uid"].parse::<u32>().ok();
    let gid = captures["gid"].parse::<u32>().ok();
    let size = captures["size"].parse::<u64>().unwrap_or(0);
    let (file_name, symlink) = if is_symlink {
        name_and_link(&captures["name"])
    } else {
        (String::from(&captures["name"]), None)
    };
    let file_name = PathBuf::from(&file_name)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or(file_name);
    if file_name == "." || file_name == ".." {
        return None;
    }
    let path = dir.join(&file_name);
    let file_type = if symlink.is_some() {
        FileType::Symlink
    } else if is_dir {
        FileType::Directory
    } else {
        FileType::File
    };
    let mut metadata = Metadata::default()
        .file_type(file_type)
        .mode(mode)
        .modified(modified)
        .size(size);
    if let Some(uid) = uid {
        metadata = metadata.uid(uid);
    }
    if let Some(gid) = gid {
        metadata = metadata.gid(gid);
    }
    if let Some(symlink) = symlink {
        metadata = metadata.symlink(symlink);
    }
    trace!(
        "Found entry at {path} with metadata {metadata:?}",
        path = path.display()
    );
    Some(File::new(path, metadata))
}

/// Splits a `name -> target` token into the name and the link target.
fn name_and_link(token: &str) -> (String, Option<PathBuf>) {
    let mut tokens = token.splitn(2, " -> ");
    let name = tokens.next().unwrap_or_default().to_string();
    let symlink = tokens.next().map(PathBuf::from);
    (name, symlink)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_parse_file_line() {
        let file = parse_ls_line(
            Path::new("/tmp"),
            "-rw-r--r--  1 1000 1000 12 Sep 10 10:00 hello.txt",
        )
        .unwrap();
        assert_eq!(file.path(), Path::new("/tmp/hello.txt"));
        assert!(file.is_file());
        assert_eq!(file.metadata().size, Some(12));
        assert_eq!(file.metadata().uid, Some(1000));
        assert_eq!(u32::from(file.metadata().mode.unwrap()), 0o644);
    }

    #[test]
    fn should_parse_symlink_and_directory_lines() {
        let link = parse_ls_line(
            Path::new("/tmp"),
            "lrwxrwxrwx  1 root root 5 Sep 10 2025 link -> /etc/hosts",
        )
        .unwrap();
        assert!(link.is_symlink());
        assert_eq!(
            link.metadata().symlink.as_deref(),
            Some(Path::new("/etc/hosts"))
        );
        let dir = parse_ls_line(
            Path::new("/"),
            "drwxr-xr-x 2 root root 4096 Sep 10 2025 etc",
        )
        .unwrap();
        assert!(dir.is_dir());
        assert_eq!(dir.path(), Path::new("/etc"));
    }

    #[test]
    fn should_skip_dot_entries_and_special_files() {
        assert!(
            parse_ls_line(Path::new("/"), "drwxr-xr-x 2 root root 4096 Sep 10 2025 .").is_none()
        );
        assert!(
            parse_ls_line(
                Path::new("/"),
                "crw-rw-rw- 1 root root 1, 3 Sep 10 2025 null"
            )
            .is_none()
        );
        assert!(parse_ls_line(Path::new("/"), "total 12").is_none());
    }

    #[test]
    fn should_build_commands_with_quoted_paths() {
        let p = Path::new("/tmp/a b");
        assert_eq!(
            exists_command(p),
            "test -e '/tmp/a b' || test -L '/tmp/a b'"
        );
        assert_eq!(mkdir_command(p, None), "mkdir -m 755 '/tmp/a b'");
        assert_eq!(
            mkdir_command(p, Some(UnixPex::from(0o700))),
            "mkdir -m 700 '/tmp/a b'"
        );
        assert_eq!(list_command(p), "unset LANG; ls -la '/tmp/a b/'");
        assert_eq!(stat_command(p, true), "ls -ld '/tmp/a b'");
        assert_eq!(
            symlink_command(Path::new("/l"), Path::new("/t")),
            "ln -s '/t' '/l'"
        );
        assert_eq!(
            rename_command(Path::new("/a"), Path::new("/b")),
            "mv -f '/a' '/b'"
        );
        assert_eq!(
            exists_command(Path::new("/tmp/$(touch /tmp/pwned)")),
            "test -e '/tmp/$(touch /tmp/pwned)' || test -L '/tmp/$(touch /tmp/pwned)'"
        );
        assert_eq!(
            exists_command(Path::new("/tmp/it's")),
            "test -e '/tmp/it'\\''s' || test -L '/tmp/it'\\''s'"
        );
    }

    #[test]
    fn should_build_stat_commands_per_flavor() {
        assert_eq!(stat_flavor(true, false), StatFlavor::Gnu);
        assert_eq!(stat_flavor(false, true), StatFlavor::Bsd);
        assert_eq!(stat_flavor(false, false), StatFlavor::Unsupported);
        assert_eq!(
            mtime_command(StatFlavor::Gnu, Path::new("/a")),
            Some("stat -c %Y '/a'".into())
        );
        assert_eq!(
            mtime_command(StatFlavor::Unsupported, Path::new("/a")),
            None
        );
        assert_eq!(
            mtimes_command(StatFlavor::Bsd, Path::new("/d"), &["x".into(), "y".into()]),
            Some("stat -f '%m %N' '/d/x' '/d/y'".into())
        );
        assert_eq!(mtimes_command(StatFlavor::Gnu, Path::new("/d"), &[]), None);
    }

    #[test]
    fn should_build_set_metadata_commands_in_order() {
        let metadata = SetMetadata::default()
            .mode(UnixPex::from(0o600))
            .uid(1)
            .gid(2)
            .modified(SystemTime::UNIX_EPOCH);
        let commands = set_metadata_commands(Path::new("/f"), &metadata);
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0], "chmod 600 '/f'");
        assert_eq!(commands[1], "chown 1:2 '/f'");
        assert!(commands[2].starts_with("touch -m -t 197001010000.00 "));
    }
}
