//! The secret of the session does not always fit in one keyring entry.
//!
//! The Windows Credential Manager takes 2.5 KB (`CRED_MAX_CREDENTIAL_BLOB_SIZE`) per secret, and
//! it stores it as UTF-16: that is 1280 characters. The cookies of an account can be bigger than
//! that, so the secret is cut in pieces and each piece goes in its own entry.

/// How many characters go in one keyring entry.
///
/// The Windows Credential Manager takes 2560 bytes of UTF-16, that is 1280 characters; we stay
/// below it so there is always room for the header of the credential.
pub const MAX_PART_LEN: usize = 1200;

/// Cuts a secret in pieces of at most `max_len` characters.
///
/// The count is in UTF-16 characters, the same unit the keyring of Windows uses, and the cut never
/// breaks a character.
pub fn split(secret: &str, max_len: usize) -> Vec<String> {
    if secret.is_empty() {
        return Vec::new();
    }

    let max_len = max_len.max(1);
    let mut parts = Vec::new();
    let mut piece = String::new();
    let mut len = 0usize;

    for character in secret.chars() {
        let width = character.len_utf16();
        if len + width > max_len && !piece.is_empty() {
            parts.push(std::mem::take(&mut piece));
            len = 0;
        }
        piece.push(character);
        len += width;
    }

    if !piece.is_empty() {
        parts.push(piece);
    }

    parts
}

/// Puts the pieces back together.
pub fn join(parts: &[String]) -> String {
    parts.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_secret_is_one_piece() {
        assert_eq!(split("abc", 10), vec!["abc".to_string()]);
    }

    #[test]
    fn an_empty_secret_has_no_pieces() {
        assert!(split("", 10).is_empty());
    }

    #[test]
    fn a_long_secret_is_cut_in_pieces() {
        let secret = "a".repeat(25);
        let parts = split(&secret, 10);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].len(), 10);
        assert_eq!(parts[2].len(), 5);
        assert_eq!(join(&parts), secret);
    }

    #[test]
    fn the_cut_does_not_break_a_character() {
        // "ñ" is two characters in UTF-16: a cut of three cannot fall in the middle of one.
        let secret = "aañb";
        let parts = split(secret, 3);
        assert_eq!(join(&parts), secret);
        for part in &parts {
            assert!(part.encode_utf16().count() <= 3);
        }
    }

    #[test]
    fn the_pieces_fit_in_the_keyring() {
        let secret = "x".repeat(9000);
        let parts = split(&secret, MAX_PART_LEN);
        assert!(parts.len() >= 8);
        assert!(
            parts
                .iter()
                .all(|part| part.encode_utf16().count() <= MAX_PART_LEN)
        );
        assert_eq!(join(&parts), secret);
    }
}
