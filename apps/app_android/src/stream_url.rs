const TEXT_LIMIT: usize = 200;
#[cfg(any(feature = "chaquopy", test))]
const THUMBNAIL_LIMIT: usize = 3;

/// Stream directo mas los metadatos que se muestran en la notificacion.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaInfo {
    pub url: String,
    pub title: String,
    pub artist: String,
    pub thumbnails: Vec<String>,
}

#[cfg(any(feature = "chaquopy", test))]
#[derive(serde::Deserialize)]
struct RawMedia {
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    thumbnails: Vec<String>,
}

impl MediaInfo {
    /// URL directa sin extractor: el titulo sale del nombre del archivo.
    pub fn direct(url: String) -> Self {
        let parsed = url::Url::parse(&url).ok();
        let file = parsed
            .as_ref()
            .and_then(|url| {
                url.path_segments()?
                    .rev()
                    .find(|segment| !segment.is_empty())
            })
            .map(percent_decode)
            .unwrap_or_default();
        let host = parsed
            .as_ref()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        Self {
            url,
            title: clean_text(&file),
            artist: clean_text(&host),
            thumbnails: Vec::new(),
        }
    }

    /// Valida el JSON de `yt_dlp_runner.extract_media`.
    #[cfg(any(feature = "chaquopy", test))]
    pub fn from_json(json: &str) -> Result<Self, String> {
        let raw: RawMedia = serde_json::from_str(json)
            .map_err(|error| format!("Respuesta de yt-dlp invalida: {error}"))?;
        let thumbnails = raw
            .thumbnails
            .iter()
            .filter_map(|value| validate_stream_url(value).ok())
            .filter(|value| value.starts_with("https://"))
            .take(THUMBNAIL_LIMIT)
            .collect();
        Ok(Self {
            url: validate_stream_url(&raw.url)?,
            title: clean_text(&raw.title),
            artist: clean_text(&raw.artist),
            thumbnails,
        })
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes
            .get(index + 1..index + 3)
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                output.push(byte);
                index += 3;
            }
            (byte, _) => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn clean_text(value: &str) -> String {
    value
        .split(|character: char| character.is_control() || character.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(TEXT_LIMIT)
        .collect()
}

pub fn validate_stream_url(output: &str) -> Result<String, String> {
    let lines: Vec<_> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() != 1 {
        return Err("yt-dlp debe devolver exactamente una URL".to_owned());
    }
    let value = lines[0].trim();
    let url = url::Url::parse(value).map_err(|error| format!("URL invalida: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err("yt-dlp no devolvio una URL HTTP(S) valida".to_owned());
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_exactly_one_http_url() {
        assert_eq!(
            validate_stream_url("https://cdn.example/video?a=1&b=2\n").unwrap(),
            "https://cdn.example/video?a=1&b=2"
        );
        for value in [
            "",
            "https://",
            "file:///tmp/x",
            "https://user:pass@host/video",
            "https://a.example\nhttps://b.example",
            "https://host/a\0b",
            "warning\nhttps://host/video",
            "https://host/a b",
        ] {
            assert!(validate_stream_url(value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn media_json_is_validated_and_sanitized() {
        let media = MediaInfo::from_json(
            r#"{"url":"https://cdn.example/a","title":" Big\nBuck\u0000 Bunny ",
                "artist":"Blender","thumbnails":["http://plain/x.jpg","file:///x",
                "https://i.ytimg.com/a.jpg","https://i.ytimg.com/b.jpg",
                "https://i.ytimg.com/c.jpg","https://i.ytimg.com/d.jpg"]}"#,
        )
        .unwrap();
        assert_eq!(media.url, "https://cdn.example/a");
        assert_eq!(media.title, "Big Buck Bunny");
        assert_eq!(media.artist, "Blender");
        assert_eq!(
            media.thumbnails,
            [
                "https://i.ytimg.com/a.jpg",
                "https://i.ytimg.com/b.jpg",
                "https://i.ytimg.com/c.jpg"
            ]
        );
        assert_eq!(
            MediaInfo::from_json(&format!(
                r#"{{"url":"https://a.b","title":"{}"}}"#,
                "x".repeat(300)
            ))
            .unwrap()
            .title
            .len(),
            200
        );
        for invalid in ["", "{}", r#"{"url":"file:///x"}"#, r#"{"url":5}"#] {
            assert!(MediaInfo::from_json(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn direct_media_uses_file_name_and_host() {
        let media = MediaInfo::direct("https://media.example/path/My%20Song+1.mp3?x=1".to_owned());
        assert_eq!(media.title, "My Song+1.mp3");
        assert_eq!(media.artist, "media.example");
        assert!(media.thumbnails.is_empty());
    }
}
