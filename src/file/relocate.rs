use anyhow::{bail, Context, Result};
use std::path::{Component, Path, PathBuf};

/// Outcome of a script-requested move.
pub struct MovedFile {
    /// Resolved destination directory.
    pub dir: PathBuf,
    /// Final file name; differs from the input when it had to be made unique.
    pub filename: String,
}

/// Move `file_dir/filename` into the directory `target` requested by a script.
///
/// A relative `target` is resolved against `file_dir`, the target directory is
/// created if missing, and an existing file is never overwritten. Errors carry
/// a human-readable reason (for example "contains '..'").
pub fn move_into_dir(file_dir: &Path, filename: &str, target: &str) -> Result<MovedFile> {
    if target.trim().is_empty() {
        bail!("empty path");
    }
    let t = PathBuf::from(target);
    // Scripts are untrusted; refuse traversal out of the requested location.
    if t.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("contains '..'");
    }

    // `file_dir.join(".")` does not compare equal to `file_dir`, so drop `.`
    // components to detect the no-op case below.
    let t_clean: PathBuf = t
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    let dir = if t_clean.is_absolute() {
        t_clean.clone()
    } else {
        file_dir.join(&t_clean)
    };

    // Moving onto itself must not touch the file: the uniqueness check below
    // would see the file itself and rename it to a suffixed copy.
    if t_clean.as_os_str().is_empty() || dir == file_dir {
        return Ok(MovedFile {
            dir: file_dir.to_path_buf(),
            filename: filename.to_string(),
        });
    }

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create directory '{}'", dir.display()))?;

    let final_name = crate::file::naming::ensure_unique_filename(&dir, filename);
    let src = file_dir.join(filename);
    let dst = dir.join(&final_name);

    // Any rename failure (typically a cross-volume move) falls back to copy +
    // remove; the copy fails for the same permission problems anyway.
    if std::fs::rename(&src, &dst).is_err() {
        copy_then_remove(&src, &dst)
            .with_context(|| format!("cannot move '{}' to '{}'", src.display(), dst.display()))?;
    }

    Ok(MovedFile {
        dir,
        filename: final_name,
    })
}

/// Copy `src` to `dst`, keep the modification time, then delete `src`.
fn copy_then_remove(src: &Path, dst: &Path) -> std::io::Result<()> {
    let mtime = std::fs::metadata(src)?.modified()?;
    if let Err(e) = std::fs::copy(src, dst) {
        // Do not leave a partial file behind.
        let _ = std::fs::remove_file(dst);
        return Err(e);
    }
    std::fs::File::options()
        .write(true)
        .open(dst)?
        .set_modified(mtime)?;
    std::fs::remove_file(src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn write(path: &Path, content: &str) {
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn moves_into_new_absolute_dir_creating_it() {
        let tmp = tempfile::tempdir().unwrap();
        let file_dir = tmp.path().join("src");
        std::fs::create_dir_all(&file_dir).unwrap();
        write(&file_dir.join("a.txt"), "hello");
        let target = tmp.path().join("x").join("y").join("z");

        let moved = move_into_dir(&file_dir, "a.txt", target.to_str().unwrap()).unwrap();

        assert_eq!(moved.dir, target);
        assert_eq!(moved.filename, "a.txt");
        assert_eq!(std::fs::read_to_string(target.join("a.txt")).unwrap(), "hello");
        assert!(!file_dir.join("a.txt").exists());
    }

    #[test]
    fn relative_target_resolves_against_file_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let file_dir = tmp.path().join("src");
        std::fs::create_dir_all(&file_dir).unwrap();
        write(&file_dir.join("a.txt"), "hello");

        let moved = move_into_dir(&file_dir, "a.txt", "images/sub").unwrap();

        assert_eq!(moved.dir, file_dir.join("images").join("sub"));
        assert!(file_dir.join("images").join("sub").join("a.txt").exists());
        assert!(!file_dir.join("a.txt").exists());
    }

    #[test]
    fn rejects_parent_dir_components() {
        let tmp = tempfile::tempdir().unwrap();
        let file_dir = tmp.path().join("src");
        std::fs::create_dir_all(&file_dir).unwrap();
        write(&file_dir.join("a.txt"), "hello");

        for target in ["../x", "a/../b"] {
            let err = move_into_dir(&file_dir, "a.txt", target).err().unwrap();
            assert!(err.to_string().contains("'..'"), "{}", err);
            assert!(file_dir.join("a.txt").exists());
        }
        assert!(!tmp.path().join("x").exists());
        assert!(!file_dir.join("a").exists());
    }

    #[test]
    fn rejects_empty_target() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a.txt"), "hello");

        for target in ["", "   "] {
            let err = move_into_dir(tmp.path(), "a.txt", target).err().unwrap();
            assert!(err.to_string().contains("empty"));
        }
        assert!(tmp.path().join("a.txt").exists());
    }

    #[test]
    fn does_not_overwrite_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file_dir = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        std::fs::create_dir_all(&file_dir).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        write(&file_dir.join("a.txt"), "new");
        write(&dest.join("a.txt"), "old");

        let moved = move_into_dir(&file_dir, "a.txt", dest.to_str().unwrap()).unwrap();

        assert_ne!(moved.filename, "a.txt");
        assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "old");
        assert_eq!(
            std::fs::read_to_string(dest.join(&moved.filename)).unwrap(),
            "new"
        );
        assert!(!file_dir.join("a.txt").exists());
    }

    #[test]
    fn target_equal_to_file_dir_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let file_dir = tmp.path().to_path_buf();
        write(&file_dir.join("a.txt"), "hello");

        for target in [".", file_dir.to_str().unwrap()] {
            let moved = move_into_dir(&file_dir, "a.txt", target).unwrap();
            assert_eq!(moved.dir, file_dir);
            assert_eq!(moved.filename, "a.txt");
            assert_eq!(
                std::fs::read_to_string(file_dir.join("a.txt")).unwrap(),
                "hello"
            );
        }
        assert_eq!(std::fs::read_dir(&file_dir).unwrap().count(), 1);
    }

    #[test]
    fn copy_then_remove_preserves_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src.bin");
        let dst = tmp.path().join("dst.bin");
        write(&src, "data");
        let old = SystemTime::now() - Duration::from_secs(3600 * 24 * 30);
        std::fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(old)
            .unwrap();

        copy_then_remove(&src, &dst).unwrap();

        assert!(!src.exists());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "data");
        let got = std::fs::metadata(&dst).unwrap().modified().unwrap();
        let diff = got
            .duration_since(old)
            .unwrap_or_else(|e| e.duration());
        assert!(diff < Duration::from_secs(1), "mtime differs by {:?}", diff);
    }
}
