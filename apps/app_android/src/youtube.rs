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

#[cfg(test)]
pub fn personalized(input: &str) -> bool {
    playlist_id(input).is_some_and(|id| {
        let public_radio = ["RDAMVM", "RD"]
            .iter()
            .any(|prefix| id.strip_prefix(prefix).is_some_and(|seed| seed.len() == 11));
        (id.starts_with("RD") && !public_radio) || matches!(id.as_str(), "WL" | "LL" | "LM")
    })
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
            "https://www.youtube.com/playlist?list=WL",
            "https://www.youtube.com/playlist?list=LL",
            "https://music.youtube.com/playlist?list=LM",
            "https://youtu.be/abcdefghijk?list=RDMM",
            "https://music.youtube.com/browse/VLRDTMAK123",
        ] {
            assert!(personalized(input), "{input}");
        }
        for input in [
            "https://youtube.com/playlist?list=RD3LXr89pMU2Q&playnext=1&si=shared",
            "https://music.youtube.com/watch?v=abcdefghijk&list=RDAMVMabcdefghijk",
            "https://youtu.be/abcdefghijk?list=RDabcdefghijk&index=3",
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

    #[test]
    fn public_radios_remain_collections_without_requiring_account_mode() {
        for prefix in ["RD", "RDAMVM"] {
            for seed in ["3LXr89pMU2Q", "abcde-gh_12"] {
                let input = format!("https://youtube.com/playlist?list={prefix}{seed}");
                assert!(collection(&input), "{input}");
                assert!(!personalized(&input), "{input}");
            }
        }
        for id in ["RDMM", "RDTMAK123", "RDabcdefghij", "RDabcdefghijkl"] {
            let input = format!("https://music.youtube.com/playlist?list={id}");
            assert!(personalized(&input), "{input}");
        }
    }
}
