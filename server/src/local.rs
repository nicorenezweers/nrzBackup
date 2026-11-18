//! Direct access to umbrelOS folders without any token: the umbrelOS package mounts
//! `${UMBREL_ROOT}/home` as `<root>/Home` and `${UMBREL_ROOT}/app-data` as `<root>/Apps`
//! (read-only). Folders are packed on the fly into a `.tar.gz` stream.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use axum::body::Bytes;
use flate2::Compression;
use flate2::write::GzEncoder;
use futures_util::Stream;
use tokio::sync::mpsc;

/// Virtual top-level folders that can be mounted.
pub const ROOTS: [&str; 2] = ["Home", "Apps"];

fn base() -> PathBuf {
    PathBuf::from(
        std::env::var("NRZBACKUP_UMBREL_ROOT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/umbrel".into()),
    )
}

/// Top-level folders that are mounted (e.g. `["Home", "Apps"]`).
pub fn mounted() -> Vec<&'static str> {
    let b = base();
    ROOTS.into_iter().filter(|r| b.join(r).is_dir()).collect()
}

pub fn available() -> bool {
    !mounted().is_empty()
}

/// Local path of a normalised virtual path such as `/Home/Documents`, when it is mounted.
pub fn resolve(virt: &str) -> Option<PathBuf> {
    let mut parts = virt.split('/').filter(|p| !p.is_empty());
    let top = parts.next()?;
    if !ROOTS.contains(&top) {
        return None;
    }
    let root = base().join(top);
    if !root.is_dir() {
        return None;
    }
    let mut p = root.clone();
    for seg in parts {
        if seg == ".." || seg == "." {
            return None;
        }
        p.push(seg);
    }
    // symlinks must not lead out of the mounted folder
    if let (Ok(real), Ok(real_root)) = (p.canonicalize(), root.canonicalize())
        && !real.starts_with(&real_root)
    {
        return None;
    }
    Some(p)
}

/// Entries of a directory: (name, is_dir, size, modified ms). Unreadable entries are skipped.
pub fn list(dir: &Path) -> io::Result<Vec<(String, bool, Option<u64>, Option<i64>)>> {
    let mut out = vec![];
    for e in fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(meta) = fs::symlink_metadata(e.path()) else {
            continue;
        };
        let is_dir = meta.is_dir();
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        out.push((name, is_dir, (!is_dir).then_some(meta.len()), modified));
    }
    Ok(out)
}

/// `name:` and `version:` from an `umbrel-app.yml` (top-level keys only).
pub fn app_manifest(dir: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(dir.join("umbrel-app.yml")).ok()?;
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}:")))
            .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
            .unwrap_or_default()
    };
    Some((field("name"), field("version")))
}

/// Result of packing a folder.
#[derive(Debug, Default)]
pub struct PackStats {
    pub files: u64,
    pub skipped: Vec<String>,
}

/// Packs `dir` as `<top>/…` into a gzip tar and streams it in chunks. `skip` (and everything
/// below it) is left out. The stats arrive on the returned channel after the last chunk.
pub fn pack(
    dir: PathBuf,
    top: String,
    skip: Option<PathBuf>,
) -> (
    impl Stream<Item = Result<Bytes, io::Error>> + Unpin + Send + 'static,
    tokio::sync::oneshot::Receiver<io::Result<PackStats>>,
) {
    let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(8);
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    tokio::task::spawn_blocking(move || {
        let w = ChanWriter {
            tx: tx.clone(),
            buf: Vec::with_capacity(CHUNK),
        };
        let res = write_tar(&dir, &top, skip.as_deref(), w);
        if let Err(e) = &res {
            let _ = tx.blocking_send(Err(io::Error::new(e.kind(), e.to_string())));
        }
        let _ = done_tx.send(res);
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    (Box::pin(stream), done_rx)
}

const CHUNK: usize = 1 << 20;

struct ChanWriter {
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
    buf: Vec<u8>,
}

impl ChanWriter {
    fn send(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = Bytes::from(std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK)));
        self.tx
            .blocking_send(Ok(chunk))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Empfänger abgebrochen"))
    }
}

impl Write for ChanWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHUNK {
            self.send()?;
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.send()
    }
}

/// Reads exactly `len` bytes: a file that shrank meanwhile is padded with zeros, one that grew
/// is cut, so the archive always stays valid.
struct Exact<R> {
    inner: R,
    left: u64,
    eof: bool,
}

impl<R: Read> Read for Exact<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let max = out.len().min(self.left as usize);
        let n = if self.eof {
            out[..max].fill(0);
            max
        } else {
            match self.inner.read(&mut out[..max])? {
                0 => {
                    self.eof = true;
                    out[..max].fill(0);
                    max
                }
                n => n,
            }
        };
        self.left -= n as u64;
        Ok(n)
    }
}

fn write_tar(dir: &Path, top: &str, skip: Option<&Path>, w: ChanWriter) -> io::Result<PackStats> {
    let gz = GzEncoder::new(w, Compression::new(5));
    let mut tar = tar::Builder::new(gz);
    tar.follow_symlinks(false);
    let mut stats = PackStats::default();
    let meta = fs::symlink_metadata(dir)?;
    if meta.is_file() {
        add_file(&mut tar, dir, Path::new(top), &mut stats)?;
    } else {
        walk(&mut tar, dir, Path::new(top), skip, &mut stats)?;
    }
    let gz = tar.into_inner()?;
    let mut w = gz.finish()?;
    w.flush()?;
    Ok(stats)
}

fn walk<W: Write>(
    tar: &mut tar::Builder<W>,
    dir: &Path,
    rel: &Path,
    skip: Option<&Path>,
    stats: &mut PackStats,
) -> io::Result<()> {
    if skip.is_some_and(|s| dir.starts_with(s)) {
        return Ok(());
    }
    let mut header = tar::Header::new_gnu();
    if let Ok(m) = fs::symlink_metadata(dir) {
        header.set_metadata(&m);
    }
    header.set_entry_type(tar::EntryType::Directory);
    header.set_size(0);
    tar.append_data(&mut header, rel, io::empty())?;
    let mut entries: Vec<_> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(Result::ok).collect(),
        Err(e) => {
            stats.skipped.push(format!("{}: {e}", rel.display()));
            return Ok(());
        }
    };
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let child = rel.join(e.file_name());
        let Ok(m) = fs::symlink_metadata(&path) else {
            continue;
        };
        let ft = m.file_type();
        if ft.is_dir() {
            walk(tar, &path, &child, skip, stats)?;
        } else if ft.is_symlink() {
            if let Ok(target) = fs::read_link(&path) {
                let mut h = tar::Header::new_gnu();
                h.set_metadata(&m);
                h.set_entry_type(tar::EntryType::Symlink);
                h.set_size(0);
                tar.append_link(&mut h, &child, &target)?;
            }
        } else if ft.is_file() {
            add_file(tar, &path, &child, stats)?;
        }
        // sockets, pipes and devices are left out
    }
    Ok(())
}

fn add_file<W: Write>(
    tar: &mut tar::Builder<W>,
    path: &Path,
    rel: &Path,
    stats: &mut PackStats,
) -> io::Result<()> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            stats.skipped.push(format!("{}: {e}", rel.display()));
            return Ok(());
        }
    };
    let Ok(m) = file.metadata() else {
        return Ok(());
    };
    let mut h = tar::Header::new_gnu();
    h.set_metadata(&m);
    h.set_size(m.len());
    tar.append_data(
        &mut h,
        rel,
        Exact {
            inner: file,
            left: m.len(),
            eof: false,
        },
    )?;
    stats.files += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    #[tokio::test]
    async fn packs_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        fs::create_dir_all(root.join("data/sub")).unwrap();
        fs::create_dir_all(root.join("skip")).unwrap();
        fs::write(root.join("data/a.txt"), b"hallo").unwrap();
        fs::write(root.join("data/sub/b.bin"), vec![7u8; 3000]).unwrap();
        fs::write(root.join("skip/x"), b"nein").unwrap();
        let (mut s, done) = pack(root.clone(), "app".into(), Some(root.join("skip")));
        let mut gz = vec![];
        while let Some(c) = s.next().await {
            gz.extend_from_slice(&c.unwrap());
        }
        let stats = done.await.unwrap().unwrap();
        assert_eq!(stats.files, 2);
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&gz[..]));
        let mut names: Vec<String> = ar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["app", "app/data", "app/data/a.txt", "app/data/sub", "app/data/sub/b.bin"]);
    }

    #[test]
    fn exact_pads_and_cuts() {
        let mut out = vec![];
        Exact { inner: &b"abc"[..], left: 5, eof: false }.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"abc\0\0");
        out.clear();
        Exact { inner: &b"abcdef"[..], left: 2, eof: false }.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"ab");
    }
}
