//! Files on disk as the store writes them: each whole or not at all and
//! made durable, and one that cannot be read set aside beside it. What the
//! desktop and mobile store ([`super::desktop`]), the history's files
//! ([`super::files`]) and Android's backup share.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Move a file in `root` that cannot be read, `stem`.json at `path`, out of
/// the way of the next save, keeping it beside the new one to be recovered,
/// and say what it is called now: nothing, when it could not be moved.
pub(super) fn move_aside(root: &Path, path: &Path, stem: &str) -> Option<String> {
    let name = aside_name(root, stem);
    match fs::rename(path, root.join(&name)) {
        Ok(()) => Some(name),
        Err(err) => {
            eprintln!("could not set aside {stem}.json, which could not be read: {err}");
            None
        }
    }
}

/// Where the user can find `name`, a file of the store's in `root`, in
/// words for them: on a computer. A phone keeps the store inside the app,
/// where no file manager reaches.
pub(super) fn shown(root: &Path, name: &str) -> Option<String> {
    let reachable = !cfg!(any(target_os = "android", target_os = "ios"));
    reachable.then(|| root.join(name).display().to_string())
}

/// A name in `dir` no file has yet for one set aside from `stem`.json:
/// `stem.unreadable-<seconds>.json`, or failing that with `-2`, `-3`… after
/// it, so that one set aside before is never written over.
pub(super) fn aside_name(dir: &Path, stem: &str) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    free_name(dir, &format!("{stem}.unreadable-{stamp}.json"))
}

/// Write `name` in `dir` whole or not at all.
///
/// The bytes go to a temporary file beside it, are flushed to the disk, and
/// only then take the name, so a crash, a power cut or a full disk leaves
/// either the old file or the new one — never a half-written one, and never
/// an empty one whose rename reached the disk before its data. The temporary
/// name is this process's own, so two copies of the app running at once
/// cannot write into each other's.
pub(super) fn write_whole(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    place_whole(dir, name, bytes)?;
    sync_dir(dir)
}

/// Write `name` in `dir` whole or not at all, as [`write_whole`] does, but
/// leave the folder's own record of the name to the caller to make durable
/// ([`sync_dir`]): once for many files written together.
pub(super) fn place_whole(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let tmp = temporary(dir, name);
    let written = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, &path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Where `name` in `dir` is written before it takes its name: a name of
/// this process's own, and of this write's, so two copies of the app
/// running at once never write into each other's.
pub(super) fn temporary(dir: &Path, name: &str) -> PathBuf {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Save a file in `folder` as `name`, or as the first name free after it
/// ([`free_name`]), written a piece at a time by `write`, and say what it
/// is called, and what `write` said of it.
///
/// It is written as [`write_whole`] writes a file, into a file of its own
/// beside it, made durable, and only then named: a file cut short, by a
/// full disk, the app stopping or `write` giving up, leaves none that is
/// half of one, and nothing is ever written over. However large it is, no
/// more of it than a buffer's worth is held at a time.
pub(super) fn save_streamed<T, E: From<io::Error>>(
    folder: &Path,
    name: &str,
    write: impl FnOnce(&mut dyn Write) -> Result<T, E>,
) -> Result<(String, T), E> {
    fs::create_dir_all(folder)?;
    let name = free_name(folder, name);
    let tmp = temporary(folder, &name);
    let written = (|| -> Result<T, E> {
        let mut out = io::BufWriter::new(fs::File::create(&tmp)?);
        let said = write(&mut out)?;
        out.into_inner()
            .map_err(io::IntoInnerError::into_error)?
            .sync_all()?;
        fs::rename(&tmp, folder.join(&name))?;
        Ok(said)
    })();
    match written {
        Ok(said) => {
            sync_dir(folder)?;
            Ok((name, said))
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

/// `name` in `folder`, or, if a file is already called that, the first of
/// `name-2`, `name-3` and so on that is free: an earlier backup is never
/// written over.
pub(super) fn free_name(folder: &Path, name: &str) -> String {
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    std::iter::once(name.to_string())
        .chain((2..1_000).map(|n| {
            if extension.is_empty() {
                format!("{stem}-{n}")
            } else {
                format!("{stem}-{n}.{extension}")
            }
        }))
        .find(|candidate| !folder.join(candidate).exists())
        .unwrap_or_else(|| name.to_string())
}

/// Make a rename in `dir` as durable as the data it names.
///
/// A filesystem that flushes no folder at all, as a virtual machine's
/// shared folder or some network mounts flush none, says so (`EINVAL`,
/// `ENOTSUP`): the rename is then as durable as it can be made there, and
/// the write stands, as SQLite and PostgreSQL take it. Any other failure is
/// one.
#[cfg(unix)]
pub(super) fn sync_dir(dir: &Path) -> io::Result<()> {
    match fs::File::open(dir)?.sync_all() {
        Err(err) if flushes_no_folder(&err) => Ok(()),
        synced => synced,
    }
}

/// Whether `err`, from flushing a folder, says its filesystem flushes no
/// folder at all: `EINVAL`, or `ENOTSUP` (with `EOPNOTSUPP`, where that is
/// another number), which std gives no kind of its own.
#[cfg(unix)]
fn flushes_no_folder(err: &io::Error) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const NOT_SUPPORTED: &[i32] = &[95];
    #[cfg(target_vendor = "apple")]
    const NOT_SUPPORTED: &[i32] = &[45, 102];
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    const NOT_SUPPORTED: &[i32] = &[];
    err.kind() == io::ErrorKind::InvalidInput
        || err
            .raw_os_error()
            .is_some_and(|code| NOT_SUPPORTED.contains(&code))
}

/// Windows cannot open a directory to flush it; its renames are journalled.
#[cfg(not(unix))]
pub(super) fn sync_dir(_: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::persist::TempDir;

    /// A folder on a filesystem that flushes no folder at all, as a virtual
    /// machine's shared folder (vboxsf) or some network mounts flush none,
    /// is as durable as it can be made once what was renamed in it is: no
    /// failed write. Every session written to such a folder was told of as
    /// not saved, and tried again for as long as the app ran, and the move
    /// to records was put off at every launch. `/proc` says what such a
    /// filesystem says, `EINVAL`. A folder that cannot be flushed for any
    /// other reason still fails the write.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_folder_its_filesystem_cannot_flush_fails_no_write() {
        let Ok(proc) = fs::File::open("/proc") else {
            eprintln!("skipped: there is no /proc to flush");
            return;
        };
        assert_eq!(
            proc.sync_all().map_err(|err| err.kind()),
            Err(io::ErrorKind::InvalidInput),
            "flushed by nothing"
        );
        assert!(sync_dir(Path::new("/proc")).is_ok());
        let dir = TempDir::new("flush-none");
        assert!(sync_dir(&dir.0.join("not there")).is_err());
    }
}
