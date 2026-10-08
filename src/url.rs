/// Validate an http(s) URL and return its lowercase host.
/// The URL itself is passed to the browser untouched; the host is only used for rule matching.
pub fn host(url: &str) -> Result<String, String> {
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("URL contains whitespace or control characters".into());
    }
    let (scheme, rest) = url.split_once("://").ok_or("not an http(s) URL")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(format!("unsupported scheme '{scheme}'"));
    }
    // '\\' ends the authority too: WHATWG parsing treats it like '/' in http(s) URLs,
    // so https://evil.com\@good.com must route by the host the browser will visit.
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if hostport.starts_with('[') {
        hostport.split_once(']').map_or(hostport, |(h, _)| h).trim_start_matches('[')
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() { Err("URL has no host".into()) } else { Ok(host) }
}

#[cfg(test)]
mod tests {
    use super::host;

    #[test]
    fn extracts_host() {
        assert_eq!(host("https://Example.COM/a?b#c").unwrap(), "example.com");
        assert_eq!(host("http://user:pw@a.b:8080/x").unwrap(), "a.b");
        assert_eq!(host("https://example.com.").unwrap(), "example.com");
        assert_eq!(host("https://[::1]:80/").unwrap(), "::1");
        assert_eq!(host("https://a.com?x=1").unwrap(), "a.com");
    }

    #[test]
    fn rejects_bad_urls() {
        for u in ["file:///etc/passwd", "javascript:alert(1)", "https://", "https://a.com/ b", "ftp://a.com", "a.com"] {
            assert!(host(u).is_err(), "{u}");
        }
    }

    #[test]
    fn scheme_is_case_insensitive() {
        assert_eq!(host("HTTPS://a.com").unwrap(), "a.com");
        assert_eq!(host("HtTp://a.com").unwrap(), "a.com");
    }

    #[test]
    fn host_is_lowercased_and_trailing_dots_stripped() {
        assert_eq!(host("https://A.B.Example.COM/").unwrap(), "a.b.example.com");
        assert_eq!(host("https://example.com../").unwrap(), "example.com");
    }

    #[test]
    fn userinfo_is_never_the_host() {
        assert_eq!(host("https://evil.com@good.com/").unwrap(), "good.com");
        assert_eq!(host("https://evil.com:pw@good.com:443/").unwrap(), "good.com");
        assert_eq!(host("https://a@b@c.com/").unwrap(), "c.com");
        assert_eq!(host("https://good.com/@evil.com").unwrap(), "good.com");
        assert_eq!(host("https://good.com?u=@evil.com").unwrap(), "good.com");
    }

    #[test]
    fn backslash_ends_the_authority_like_browsers() {
        assert_eq!(host(r"https://evil.com\@good.com/").unwrap(), "evil.com");
        assert_eq!(host(r"https://a.com\path").unwrap(), "a.com");
        assert!(host(r"https://\@a.com").is_err());
    }

    #[test]
    fn delimiters_end_the_authority() {
        assert_eq!(host("https://a.com/path").unwrap(), "a.com");
        assert_eq!(host("https://a.com?q").unwrap(), "a.com");
        assert_eq!(host("https://a.com#frag").unwrap(), "a.com");
        assert_eq!(host("https://a.com:8080").unwrap(), "a.com");
    }

    #[test]
    fn ip_literals() {
        assert_eq!(host("http://127.0.0.1:3000/x").unwrap(), "127.0.0.1");
        assert_eq!(host("https://[2001:DB8::1]/").unwrap(), "2001:db8::1");
        assert_eq!(host("https://[::1]").unwrap(), "::1");
    }

    #[test]
    fn empty_hosts_are_rejected() {
        for u in ["http://", "http:///path", "http://:80/", "http://user@/", "http://./", "http://?q", "http://#f"] {
            assert!(host(u).is_err(), "{u}");
        }
    }

    #[test]
    fn whitespace_and_control_characters_are_rejected_anywhere() {
        for u in ["https://a.com/\n", "https://a.com/\t", "https://a.com/\0", " https://a.com", "https://a.com ", "https://a\r.com", "https://a.com/\u{7f}", "https://a.com/\u{a0}"] {
            assert!(host(u).is_err(), "{u:?}");
        }
    }

    #[test]
    fn non_http_schemes_are_rejected_with_a_named_error() {
        assert_eq!(host("ftp://a.com").unwrap_err(), "unsupported scheme 'ftp'");
        assert_eq!(host("mailto://a@b.com").unwrap_err(), "unsupported scheme 'mailto'");
        assert_eq!(host("a.com/x").unwrap_err(), "not an http(s) URL");
        assert_eq!(host("").unwrap_err(), "not an http(s) URL");
        assert_eq!(host("javascript:alert(1)").unwrap_err(), "not an http(s) URL");
        assert_eq!(host("https://").unwrap_err(), "URL has no host");
    }

    #[test]
    fn percent_encoding_and_unicode_pass_through_untouched() {
        assert_eq!(host("https://a.com/%20x").unwrap(), "a.com");
        // only ASCII is case-folded; IDN hosts are matched as written
        assert_eq!(host("https://BÜCHER.de/").unwrap(), "bÜcher.de");
    }
}
