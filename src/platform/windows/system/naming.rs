//! Platform-independent pipe-name framing, also testable on non-Windows hosts.

pub(super) fn hash_input(sid: &str, canonical_path: &[u16]) -> Vec<u8> {
    // Separate the SID from the path with a UTF-16 NUL, impossible in either.
    // Encode code units directly: lossy UTF-8 conversion would alias paths.
    sid.encode_utf16()
        .chain(Some(0))
        .chain(canonical_path.iter().copied())
        .flat_map(u16::to_le_bytes)
        .collect()
}

pub(super) fn pipe_name(digest: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut name = String::from(r"\\.\pipe\ScreenAnswer-");
    for byte in digest {
        write!(&mut name, "{byte:02x}").unwrap();
    }
    name
}

pub(super) fn valid_file_name(name: &[u16]) -> bool {
    !name.is_empty()
        && !matches!(name.last(), Some(32 | 46)) // trailing space/dot
        && !name.iter().any(|c| matches!(*c, 0 | 47 | 58 | 92)) // NUL, /, :, \
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_is_scoped_and_lossless() {
        let path: Vec<u16> = r"\\?\C:\Users\alice\ScreenAnswer".encode_utf16().collect();
        assert_eq!(
            hash_input("S-1-5-21-1", &path),
            hash_input("S-1-5-21-1", &path)
        );
        assert_ne!(
            hash_input("S-1-5-21-1", &path),
            hash_input("S-1-5-21-2", &path)
        );
        assert_ne!(
            hash_input("S-1-5-21-1", &path),
            hash_input("S-1-5-21-1", &[1])
        );
        assert_ne!(hash_input("S", &[0xd800]), hash_input("S", &[0xfffd]));
        assert_ne!(hash_input("S1", &[50]), hash_input("S", &[49, 50]));
    }

    #[test]
    fn pipe_name_is_local_and_bounded() {
        let name = pipe_name(&[0xab; 32]);
        assert_eq!(name, format!(r"\\.\pipe\ScreenAnswer-{}", "ab".repeat(32)));
        assert!(name.len() < 256);
    }

    #[test]
    fn file_names_cannot_select_streams_or_escape_parent() {
        for valid in ["daemon.log", "daemon.lock", "ą.txt"] {
            assert!(valid_file_name(&valid.encode_utf16().collect::<Vec<_>>()));
        }
        for invalid in ["", ".", "..", "a:b", "a/b", "a\\b", "a\0b", "a.", "a "] {
            assert!(!valid_file_name(
                &invalid.encode_utf16().collect::<Vec<_>>()
            ));
        }
    }
}
