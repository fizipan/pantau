//! Folder sizes and project folder discovery.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measured {
    Bytes(u64),
    Denied,
    Missing,
}

/// How many entries a walk reads between asking whether to go on: asking costs system calls, and
/// a walk of this many entries takes milliseconds.
const CHECK_EVERY: u64 = 1024;

/// On-disk size (`st_blocks * 512`). Symlinks are counted as themselves, never followed.
/// Only an unreadable root is `Denied`; unreadable children are skipped.
/// `None` when `go` said to stop partway: a partial sum is not a size.
pub fn measure_while(path: &Path, go: &dyn Fn() -> bool) -> Option<Measured> {
    let root = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => return Some(failure(&e)),
    };
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut total = 0;
    let mut read = 0;
    if !root.is_dir() {
        return Some(Measured::Bytes(size_of(&root, &mut seen)));
    }
    // Only the root being unreadable is a verdict; a child that cannot be read is skipped below.
    if let Err(e) = fs::read_dir(path) {
        return Some(failure(&e));
    }
    total += size_of(&root, &mut seen);
    // Folders still to read, as paths: a deep tree must not hold one open directory per level.
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd {
            read += 1;
            if read % CHECK_EVERY == 0 && !go() {
                return None;
            }
            let Ok(entry) = entry else { continue };
            let Ok(meta) = entry.metadata() else { continue };
            total += size_of(&meta, &mut seen);
            if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    Some(Measured::Bytes(total))
}

/// `measure_while` that never stops, for tests.
#[cfg(test)]
pub fn measure(path: &Path) -> Measured {
    measure_while(path, &|| true).unwrap()
}

/// Scan roots that exist but cannot be listed. `find_named_dirs` has to skip them, and macOS
/// privacy protection fails exactly here, so without this their rows would silently vanish.
pub fn denied_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots.iter().filter(|r| fs::read_dir(r).is_err_and(|e| failure(&e) == Measured::Denied)).cloned().collect()
}

fn failure(e: &io::Error) -> Measured {
    match e.kind() {
        io::ErrorKind::NotFound => Measured::Missing,
        _ => Measured::Denied,
    }
}

fn size_of(meta: &fs::Metadata, seen: &mut HashSet<(u64, u64)>) -> u64 {
    if meta.nlink() > 1 && !seen.insert((meta.dev(), meta.ino())) {
        return 0;
    }
    meta.blocks() * 512
}

/// Directories called `name` at most `max_depth` levels under each root. Never descends
/// into a match, `node_modules` or `.next` build output, and does not follow symlinks.
pub fn find_named_dirs(roots: &[PathBuf], name: &str, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in roots {
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for entry in rd.flatten() {
                let Ok(ft) = entry.file_type() else { continue };
                if !ft.is_dir() {
                    continue;
                }
                let file_name = entry.file_name();
                let child_depth = depth + 1;
                if child_depth > max_depth {
                    continue;
                }
                if file_name == name {
                    found.push(entry.path());
                } else if file_name != "node_modules" && file_name != ".next" {
                    stack.push((entry.path(), child_depth));
                }
            }
        }
    }
    found.sort();
    // Nested roots reach the same folder twice.
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    const MB: usize = 1_000_000;

    fn write(path: &Path, len: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7u8; len]).unwrap();
    }

    struct Unlock(PathBuf);
    impl Drop for Unlock {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn hardlinks_are_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.bin"), MB);
        fs::hard_link(dir.path().join("a.bin"), dir.path().join("b.bin")).unwrap();
        assert!(matches!(measure(dir.path()), Measured::Bytes(b) if (MB as u64..1_500_000).contains(&b)));
    }

    #[test]
    fn symlinks_are_not_followed() {
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("big.bin"), 5 * MB);
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("small.bin"), 10);
        symlink(outside.path(), dir.path().join("link")).unwrap();
        assert!(matches!(measure(dir.path()), Measured::Bytes(b) if b < 500_000));
    }

    #[test]
    fn a_missing_folder_is_missing_not_zero() {
        assert_eq!(measure(Path::new("/tidak/ada")), Measured::Missing);
    }

    #[test]
    fn an_unreadable_folder_is_denied_not_zero() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        write(&locked.join("x.bin"), MB);
        let _unlock = Unlock(locked.clone());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(measure(&locked), Measured::Denied);
    }

    #[test]
    fn an_unreadable_child_is_skipped_and_the_rest_still_counts() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("open.bin"), MB);
        let locked = dir.path().join("locked");
        write(&locked.join("x.bin"), 3 * MB);
        let _unlock = Unlock(locked.clone());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        assert!(matches!(measure(dir.path()), Measured::Bytes(b) if (MB as u64..2 * MB as u64).contains(&b)));
    }

    #[test]
    fn a_walk_stops_inside_a_big_folder_when_told_to() {
        use std::cell::Cell;
        let dir = tempfile::tempdir().unwrap();
        for i in 0..1100 {
            fs::write(dir.path().join(format!("f{i}")), b"x").unwrap();
        }
        let asked = Cell::new(0);
        let go = || {
            asked.set(asked.get() + 1);
            false
        };
        assert_eq!(measure_while(dir.path(), &go), None);
        assert_eq!(asked.get(), 1, "stops at the first check");
        assert!(matches!(measure_while(dir.path(), &|| true), Some(Measured::Bytes(_))));
    }

    #[test]
    fn a_very_deep_tree_does_not_hold_a_file_descriptor_per_level() {
        use std::os::unix::io::AsRawFd;
        let dir = tempfile::tempdir().unwrap();
        let mut deep = dir.path().to_path_buf();
        for _ in 0..450 {
            deep.push("d");
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("f"), vec![1u8; 100_000]).unwrap();
        // Descriptors left over once the walk is under way: take the number of the next one opened.
        let probe = || fs::File::open("/dev/null").unwrap().as_raw_fd();
        let base = probe();
        let seen = std::cell::Cell::new(0);
        // The walk asks every CHECK_EVERY entries; pad the tree so it asks while deep down.
        for i in 0..1100 {
            fs::write(deep.join(format!("p{i}")), b"x").unwrap();
        }
        let go = || {
            seen.set(seen.get().max(probe()));
            true
        };
        assert!(matches!(measure_while(dir.path(), &go), Some(Measured::Bytes(b)) if b > 100_000));
        assert!(seen.get() < base + 200, "walk held {} descriptors at once", seen.get() - base);
    }

    #[test]
    fn a_small_folder_is_measured_without_asking() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.bin"), 10);
        assert!(matches!(measure_while(dir.path(), &|| false), Some(Measured::Bytes(_))));
    }

    #[test]
    fn only_a_scan_root_that_exists_and_cannot_be_listed_is_denied() {
        let dir = tempfile::tempdir().unwrap();
        let (locked, open, missing) = (dir.path().join("locked"), dir.path().join("open"), dir.path().join("missing"));
        fs::create_dir_all(&locked).unwrap();
        fs::create_dir_all(&open).unwrap();
        let _unlock = Unlock(locked.clone());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(denied_roots(&[locked.clone(), open, missing]), vec![locked]);
    }

    #[test]
    fn overlapping_roots_find_a_folder_once() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fs::create_dir_all(r.join("a/web/.next")).unwrap();
        let found = find_named_dirs(&[r.to_path_buf(), r.join("a")], ".next", 5);
        assert_eq!(found, vec![r.join("a/web/.next")]);
    }

    #[test]
    fn find_named_dirs_respects_depth_and_skips_node_modules() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fs::create_dir_all(r.join("a/web/.next/inner/.next")).unwrap();
        fs::create_dir_all(r.join("a/node_modules/pkg/.next")).unwrap();
        fs::create_dir_all(r.join("b/c/d/e/f/.next")).unwrap();
        let found = find_named_dirs(&[r.to_path_buf()], ".next", 5);
        assert_eq!(found, vec![r.join("a/web/.next")]);
        let deep = find_named_dirs(&[r.to_path_buf()], ".next", 6);
        assert!(deep.contains(&r.join("b/c/d/e/f/.next")));
    }

    #[test]
    fn find_named_dirs_can_return_node_modules_without_entering_them() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fs::create_dir_all(r.join("p/node_modules/dep/node_modules")).unwrap();
        let found = find_named_dirs(&[r.to_path_buf()], "node_modules", 5);
        assert_eq!(found, vec![r.join("p/node_modules")]);
    }

    #[test]
    fn find_named_dirs_does_not_enter_next_build_output() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fs::create_dir_all(r.join("p/.next/standalone/node_modules")).unwrap();
        fs::create_dir_all(r.join("p/node_modules")).unwrap();
        let found = find_named_dirs(&[r.to_path_buf()], "node_modules", 5);
        assert_eq!(found, vec![r.join("p/node_modules")]);
    }
}
