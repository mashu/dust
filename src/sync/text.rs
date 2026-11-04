//! The few encodings sync writes and reads by hand: hex for keys and names,
//! base64url for the sign-in's code challenge, and percent-encoding for the
//! addresses Google is asked at. Written out here rather than taken from a
//! crate each: they are a line or two apiece, and what they write must
//! never change with a dependency.

/// `bytes` in lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0xf)] as char);
    }
    out
}

/// The bytes `text` is the hex of, either case: none where it is anything
/// else.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks(2)
        .map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

/// `bytes` in base64url, unpadded, as OAuth's code challenge is written
/// (RFC 7636).
#[cfg_attr(not(any(feature = "desktop", test)), allow(dead_code))]
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (at, &byte)| n | u32::from(byte) << (16 - 8 * at));
        for at in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * at) & 0x3f) as usize] as char);
        }
    }
    out
}

/// `text` percent-encoded for a query or a form: every byte but the
/// unreserved ones (RFC 3986) written as `%XX`.
pub fn percent(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&hex(&[byte]).to_ascii_uppercase());
        }
    }
    out
}

/// `text` percent-decoded, `+` read as a space, as a query or a form is:
/// none where it decodes to no UTF-8, or a `%` is not followed by two hex
/// digits.
#[cfg_attr(not(any(feature = "desktop", test)), allow(dead_code))]
pub fn unpercent(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' => {
                let pair = text.get(at + 1..at + 3)?;
                out.push(*unhex(pair)?.first()?);
                at += 3;
            }
            b'+' => {
                out.push(b' ');
                at += 1;
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// `pairs` as a query or a form body: `a=1&b=2`, each part percent-encoded.
pub fn query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", percent(key), percent(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The pairs of a query or a fragment, `a=1&b=2`, each decoded; a pair that
/// does not decode is passed over.
#[cfg_attr(not(any(feature = "web", feature = "desktop", test)), allow(dead_code))]
pub fn parse_query(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((unpercent(key)?, unpercent(value)?))
        })
        .collect()
}

/// `N` bytes from the system's own source of randomness: none where it has
/// none to give, which no platform Dust runs on is without.
pub fn random<const N: usize>() -> Option<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes)
}

/// `N` random bytes in hex: a name, an id or a state no one can guess.
pub fn random_hex<const N: usize>() -> Option<String> {
    random::<N>().map(|bytes| hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_reads_back_what_it_writes() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(unhex("000fa5ff"), Some(vec![0x00, 0x0f, 0xa5, 0xff]));
        assert_eq!(unhex("000FA5FF"), Some(vec![0x00, 0x0f, 0xa5, 0xff]));
        assert_eq!(unhex(""), Some(Vec::new()));
        for not_hex in ["0", "0g", "zz", "a b "] {
            assert_eq!(unhex(not_hex), None, "{not_hex}");
        }
    }

    /// RFC 4648's own examples, unpadded, in the URL-safe alphabet; and
    /// RFC 7636's code challenge for its example verifier.
    #[test]
    fn base64url_is_the_rfcs() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64url(plain.as_bytes()), encoded, "{plain}");
        }
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(
            base64url(&digest),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_query_reads_back_as_it_was_written() {
        let pairs = [
            ("redirect_uri", "http://127.0.0.1:5000/"),
            ("scope", "https://www.googleapis.com/auth/drive.appdata"),
            ("state", "a b+c&d=é"),
        ];
        let written = query(&pairs);
        assert_eq!(
            written,
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A5000%2F&scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.appdata&state=a%20b%2Bc%26d%3D%C3%A9"
        );
        let read = parse_query(&written);
        let want: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(read, want);
        assert_eq!(
            parse_query("a=1+2&&b&c=%zz"),
            [("a".into(), "1 2".into()), ("b".into(), String::new())]
        );
    }

    #[test]
    fn randomness_is_fresh_each_time() {
        let (one, two) = (random::<16>().unwrap(), random::<16>().unwrap());
        assert_ne!(one, two);
        assert_eq!(random_hex::<16>().unwrap().len(), 32);
    }
}
