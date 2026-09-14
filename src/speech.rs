//! Where the spoken audio for an announcement comes from. Nothing here
//! knows about the Cast protocol (`cast.rs` does), so a different speech
//! source is a change to this file only.

/// Percent-encode for a query string (RFC 3986 unreserved characters pass).
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Google Translate's text-to-speech endpoint, which Cast devices fetch
/// directly (the same trick the home-automation crowd has used for years;
/// unofficial, so `MAX_CHARS` is its limit, not ours).
pub const MAX_CHARS: usize = 200;

pub fn tts_url(message: &str, lang: &str) -> String {
    format!(
        "https://translate.google.com/translate_tts?ie=UTF-8&client=tw-ob&tl={}&q={}",
        url_encode(lang),
        url_encode(message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_urls_are_encoded() {
        let u = tts_url("Dinner's ready, come down!", "en-GB");
        assert!(u.starts_with(
            "https://translate.google.com/translate_tts?ie=UTF-8&client=tw-ob&tl=en-GB&q="
        ));
        assert!(u.ends_with("Dinner%27s%20ready%2C%20come%20down%21"));
        assert_eq!(url_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(url_encode("é"), "%C3%A9");
    }
}
