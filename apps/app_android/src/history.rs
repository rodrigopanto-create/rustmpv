//! Historial de reproduccion con reanudacion, guardado en `files/history.json`.

use crate::playback::Playback;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const LIMIT: usize = 50;
/// Saltos menores no merecen reanudar; cerca del final se considera visto.
const MIN_RESUME: f64 = 15.0;
const END_MARGIN: f64 = 15.0;
const SAVE_EVERY: f64 = 5.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub input: String,
    pub audio_only: bool,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub position: f64,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub updated: u64,
}

impl Entry {
    pub fn resume_position(&self) -> Option<f64> {
        let duration = self
            .duration
            .filter(|value| value.is_finite() && *value > 0.0)?;
        (self.position.is_finite()
            && self.position >= MIN_RESUME
            && self.position < duration - END_MARGIN
            && self.position < duration * 0.95)
            .then_some(self.position)
    }

    pub fn finished(&self) -> bool {
        self.duration
            .is_some_and(|duration| duration > 0.0 && self.position >= duration - END_MARGIN)
    }

    pub fn label(&self) -> &str {
        if self.title.is_empty() {
            &self.input
        } else {
            &self.title
        }
    }
}

/// Un mismo video de YouTube comparte posicion aunque cambie el formato del enlace.
pub fn key(input: &str) -> String {
    let input = input.trim();
    youtube_id(input)
        .map(|id| format!("youtube:{id}"))
        .unwrap_or_else(|| input.to_owned())
}

fn youtube_id(input: &str) -> Option<String> {
    let url = url::Url::parse(input).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let host = host.strip_prefix("m.").unwrap_or(host);
    let id = match host {
        "youtu.be" => url.path_segments()?.next()?.to_owned(),
        "youtube.com" | "music.youtube.com" | "youtube-nocookie.com" => {
            if url.path() == "/watch" {
                url.query_pairs()
                    .find(|(name, _)| name == "v")
                    .map(|(_, value)| value.into_owned())?
            } else {
                let mut segments = url.path_segments()?;
                match segments.next()? {
                    "shorts" | "live" | "embed" | "v" => segments.next()?.to_owned(),
                    _ => return None,
                }
            }
        }
        _ => return None,
    };
    (id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then_some(id)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct History {
    entries: Vec<Entry>,
}

impl History {
    #[cfg(test)]
    fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.key == key)
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.key == key)
    }

    /// Mueve la entrada al principio y devuelve donde reanudar.
    fn record(&mut self, mut entry: Entry) -> Option<f64> {
        let previous = self
            .entries
            .iter()
            .position(|old| old.key == entry.key)
            .map(|index| self.entries.remove(index));
        let resume = previous.as_ref().and_then(Entry::resume_position);
        if let Some(previous) = previous {
            if entry.title.is_empty() {
                entry.title = previous.title;
                entry.artist = previous.artist;
            }
            entry.position = resume.unwrap_or(0.0);
            entry.duration = previous.duration;
        }
        self.entries.insert(0, entry);
        self.entries.truncate(LIMIT);
        resume
    }
}

struct Tracker {
    history: History,
    path: Option<PathBuf>,
    current: Option<String>,
    saved_position: Option<f64>,
    revision: u64,
}

impl Tracker {
    fn new(path: Option<PathBuf>) -> Self {
        let history = path
            .as_deref()
            .and_then(|path| match std::fs::read_to_string(path) {
                Ok(text) => serde_json::from_str(&text)
                    .map_err(|error| log::error!("Historial danado, se reinicia: {error}"))
                    .ok(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    log::error!("Leer historial: {error}");
                    None
                }
            })
            .unwrap_or_default();
        Self {
            history,
            path,
            current: None,
            saved_position: None,
            revision: 1,
        }
    }

    fn save(&mut self) {
        self.revision += 1;
        let Some(path) = &self.path else {
            return;
        };
        if let Err(error) = write_atomic(path, &self.history) {
            log::error!("Guardar historial: {error}");
        }
    }

    fn start(&mut self, entry: Entry) -> Option<f64> {
        let key = entry.key.clone();
        let resume = self.history.record(entry);
        self.current = Some(key);
        self.saved_position = resume;
        self.save();
        resume
    }

    fn observe(&mut self, playback: &Playback, now: u64) {
        let Some(key) = self.current.clone() else {
            return;
        };
        let Some(entry) = self.history.get_mut(&key) else {
            self.current = None;
            return;
        };
        if playback.ended {
            if let Some(duration) = entry.duration {
                entry.position = duration;
            }
            entry.updated = now;
            self.current = None;
            self.save();
            return;
        }
        if playback.stopped || playback.error.is_some() || !playback.active {
            self.current = None;
            if self.saved_position != Some(entry.position) {
                self.save();
            }
            return;
        }
        if !(playback.loaded && playback.started) {
            return;
        }
        if let Some(duration) = playback.duration.filter(|value| value.is_finite()) {
            entry.duration = Some(duration);
        }
        let Some(position) = playback.position.filter(|value| value.is_finite()) else {
            return;
        };
        entry.position = position;
        entry.updated = now;
        let moved = self
            .saved_position
            .is_none_or(|saved| (position - saved).abs() >= SAVE_EVERY);
        let paused_unsaved = playback.paused && self.saved_position != Some(position);
        if moved || paused_unsaved {
            self.saved_position = Some(position);
            self.save();
        }
    }

    fn remove(&mut self, key: &str) {
        self.history.entries.retain(|entry| entry.key != key);
        if self.current.as_deref() == Some(key) {
            self.current = None;
        }
        self.save();
    }

    fn clear(&mut self) {
        self.history.entries.clear();
        self.current = None;
        self.save();
    }
}

fn write_atomic(path: &Path, history: &History) -> Result<(), String> {
    let text = serde_json::to_string(history).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

#[cfg(target_os = "android")]
mod native {
    use super::{Entry, Playback, Tracker, key};
    use std::path::Path;
    use std::sync::Mutex;

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_secs())
    }

    static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);

    fn with<T>(action: impl FnOnce(&mut Tracker) -> T) -> Option<T> {
        TRACKER.lock().expect("history lock").as_mut().map(action)
    }

    /// Carga el historial; sin directorio solo se conserva en memoria.
    pub fn init(directory: Option<&Path>) {
        let tracker = Tracker::new(directory.map(|directory| directory.join("history.json")));
        log::info!("Historial: {} entradas", tracker.history.entries.len());
        *TRACKER.lock().expect("history lock") = Some(tracker);
    }

    /// Registra el medio que empieza y devuelve la posicion donde reanudar.
    pub fn start(input: &str, audio_only: bool, title: &str, artist: &str) -> Option<f64> {
        let entry = Entry {
            key: key(input),
            input: input.trim().to_owned(),
            audio_only,
            title: title.to_owned(),
            artist: artist.to_owned(),
            position: 0.0,
            duration: None,
            updated: now(),
        };
        with(|tracker| tracker.start(entry)).flatten()
    }

    /// Llamado tras cada `Playback::poll`, tambien en segundo plano.
    pub fn observe(playback: &Playback) {
        with(|tracker| tracker.observe(playback, now()));
    }

    /// Cambia cuando el historial cambia, para no copiar la lista en cada frame.
    pub fn revision() -> u64 {
        with(|tracker| tracker.revision).unwrap_or(0)
    }

    pub fn entries() -> Vec<Entry> {
        with(|tracker| tracker.history.entries.clone()).unwrap_or_default()
    }

    pub fn remove(key: &str) {
        with(|tracker| tracker.remove(key));
    }

    pub fn clear() {
        with(Tracker::clear);
    }
}

#[cfg(target_os = "android")]
pub use native::{clear, entries, init, observe, remove, revision, start};

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(input: &str) -> Entry {
        Entry {
            key: key(input),
            input: input.to_owned(),
            audio_only: true,
            title: "Titulo".to_owned(),
            artist: String::new(),
            position: 0.0,
            duration: None,
            updated: 1,
        }
    }

    fn playing(position: f64, duration: f64) -> Playback {
        Playback {
            active: true,
            loaded: true,
            started: true,
            position: Some(position),
            duration: Some(duration),
            ..Playback::default()
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "visualrustmpv-history-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("history.json")
    }

    #[test]
    fn youtube_links_share_one_key() {
        let expected = "youtube:jNQXAC9IVRw";
        for input in [
            "https://www.youtube.com/watch?v=jNQXAC9IVRw&t=4",
            "https://youtu.be/jNQXAC9IVRw?si=abc",
            "https://m.youtube.com/watch?v=jNQXAC9IVRw",
            "https://music.youtube.com/watch?v=jNQXAC9IVRw&list=x",
            "https://youtube.com/shorts/jNQXAC9IVRw",
            " https://www.youtube.com/live/jNQXAC9IVRw ",
        ] {
            assert_eq!(key(input), expected, "{input}");
        }
        assert_eq!(
            key(" https://cdn.example/a.mp3 "),
            "https://cdn.example/a.mp3"
        );
        assert_eq!(
            key("https://www.youtube.com/watch?v=short"),
            "https://www.youtube.com/watch?v=short"
        );
        assert_eq!(
            key("https://evil.example/watch?v=jNQXAC9IVRw"),
            "https://evil.example/watch?v=jNQXAC9IVRw"
        );
    }

    #[test]
    fn resumes_only_meaningful_unfinished_positions() {
        let mut item = entry("a");
        item.duration = Some(600.0);
        for (position, expected) in [
            (5.0, None),
            (15.0, Some(15.0)),
            (300.0, Some(300.0)),
            (580.0, None),
            (600.0, None),
        ] {
            item.position = position;
            assert_eq!(item.resume_position(), expected, "{position}");
        }
        item.position = 120.0;
        item.duration = None;
        assert_eq!(
            item.resume_position(),
            None,
            "live streams start at the edge"
        );
        item.duration = Some(f64::INFINITY);
        assert_eq!(item.resume_position(), None);
    }

    #[test]
    fn records_move_to_front_dedupe_and_cap() {
        let mut history = History::default();
        for index in 0..LIMIT + 5 {
            history.record(entry(&format!("https://cdn.example/{index}")));
        }
        assert_eq!(history.entries.len(), LIMIT);
        history.record(entry("https://youtu.be/jNQXAC9IVRw"));
        history.record(entry("https://cdn.example/54"));
        let mut again = entry("https://www.youtube.com/watch?v=jNQXAC9IVRw");
        again.title.clear();
        history.record(again);
        assert_eq!(history.entries.len(), LIMIT);
        assert_eq!(history.entries[0].key, "youtube:jNQXAC9IVRw");
        assert_eq!(history.entries[0].title, "Titulo", "keeps the known title");
        assert_eq!(history.entries[0].label(), "Titulo");
        history.entries[0].title.clear();
        assert_eq!(
            history.entries[0].label(),
            "https://www.youtube.com/watch?v=jNQXAC9IVRw"
        );
        assert_eq!(history.entries[1].input, "https://cdn.example/54");
    }

    #[test]
    fn tracker_saves_progress_and_resumes_after_restart() {
        let path = temp_path("resume");
        let mut tracker = Tracker::new(Some(path.clone()));
        assert_eq!(tracker.start(entry("https://youtu.be/jNQXAC9IVRw")), None);
        let revision = tracker.revision;
        tracker.observe(&playing(1.0, 600.0), 10);
        assert_eq!(tracker.revision, revision + 1, "first position is saved");
        tracker.observe(&playing(3.0, 600.0), 11);
        assert_eq!(tracker.revision, revision + 1, "small moves are throttled");
        tracker.observe(&playing(125.5, 600.0), 12);
        let mut paused = playing(127.0, 600.0);
        paused.paused = true;
        tracker.observe(&paused, 13);
        tracker.observe(&paused, 14);
        assert_eq!(tracker.revision, revision + 3, "pause saves once");

        let mut stopped = Playback::default();
        stopped.stop();
        tracker.observe(&stopped, 15);
        assert!(tracker.current.is_none());

        let mut restarted = Tracker::new(Some(path.clone()));
        let entry_after = restarted.history.get("youtube:jNQXAC9IVRw").unwrap();
        assert_eq!(entry_after.position, 127.0);
        assert_eq!(entry_after.duration, Some(600.0));
        let resume = restarted.start(entry("https://www.youtube.com/watch?v=jNQXAC9IVRw"));
        assert_eq!(resume, Some(127.0));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn finished_media_restarts_from_the_beginning() {
        let mut tracker = Tracker::new(None);
        tracker.start(entry("https://cdn.example/a.mp3"));
        tracker.observe(&playing(200.0, 600.0), 1);
        tracker.observe(
            &Playback {
                ended: true,
                ..Playback::default()
            },
            2,
        );
        let item = tracker.history.get("https://cdn.example/a.mp3").unwrap();
        assert!(item.finished());
        assert_eq!(tracker.start(entry("https://cdn.example/a.mp3")), None);
        assert_eq!(tracker.history.entries[0].position, 0.0);
    }

    #[test]
    fn loading_and_errors_never_overwrite_the_saved_position() {
        let mut tracker = Tracker::new(None);
        tracker.start(entry("https://cdn.example/a.mp3"));
        tracker.observe(&playing(200.0, 600.0), 1);
        let mut loading = Playback::default();
        loading.loading();
        tracker.observe(&loading, 2);
        tracker.observe(
            &Playback {
                error: Some("403".to_owned()),
                position: Some(0.0),
                ..Playback::default()
            },
            3,
        );
        assert!(tracker.current.is_none());
        assert_eq!(tracker.history.entries[0].position, 200.0);
        tracker.observe(&playing(0.0, 600.0), 4);
        assert_eq!(
            tracker.history.entries[0].position, 200.0,
            "no current item"
        );
    }

    #[test]
    fn corrupt_file_starts_empty_and_remove_clear_work() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{not json").unwrap();
        let mut tracker = Tracker::new(Some(path.clone()));
        assert!(tracker.history.entries.is_empty());
        tracker.start(entry("https://cdn.example/a"));
        tracker.start(entry("https://cdn.example/b"));
        tracker.remove("https://cdn.example/b");
        assert!(tracker.current.is_none());
        assert_eq!(Tracker::new(Some(path.clone())).history.entries.len(), 1);
        tracker.clear();
        assert!(Tracker::new(Some(path.clone())).history.entries.is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
