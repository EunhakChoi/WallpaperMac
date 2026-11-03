//! UTF-8 JSON with optional BOM, comments and trailing commas.
use crate::{Error, Result};

pub fn parse(input: &[u8]) -> Result<serde_json::Value> {
    let bom = if input.starts_with(&[0xef, 0xbb, 0xbf]) {
        3
    } else {
        0
    };
    let bytes = &input[bom..];
    std::str::from_utf8(bytes)
        .map_err(|e| Error::parse(bom + e.valid_up_to(), "JSON is not UTF-8"))?;
    let mut clean = bytes.to_vec();
    let mut i = 0;
    let mut quoted = false;
    let mut escape = false;
    while i < bytes.len() {
        let c = bytes[i];
        if quoted {
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                quoted = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            quoted = true;
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && !matches!(bytes[i], b'\r' | b'\n') {
                clean[i] = b' ';
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            let start = i;
            clean[i] = b' ';
            clean[i + 1] = b' ';
            i += 2;
            loop {
                if i + 1 >= bytes.len() {
                    return Err(Error::parse(bom + start, "unterminated JSON block comment"));
                }
                if bytes[i..].starts_with(b"*/") {
                    clean[i] = b' ';
                    clean[i + 1] = b' ';
                    i += 2;
                    break;
                }
                if !matches!(bytes[i], b'\r' | b'\n') {
                    clean[i] = b' ';
                }
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    i = 0;
    quoted = false;
    escape = false;
    while i < clean.len() {
        let c = clean[i];
        if quoted {
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                quoted = false;
            }
        } else if c == b'"' {
            quoted = true;
        } else if c == b',' {
            let mut j = i + 1;
            while j < clean.len() && clean[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < clean.len() && matches!(clean[j], b']' | b'}') {
                clean[i] = b' ';
            }
        }
        i += 1;
    }
    serde_json::from_slice(&clean).map_err(|e| {
        let line_start = clean
            .split(|b| *b == b'\n')
            .take(e.line().saturating_sub(1))
            .map(|s| s.len() + 1)
            .sum::<usize>();
        Error::parse(
            bom + line_start + e.column().saturating_sub(1),
            format!("JSON: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings_comments_and_trailing_commas() {
        let v = parse(b"\xef\xbb\xbf{\"url\":\"https://host/a/*b*/,\",/*x*/\"v\":[1,//c\n2,],}")
            .unwrap();
        assert_eq!(v["url"], "https://host/a/*b*/,");
        assert_eq!(v["v"], serde_json::json!([1, 2]));
    }
    #[test]
    fn unterminated_and_invalid_utf8_fail() {
        assert!(parse(b"{/*").is_err());
        assert!(parse(b"[\xff]").is_err());
        assert!(parse(br#"{"a":"x\"}"#).is_err());
    }
}
