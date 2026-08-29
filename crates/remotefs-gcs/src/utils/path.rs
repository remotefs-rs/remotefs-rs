//! ## Path
//!
//! path utilities

use std::path::{Component, Path, PathBuf};

/// Converts a path to a slash-separated string for object APIs.
pub fn slash(path: &Path) -> String {
    #[cfg(target_os = "windows")]
    {
        path_slash::PathExt::to_slash_lossy(path).into_owned()
    }

    #[cfg(not(target_os = "windows"))]
    {
        path.to_string_lossy().replace('\\', "/")
    }
}

/// Normalizes an absolute path and rejects traversal above its root.
pub fn normalize(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) => return None,
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::Normal(component) => normalized.push(component),
        }
    }
    Some(normalized)
}

/// Absolutize target path if relative.
pub fn absolutize(wrkdir: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        let mut path = wrkdir.to_path_buf();
        path.push(target);
        path
    }
}

/// This function will get the difference from path `path` to `base`. Basically will remove `base` from `path`
/// This function has been written by <https://github.com/Manishearth>
/// and is licensed under the APACHE-2/MIT license <https://github.com/Manishearth/pathdiff>
pub fn diff_paths<P, B>(path: P, base: B) -> Option<PathBuf>
where
    P: AsRef<Path>,
    B: AsRef<Path>,
{
    let path = path.as_ref();
    let base = base.as_ref();

    if path.is_absolute() == base.is_absolute() {
        let mut ita = path.components();
        let mut itb = base.components();
        let mut comps = vec![];
        loop {
            match (ita.next(), itb.next()) {
                (None, None) => break,
                (Some(a), None) => {
                    comps.push(a);
                    comps.extend(ita.by_ref());
                    break;
                }
                (None, _) => comps.push(Component::ParentDir),
                (Some(a), Some(b)) if comps.is_empty() && a == b => (),
                (Some(a), Some(Component::CurDir)) => comps.push(a),
                (Some(_), Some(Component::ParentDir)) => return None,
                (Some(a), Some(_)) => {
                    comps.push(Component::ParentDir);
                    for _ in itb {
                        comps.push(Component::ParentDir);
                    }
                    comps.push(a);
                    comps.extend(ita.by_ref());
                    break;
                }
            }
        }
        Some(
            comps
                .iter()
                .map(|component| component.as_os_str())
                .collect(),
        )
    } else {
        path.is_absolute().then(|| PathBuf::from(path))
    }
}

#[cfg(test)]
mod test {

    use super::*;

    #[test]
    fn absolutize_path() {
        assert_eq!(
            absolutize(Path::new("/home/omar"), Path::new("readme.txt")).as_path(),
            Path::new("/home/omar/readme.txt")
        );
        assert_eq!(
            absolutize(Path::new("/home/omar"), Path::new("/tmp/readme.txt")).as_path(),
            Path::new("/tmp/readme.txt")
        );
    }

    #[test]
    fn calc_diff_paths() {
        assert_eq!(
            diff_paths(Path::new("/foo/bar"), Path::new("/"))
                .unwrap()
                .as_path(),
            Path::new("foo/bar")
        );
        assert_eq!(
            diff_paths(Path::new("/foo/bar"), Path::new("/foo"))
                .unwrap()
                .as_path(),
            Path::new("bar")
        );
        assert_eq!(
            diff_paths(Path::new("/foo/bar/chiedo.gif"), Path::new("/"))
                .unwrap()
                .as_path(),
            Path::new("foo/bar/chiedo.gif")
        );
    }

    #[test]
    fn normalizes_paths_without_leaving_root() {
        assert_eq!(
            normalize(Path::new("/docs/../readme")),
            Some(PathBuf::from("/readme"))
        );
        assert_eq!(normalize(Path::new("/../../secret")), None);
    }

    #[test]
    fn converts_backslashes_to_object_separators() {
        assert_eq!(slash(Path::new("docs\\readme.md")), "docs/readme.md");
    }
}
