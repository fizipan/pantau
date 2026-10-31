//! OS failures in words. The window shows Indonesian only; the raw text goes to stderr.

use std::fmt::Display;
use std::io::{self, ErrorKind};

fn reason(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::PermissionDenied => "izin ditolak",
        ErrorKind::NotFound => "lokasinya tidak ada",
        ErrorKind::AlreadyExists => "lokasinya sudah ada",
        ErrorKind::ReadOnlyFilesystem => "disk ini hanya bisa dibaca",
        ErrorKind::StorageFull => "disk penuh",
        ErrorKind::ResourceBusy | ErrorKind::ExecutableFileBusy => "sedang dipakai proses lain",
        ErrorKind::DirectoryNotEmpty => "foldernya belum kosong",
        ErrorKind::TimedOut => "terlalu lama",
        _ => "terjadi kesalahan sistem",
    }
}

/// `"<what>: <reason>"`. `what` says what failed, in Indonesian.
pub fn os_error(what: &str, e: &io::Error) -> String {
    eprintln!("[pantau] {what}: {e}");
    format!("{what}: {}", reason(e.kind()))
}

/// For failures that are not `io::Error`, so they carry no kind to explain.
pub fn other_error(what: &str, e: impl Display) -> String {
    eprintln!("[pantau] {what}: {e}");
    format!("{what}: {}", reason(ErrorKind::Other))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from(errno: i32) -> io::Error {
        io::Error::from_raw_os_error(errno)
    }

    #[test]
    fn both_permission_errnos_read_the_same_and_carry_no_os_text() {
        // EPERM is what macOS privacy protection returns; EACCES is plain file permissions.
        for errno in [libc::EPERM, libc::EACCES] {
            assert_eq!(os_error("Gagal menghapus .next", &from(errno)), "Gagal menghapus .next: izin ditolak");
        }
    }

    #[test]
    fn no_kind_lets_english_through() {
        for errno in [libc::ENOENT, libc::EEXIST, libc::EROFS, libc::ENOSPC, libc::EBUSY, libc::ETXTBSY, libc::ENOTEMPTY, libc::ETIMEDOUT, libc::EIO, libc::EINVAL, libc::ENAMETOOLONG] {
            let text = os_error("Gagal", &from(errno));
            assert!(!text.contains("os error"), "{text}");
            assert!(text.starts_with("Gagal: ") && text.len() > "Gagal: ".len(), "{text}");
        }
    }

    #[test]
    fn other_failures_keep_their_detail_out_of_the_sentence() {
        assert_eq!(other_error("Pengaturan tidak bisa dibaca", "No such thing in English"), "Pengaturan tidak bisa dibaca: terjadi kesalahan sistem");
    }
}
