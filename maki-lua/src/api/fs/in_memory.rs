use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::path::{Component, Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, UNIX_EPOCH};

use globset::{Glob, GlobMatcher};
use maki_agent::tools::grep::GrepParams;
use maki_agent::{GrepFileEntry, GrepLine, GrepMatchGroup};
use regex::Regex;

use super::{
    DIR_MISSING_PREFIX, DIR_NOT_A_DIR_PREFIX, FsBackend, FsError, FsMeta, TYPE_DIRECTORY, TYPE_FILE,
};

/// Hermetic test backend: a deliberately dumb `BTreeMap` model. No symlinks,
/// no gitignore, no permissions; file mtime is an insertion counter
/// (deterministic, no sleeps or time reads). Behavior tests must not rely on
/// real-FS vagaries — those belong in the `RealFs` suite.
///
/// Deliberate grep/glob divergences from the real backend — in-memory
/// behavior tests must not assert the real shapes:
/// - grep skips lines over `max_line_bytes` instead of truncating and
///   keeping them, emits one group per match (no merging of adjacent
///   matches within the context window), matches per line only (no multiline
///   patterns), skips non-UTF-8 files (real: byte search, NUL quit), keeps
///   `\r` (real strips it), and never excludes `.git`.
/// - glob/grep ignore the `gitignore` flag (gitignored files surface) and
///   match patterns with plain globset: slash-less patterns agree with
///   gitignore (match at any depth), but slash-containing patterns are not
///   anchored to the search root and negation patterns are unsupported.
pub struct InMemoryFs {
    inner: RwLock<Inner>,
}

struct Inner {
    entries: BTreeMap<PathBuf, Entry>,
    seq: u64,
}

enum Entry {
    File(Vec<u8>, u64),
    Dir,
}

impl InMemoryFs {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                entries: BTreeMap::new(),
                seq: 0,
            }),
        }
    }

    /// All file entries as `(path, content)` pairs, for test assertions.
    pub fn files(&self) -> Vec<(PathBuf, Vec<u8>)> {
        self.inner
            .read()
            .unwrap()
            .entries
            .iter()
            .filter_map(|(p, e)| match e {
                Entry::File(bytes, _) => Some((p.clone(), bytes.clone())),
                Entry::Dir => None,
            })
            .collect()
    }

    fn bump_seq(inner: &mut Inner) -> u64 {
        inner.seq += 1;
        inner.seq
    }
}

/// The filesystem root (`/`, `C:\`) is implicit: it never needs an entry.
fn is_fs_root(p: &Path) -> bool {
    p.parent().is_none()
}

fn parent_exists(inner: &Inner, path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return true;
    };
    if is_fs_root(parent) {
        return true;
    }
    matches!(inner.entries.get(parent), Some(Entry::Dir))
}

fn create_dir(inner: &mut Inner, path: &Path) -> IoResult<()> {
    if inner.entries.contains_key(path) {
        return Err(IoError::from(ErrorKind::AlreadyExists));
    }
    if !parent_exists(inner, path) {
        return Err(IoError::from(ErrorKind::NotFound));
    }
    inner.entries.insert(path.to_path_buf(), Entry::Dir);
    Ok(())
}

fn create_dir_all(inner: &mut Inner, path: &Path) -> IoResult<()> {
    let mut current = PathBuf::new();
    for comp in path.components() {
        current.push(comp);
        match comp {
            Component::RootDir | Component::Prefix(_) => continue,
            _ => {}
        }
        match inner.entries.get(&current) {
            Some(Entry::Dir) => {}
            Some(Entry::File(..)) => return Err(IoError::from(ErrorKind::AlreadyExists)),
            None => {
                inner.entries.insert(current.clone(), Entry::Dir);
            }
        }
    }
    Ok(())
}

/// Root for glob/grep: the `path` option (tilde-expanded, cwd-joined when
/// relative), or the cwd.
fn resolve_root(path: Option<&str>) -> Result<PathBuf, FsError> {
    match path {
        Some(p) => {
            let expanded = super::expand_tilde(p);
            if expanded.is_absolute() {
                Ok(expanded)
            } else {
                Ok(std::env::current_dir().map_err(FsError::Io)?.join(expanded))
            }
        }
        None => std::env::current_dir().map_err(FsError::Io),
    }
}

impl FsBackend for InMemoryFs {
    fn read(&self, path: &Path) -> IoResult<String> {
        let inner = self.inner.read().unwrap();
        match inner.entries.get(path) {
            Some(Entry::File(bytes, _)) => String::from_utf8(bytes.clone())
                .map_err(|_| IoError::new(ErrorKind::InvalidData, "non-utf8 content")),
            Some(Entry::Dir) => Err(IoError::new(ErrorKind::IsADirectory, "is a directory")),
            None => Err(IoError::from(ErrorKind::NotFound)),
        }
    }

    fn read_bytes(&self, path: &Path) -> IoResult<Vec<u8>> {
        let inner = self.inner.read().unwrap();
        match inner.entries.get(path) {
            Some(Entry::File(bytes, _)) => Ok(bytes.clone()),
            Some(Entry::Dir) => Err(IoError::new(ErrorKind::IsADirectory, "is a directory")),
            None => Err(IoError::from(ErrorKind::NotFound)),
        }
    }

    fn stat(&self, path: &Path) -> IoResult<FsMeta> {
        let inner = self.inner.read().unwrap();
        match inner.entries.get(path) {
            Some(Entry::File(bytes, seq)) => Ok(FsMeta {
                size: bytes.len() as u64,
                is_file: true,
                is_dir: false,
                mtime: Some(UNIX_EPOCH + Duration::from_secs(*seq)),
            }),
            Some(Entry::Dir) => Ok(FsMeta {
                size: 0,
                is_file: false,
                is_dir: true,
                mtime: None,
            }),
            None => Err(IoError::from(ErrorKind::NotFound)),
        }
    }

    fn write(&self, path: &Path, content: &[u8]) -> IoResult<()> {
        let mut inner = self.inner.write().unwrap();
        if matches!(inner.entries.get(path), Some(Entry::Dir)) {
            return Err(IoError::new(ErrorKind::IsADirectory, "is a directory"));
        }
        if !parent_exists(&inner, path) {
            return Err(IoError::from(ErrorKind::NotFound));
        }
        let seq = Self::bump_seq(&mut inner);
        inner
            .entries
            .insert(path.to_path_buf(), Entry::File(content.to_vec(), seq));
        Ok(())
    }

    /// Same contract as `write` here: no temp file, no rename.
    fn atomic_write(&self, path: &Path, content: &[u8]) -> IoResult<()> {
        self.write(path, content)
    }

    fn rm(&self, path: &Path, recursive: bool, force: bool) -> IoResult<()> {
        let mut inner = self.inner.write().unwrap();
        let is_dir = match inner.entries.get(path) {
            Some(Entry::Dir) => true,
            Some(Entry::File(..)) => false,
            None => {
                if force {
                    return Ok(());
                }
                return Err(IoError::from(ErrorKind::NotFound));
            }
        };
        if is_dir {
            if !recursive
                && inner
                    .entries
                    .keys()
                    .any(|p| p != path && p.starts_with(path))
            {
                return Err(IoError::other("rm: directory not empty"));
            }
            inner
                .entries
                .retain(|p, _| p != path && !p.starts_with(path));
        } else {
            inner.entries.remove(path);
        }
        Ok(())
    }

    fn mkdir(&self, path: &Path, parents: bool) -> IoResult<()> {
        let mut inner = self.inner.write().unwrap();
        if parents {
            create_dir_all(&mut inner, path)
        } else {
            create_dir(&mut inner, path)
        }
    }

    fn dir(&self, path: &Path, max_depth: u32) -> Result<Vec<(String, &'static str)>, FsError> {
        let meta = match self.stat(path) {
            Ok(m) => m,
            Err(_) => {
                return Err(FsError::Message(format!(
                    "{DIR_MISSING_PREFIX}{}",
                    path.display()
                )));
            }
        };
        if !meta.is_dir {
            return Err(FsError::Message(format!(
                "{DIR_NOT_A_DIR_PREFIX}{}",
                path.display()
            )));
        }
        let inner = self.inner.read().unwrap();
        let mut out = Vec::new();
        for (p, entry) in &inner.entries {
            let Some(rel) = p
                .strip_prefix(path)
                .ok()
                .filter(|r| !r.as_os_str().is_empty())
            else {
                continue;
            };
            if rel.components().count() as u32 > max_depth {
                continue;
            }
            let typ = match entry {
                Entry::File(..) => TYPE_FILE,
                Entry::Dir => TYPE_DIRECTORY,
            };
            out.push((rel.to_string_lossy().into_owned(), typ));
        }
        Ok(out)
    }

    fn glob(
        &self,
        patterns: &[String],
        path: Option<&str>,
        limit: Option<usize>,
        _gitignore: bool,
        sort_mtime: bool,
    ) -> Result<Vec<String>, FsError> {
        let compiled: Vec<GlobMatcher> = patterns
            .iter()
            .map(|p| {
                Glob::new(p)
                    .map(|g| g.compile_matcher())
                    .map_err(|e| FsError::Message(format!("invalid glob pattern: {e}")))
            })
            .collect::<Result<_, _>>()?;
        let root = resolve_root(path)?;
        let inner = self.inner.read().unwrap();
        let mut hits: Vec<(u64, String)> = Vec::new();
        for (p, entry) in &inner.entries {
            let Entry::File(_, seq) = entry else {
                continue;
            };
            let Some(rel) = p
                .strip_prefix(&root)
                .ok()
                .filter(|r| !r.as_os_str().is_empty())
            else {
                continue;
            };
            if compiled.iter().any(|g| g.is_match(rel)) {
                hits.push((*seq, p.to_string_lossy().into_owned()));
            }
        }
        if sort_mtime {
            hits.sort_by_key(|h| Reverse(h.0));
        }
        if let Some(lim) = limit {
            hits.truncate(lim);
        }
        Ok(hits.into_iter().map(|(_, p)| p).collect())
    }

    fn grep(&self, params: GrepParams) -> Result<(PathBuf, Vec<GrepFileEntry>), FsError> {
        let root = resolve_root(params.path.as_deref())?;
        let re = Regex::new(&params.pattern)
            .map_err(|e| FsError::Message(format!("invalid regex pattern: {e}")))?;
        let include = params
            .include
            .as_deref()
            .map(|p| Glob::new(p).map(|g| g.compile_matcher()))
            .transpose()
            .map_err(|e| FsError::Message(format!("invalid glob pattern: {e}")))?;

        let inner = self.inner.read().unwrap();
        let mut files: Vec<(u64, &PathBuf)> = inner
            .entries
            .iter()
            .filter_map(|(p, e)| match e {
                Entry::File(_, seq) => Some((*seq, p)),
                Entry::Dir => None,
            })
            .filter(|(_, p)| p.starts_with(&root))
            .collect();
        files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));

        let mut total_groups = 0;
        let mut out: Vec<GrepFileEntry> = Vec::new();
        for (_, file) in files {
            if total_groups >= params.limit {
                break;
            }
            let Entry::File(bytes, _) = &inner.entries[file] else {
                continue;
            };
            let Ok(content) = String::from_utf8(bytes.clone()) else {
                continue;
            };
            let rel = file.strip_prefix(&root).unwrap_or(file);
            if let Some(g) = &include
                && !g.is_match(rel)
            {
                continue;
            }
            let lines: Vec<&str> = content.split('\n').collect();
            let mut groups: Vec<GrepMatchGroup> = Vec::new();
            for (idx, line) in lines.iter().enumerate() {
                if line.len() > params.max_line_bytes || !re.is_match(line) {
                    continue;
                }
                let mut group_lines: Vec<GrepLine> = Vec::new();
                for (i, &text) in lines
                    .iter()
                    .enumerate()
                    .take(idx)
                    .skip(idx.saturating_sub(params.context_before))
                {
                    if text.len() <= params.max_line_bytes {
                        group_lines.push(GrepLine::context(i + 1, text));
                    }
                }
                group_lines.push(GrepLine::matched(idx + 1, *line));
                for (i, &text) in lines
                    .iter()
                    .enumerate()
                    .skip(idx + 1)
                    .take(params.context_after)
                {
                    if text.len() <= params.max_line_bytes {
                        group_lines.push(GrepLine::context(i + 1, text));
                    }
                }
                groups.push(GrepMatchGroup { lines: group_lines });
                total_groups += 1;
                if total_groups >= params.limit {
                    break;
                }
            }
            if !groups.is_empty() {
                out.push(GrepFileEntry {
                    path: rel.to_string_lossy().into_owned(),
                    groups,
                });
            }
        }
        Ok((root, out))
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::path::Path;

    use super::*;

    const ROOT: &str = "/t";

    fn seeded() -> InMemoryFs {
        let fs = InMemoryFs::new();
        fs.mkdir(Path::new(ROOT), true).unwrap();
        fs.write(Path::new("/t/a.rs"), b"fn a()").unwrap();
        fs.write(Path::new("/t/b.txt"), b"hello").unwrap();
        fs
    }

    #[test]
    fn write_read_roundtrip_and_stat_fields() {
        let fs = InMemoryFs::new();
        fs.mkdir(Path::new(ROOT), true).unwrap();
        fs.write(Path::new("/t/note.md"), b"hello-maki").unwrap();

        assert_eq!(fs.read(Path::new("/t/note.md")).unwrap(), "hello-maki");
        assert_eq!(
            fs.read_bytes(Path::new("/t/note.md")).unwrap(),
            b"hello-maki"
        );

        let meta = fs.stat(Path::new("/t/note.md")).unwrap();
        assert!(meta.is_file);
        assert!(!meta.is_dir);
        assert_eq!(meta.size, 10);
        assert!(meta.mtime.is_some(), "file mtime must be present");

        let dir_meta = fs.stat(Path::new(ROOT)).unwrap();
        assert!(dir_meta.is_dir);
        assert!(!dir_meta.is_file);
        assert_eq!(dir_meta.size, 0);
        assert!(dir_meta.mtime.is_none());
    }

    #[test]
    fn error_kinds_match_real_fs_contract() {
        let fs = seeded();

        assert_eq!(
            fs.read(Path::new("/t/missing")).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        assert_eq!(
            fs.read(Path::new(ROOT)).unwrap_err().kind(),
            ErrorKind::IsADirectory
        );

        fs.write(Path::new("/t/bin.dat"), &[0xff, 0xfe]).unwrap();
        assert_eq!(
            fs.read(Path::new("/t/bin.dat")).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        assert_eq!(
            fs.read_bytes(Path::new("/t/bin.dat")).unwrap(),
            vec![0xff, 0xfe]
        );

        assert_eq!(
            fs.write(Path::new(ROOT), b"x").unwrap_err().kind(),
            ErrorKind::IsADirectory
        );
        assert_eq!(
            fs.write(Path::new("/t/noparent/x.txt"), b"x")
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );
        assert_eq!(
            fs.stat(Path::new("/t/missing")).unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn mkdir_existing_fails_and_parents_creates_chain() {
        let fs = InMemoryFs::new();
        fs.mkdir(Path::new(ROOT), true).unwrap();
        assert_eq!(
            fs.mkdir(Path::new(ROOT), false).unwrap_err().kind(),
            ErrorKind::AlreadyExists
        );
        // `parents = true` is a no-op on an existing dir, like `create_dir_all`.
        fs.mkdir(Path::new(ROOT), true).unwrap();

        let deep = Path::new("/t/x/y/z");
        fs.mkdir(deep, true).unwrap();
        assert!(matches!(fs.stat(deep), Ok(m) if m.is_dir));
        assert!(matches!(fs.stat(Path::new("/t/x/y")), Ok(m) if m.is_dir));

        assert_eq!(
            fs.mkdir(Path::new("/t/p/q"), false).unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn rm_edges() {
        let fs = InMemoryFs::new();
        fs.mkdir(Path::new("/t/tree/nested"), true).unwrap();
        fs.write(Path::new("/t/tree/a.txt"), b"a").unwrap();
        fs.write(Path::new("/t/tree/nested/b.txt"), b"b").unwrap();

        assert_eq!(
            fs.rm(Path::new("/t/tree"), false, false)
                .unwrap_err()
                .kind(),
            ErrorKind::Other
        );
        assert!(fs.stat(Path::new("/t/tree")).is_ok());

        assert!(fs.rm(Path::new("/t/ghost"), false, true).is_ok());
        assert_eq!(
            fs.rm(Path::new("/t/ghost"), false, false)
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );

        fs.rm(Path::new("/t/tree"), true, false).unwrap();
        assert!(fs.stat(Path::new("/t/tree")).is_err());
        assert!(fs.stat(Path::new("/t/tree/a.txt")).is_err());
        assert!(fs.stat(Path::new("/t/tree/nested/b.txt")).is_err());

        fs.write(Path::new("/t/solo.txt"), b"x").unwrap();
        fs.rm(Path::new("/t/solo.txt"), false, false).unwrap();
        assert!(fs.stat(Path::new("/t/solo.txt")).is_err());

        fs.mkdir(Path::new("/t/empty"), false).unwrap();
        fs.rm(Path::new("/t/empty"), false, false).unwrap();
        assert!(fs.stat(Path::new("/t/empty")).is_err());
    }

    #[test]
    fn dir_recursive_lists_relative_names_and_types() {
        let fs = InMemoryFs::new();
        fs.mkdir(Path::new("/t/x"), true).unwrap();
        fs.write(Path::new("/t/x/file.txt"), b"x").unwrap();
        fs.mkdir(Path::new("/t/y"), false).unwrap();

        let entries = fs.dir(Path::new("/t"), 2).unwrap();
        let mut names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["x", "x/file.txt", "y"]);
        let types: std::collections::HashMap<&str, &str> =
            entries.iter().map(|(n, t)| (n.as_str(), *t)).collect();
        assert_eq!(types["x"], TYPE_DIRECTORY);
        assert_eq!(types["x/file.txt"], TYPE_FILE);
        assert_eq!(types["y"], TYPE_DIRECTORY);

        let shallow = fs.dir(Path::new("/t"), 1).unwrap();
        let shallow_names: Vec<&str> = shallow.iter().map(|(n, _)| n.as_str()).collect();
        assert!(shallow_names.contains(&"x"));
        assert!(
            !shallow_names.iter().any(|n| n.contains('/')),
            "depth 1 must not recurse"
        );
    }

    #[test]
    fn dir_missing_and_not_a_directory_messages() {
        let fs = seeded();

        let missing = fs.dir(Path::new("/t/nope"), 1).unwrap_err().to_string();
        assert!(missing.starts_with(DIR_MISSING_PREFIX), "{missing}");
        let not_dir = fs.dir(Path::new("/t/a.rs"), 1).unwrap_err().to_string();
        assert!(not_dir.starts_with(DIR_NOT_A_DIR_PREFIX), "{not_dir}");
    }

    #[test]
    fn glob_scope_limit_and_mtime_order() {
        let fs = InMemoryFs::new();
        fs.mkdir(Path::new("/t/sub"), true).unwrap();
        fs.write(Path::new("/t/top.rs"), b"").unwrap();
        fs.write(Path::new("/t/sub/inner.rs"), b"").unwrap();

        // gitignore semantics (real backend delegates to the `ignore` crate):
        // a pattern without a slash matches at any depth under the scope.
        let shallow = fs
            .glob(&["*.rs".into()], Some("/t"), None, true, false)
            .unwrap();
        let mut shallow: Vec<&str> = shallow.iter().map(String::as_str).collect();
        shallow.sort();
        assert_eq!(shallow, vec!["/t/sub/inner.rs", "/t/top.rs"]);

        let scoped = fs
            .glob(&["*.rs".into()], Some("/t/sub"), None, true, false)
            .unwrap();
        assert_eq!(scoped, vec!["/t/sub/inner.rs"]);

        let deep = fs
            .glob(&["**/*.rs".into()], Some("/t"), None, true, false)
            .unwrap();
        let mut deep: Vec<&str> = deep.iter().map(String::as_str).collect();
        deep.sort();
        assert_eq!(deep, vec!["/t/sub/inner.rs", "/t/top.rs"]);

        let limited = fs
            .glob(&["**/*.rs".into()], Some("/t"), Some(1), true, false)
            .unwrap();
        assert_eq!(limited.len(), 1);

        // mtime sort is insertion order, newest first.
        let f = InMemoryFs::new();
        f.mkdir(Path::new(ROOT), true).unwrap();
        f.write(Path::new("/t/one.rs"), b"").unwrap();
        f.write(Path::new("/t/two.rs"), b"").unwrap();
        f.write(Path::new("/t/three.rs"), b"").unwrap();
        let by_mtime = f
            .glob(&["*.rs".into()], Some("/t"), None, true, true)
            .unwrap();
        assert_eq!(by_mtime, vec!["/t/three.rs", "/t/two.rs", "/t/one.rs"]);

        assert!(
            f.glob(&["[".into()], Some("/t"), None, true, false)
                .is_err(),
            "invalid pattern must surface as an error"
        );
    }

    #[test]
    fn grep_matches_with_context_include_and_limit() {
        let fs = InMemoryFs::new();
        fs.mkdir(Path::new(ROOT), true).unwrap();
        let mut content = String::new();
        for i in 1..=5 {
            content.push_str(&format!("line_{i}\n"));
        }
        fs.write(Path::new("/t/data.rs"), content.as_bytes())
            .unwrap();
        fs.write(Path::new("/t/other.txt"), b"line_99\n").unwrap();

        let mut params = GrepParams::new("line_3".to_string());
        params.path = Some("/t".to_string());
        params.context_before = 1;
        params.context_after = 1;
        params.include = Some("*.rs".to_string());
        let (base, entries) = fs.grep(params).unwrap();
        assert_eq!(base, PathBuf::from("/t"));
        assert_eq!(entries.len(), 1, "include must exclude other.txt");
        assert_eq!(entries[0].path, "data.rs");
        assert_eq!(entries[0].groups.len(), 1);
        let lines = &entries[0].groups[0].lines;
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].line_nr, 2);
        assert!(!lines[0].is_match);
        assert_eq!(lines[0].text, "line_2");
        assert_eq!(lines[1].line_nr, 3);
        assert!(lines[1].is_match);
        assert_eq!(lines[2].line_nr, 4);
        assert!(!lines[2].is_match);

        // Without the include filter both files hit.
        let mut params = GrepParams::new("line_".to_string());
        params.path = Some("/t".to_string());
        let (_, all) = fs.grep(params).unwrap();
        assert_eq!(all.len(), 2);

        // The limit caps total groups across files.
        let mut params = GrepParams::new("line_".to_string());
        params.path = Some("/t".to_string());
        params.include = Some("*.rs".to_string());
        params.limit = 2;
        let (_, capped) = fs.grep(params).unwrap();
        assert_eq!(capped[0].groups.len(), 2);

        let mut params = GrepParams::new("[".to_string());
        params.path = Some("/t".to_string());
        let err = fs.grep(params).unwrap_err().to_string();
        assert!(err.starts_with("invalid regex pattern"), "{err}");
    }

    #[test]
    fn files_lists_all_file_entries() {
        let fs = seeded();
        let mut files = fs.files();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, PathBuf::from("/t/a.rs"));
        assert_eq!(files[0].1, b"fn a()");
        assert_eq!(files[1].0, PathBuf::from("/t/b.txt"));
        assert_eq!(files[1].1, b"hello");
    }
}
