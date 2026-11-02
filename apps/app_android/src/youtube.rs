pub fn playlist_id(input: &str) -> Option<String> {
    let url = url::Url::parse(input.trim()).ok()?;
    let host = url.host_str()?.trim_end_matches('.');
    if !matches!(url.scheme(), "http" | "https")
        || !["youtu.be", "youtube.com", "youtube-nocookie.com"]
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
        return None;
    }
    let id = url
        .query_pairs()
        .find(|(key, value)| key == "list" && !value.is_empty())
        .map(|(_, value)| value.into_owned())
        .or_else(|| url.path().strip_prefix("/browse/VL").map(str::to_owned))?;
    (!id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    .then_some(id)
}

pub fn personalized(input: &str) -> bool {
    playlist_id(input)
        .is_some_and(|id| id.starts_with("RD") || matches!(id.as_str(), "WL" | "LL" | "LM"))
}

pub fn collection(input: &str) -> bool {
    playlist_id(input).is_some()
        || url::Url::parse(input.trim()).ok().is_some_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str() == Some("music.youtube.com")
                && url.path().starts_with("/browse/MP")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_music_playlists_and_personal_mixes_without_matching_other_hosts() {
        for input in [
            "https://music.youtube.com/watch?v=abcdefghijk&list=RDAMVMabcdefghijk",
            "https://www.youtube.com/playlist?list=WL",
            "https://music.youtube.com/playlist?list=LM",
            "https://youtu.be/abcdefghijk?list=RDMM",
            "https://music.youtube.com/browse/VLRDTMAK123",
        ] {
            assert!(personalized(input), "{input}");
        }
        for input in [
            "https://music.youtube.com/playlist?list=PL123",
            "https://music.youtube.com/browse/MPREb123",
            "https://youtube.com/watch?v=abcdefghijk",
            "https://youtube.com.evil.example/playlist?list=RDMM",
            "https://example.com/playlist?list=WL",
        ] {
            assert!(!personalized(input), "{input}");
        }
        assert_eq!(
            playlist_id("https://music.youtube.com/browse/VLPL123"),
            Some("PL123".into())
        );
        assert!(collection("https://music.youtube.com/browse/MPREb123"));
        assert!(!collection(
            "https://music.youtube.com.evil.example/browse/MPREb123"
        ));
        assert_eq!(
            playlist_id("https://youtube.com/playlist?list=RD%0Asecret"),
            None
        );
    }
}
