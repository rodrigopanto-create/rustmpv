use crate::{playback::Playback, stream_url::MediaInfo};
use core_logic::PlaybackMode;
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, TryRecvError};

pub enum Update {
    Started(String),
    Loaded(MediaInfo),
    Extended(MediaInfo),
    QueueFailed(String),
    Failed(String),
    Cancelled,
}

struct Pending {
    request: Request,
    receiver: Receiver<Result<MediaInfo, String>>,
}

pub struct Session {
    pub input: String,
    pub mode: PlaybackMode,
    media: MediaInfo,
    policy: Recovery,
    pending: Option<Pending>,
    blocked: bool,
    error: Option<String>,
    updates: VecDeque<Update>,
    queue: Option<Receiver<Result<MediaInfo, String>>>,
}

impl Session {
    pub fn new(input: String, mode: PlaybackMode, media: MediaInfo) -> Self {
        Self {
            input,
            mode,
            media,
            policy: Recovery::default(),
            pending: None,
            blocked: false,
            error: None,
            updates: VecDeque::new(),
            queue: None,
        }
    }

    pub fn pending(&self) -> Option<&Request> {
        self.pending.as_ref().map(|pending| &pending.request)
    }

    pub fn terminal_error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn set_paused(&mut self, paused: bool) {
        if let Some(pending) = &mut self.pending {
            pending.request.paused = paused;
        }
    }

    pub fn cancel(&mut self) {
        self.blocked = true;
        self.queue = None;
    }

    pub fn continue_queue(&mut self, receiver: Receiver<Result<MediaInfo, String>>) {
        self.queue = Some(receiver);
    }

    pub fn queue_pending(&self) -> bool {
        self.queue.is_some() && !self.blocked
    }

    pub fn discard_queue(&mut self) {
        self.queue = None;
    }

    pub fn poll_queue(
        &mut self,
        playback: &Playback,
        mut append: impl FnMut(&str) -> Result<(), String>,
    ) -> bool {
        let mut appended = false;
        if self.blocked || playback.stopped {
            self.queue = None;
            return false;
        }
        while let Some(receiver) = &self.queue {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    Err("El worker termino antes de completar la cola".to_owned())
                }
            };
            let result = result.and_then(|media| {
                let old = std::iter::once(&self.media.url).chain(self.media.playlist.iter());
                let new = std::iter::once(&media.url).chain(media.playlist.iter());
                let count = self.media.playlist.len() + 1;
                if media.playlist.len() < self.media.playlist.len()
                    || old.zip(new).any(|(old, new)| old != new)
                    || !media.entry_ids.starts_with(&self.media.entry_ids)
                    || media.used_cookies != self.media.used_cookies
                {
                    return Err("La cola cambio de orden o sesion durante la resolucion".to_owned());
                }
                for url in media.playlist.iter().skip(count - 1) {
                    append(url)?;
                    appended = true;
                }
                Ok(media)
            });
            match result {
                Ok(media) => {
                    if !media.playlist_pending {
                        self.queue = None;
                    }
                    self.media = media.clone();
                    if matches!(self.updates.back(), Some(Update::Extended(_))) {
                        self.updates.pop_back();
                    }
                    self.updates.push_back(Update::Extended(media));
                }
                Err(error) => {
                    self.queue = None;
                    log::error!("Completar cola: {error}");
                    self.updates.push_back(Update::QueueFailed(format!(
                        "No se pudo completar la cola; se conservan los elementos listos: {error}"
                    )));
                }
            }
        }
        appended
    }

    pub fn take_updates(&mut self) -> Vec<Update> {
        self.updates.drain(..).collect()
    }

    fn fail(&mut self, playback: &mut Playback, error: String) {
        log::error!("Recuperar stream: {error}");
        self.blocked = true;
        self.error = Some(error.clone());
        playback.stream_failure = false;
        playback.fail(error.clone());
        self.updates.push_back(Update::Failed(error));
    }

    pub fn tick(
        &mut self,
        playback: &mut Playback,
        submit: impl FnOnce(
            &str,
            PlaybackMode,
            bool,
        ) -> Result<Receiver<Result<MediaInfo, String>>, String>,
        stop: impl FnOnce() -> Result<(), String>,
        load: impl FnOnce(&MediaInfo, PlaybackMode, Option<f64>, bool) -> Result<(), String>,
    ) {
        if let Some(pending) = &self.pending {
            let cancelled = self.blocked || pending.request.cancelled(playback);
            let result = match pending.receiver.try_recv() {
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err("El worker no respondio".to_owned()),
                Ok(result) => result,
            };
            let pending = self.pending.take().expect("pending retry");
            if cancelled {
                if let Err(error) = result {
                    log::error!("Reintento cancelado termino con error: {error}");
                }
                self.blocked = true;
                self.updates.push_back(Update::Cancelled);
                return;
            }
            let request = pending.request;
            let result = result
                .and_then(|media| request.select(media))
                .and_then(|media| {
                    load(&media, self.mode, request.position, request.paused)?;
                    Ok(media)
                });
            match result {
                Ok(media) => {
                    playback.loading();
                    playback.paused = request.paused;
                    self.media = media.clone();
                    self.updates.push_back(Update::Loaded(media));
                }
                Err(error) => {
                    let mut error = format!(
                        "Stream: {}. Reintento: {}",
                        request.error,
                        crate::user_facing_error(&error)
                    );
                    if let Err(stop_error) = stop() {
                        error.push_str(&format!(". Detener stream: {stop_error}"));
                    }
                    self.fail(playback, error);
                }
            }
            return;
        }
        if self.blocked || playback.stopped || playback.error.is_none() {
            return;
        }
        let Some(request) = self.policy.next(playback, &self.media) else {
            let mut error = playback.status();
            if let Err(stop_error) = stop() {
                error.push_str(&format!(". Detener stream: {stop_error}"));
            }
            self.fail(playback, error);
            return;
        };
        let result = stop().and_then(|()| submit(&self.input, self.mode, request.cookies_only));
        match result {
            Ok(receiver) => {
                playback.stream_failure = false;
                playback.error = None;
                let notice = if request.cookies_only {
                    "El stream fallo; reintentando con cookies guardadas..."
                } else {
                    "El stream fallo; renovando el enlace sin cuenta..."
                };
                log::info!("{notice}");
                self.updates.push_back(Update::Started(notice.to_owned()));
                self.pending = Some(Pending { request, receiver });
            }
            Err(error) => self.fail(
                playback,
                format!("Stream: {}. Reintento: {error}", request.error),
            ),
        }
    }
}

#[derive(Default)]
pub struct Recovery {
    attempts: u8,
    resume_position: Option<f64>,
}

#[derive(Clone)]
pub struct Request {
    pub cookies_only: bool,
    pub position: Option<f64>,
    pub paused: bool,
    pub error: String,
    pub cancel_generation: u64,
    entry_id: Option<String>,
    playlist_position: usize,
}

impl Recovery {
    #[cfg(test)]
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn next(&mut self, playback: &Playback, media: &MediaInfo) -> Option<Request> {
        if !playback.stream_failure || playback.stopped || self.attempts >= 2 {
            return None;
        }
        let error = playback.error.clone()?;
        let cookies_only = media.used_cookies || self.attempts > 0;
        let position = playback
            .position
            .or(playback.last_position)
            .filter(|position| position.is_finite() && *position > 0.0)
            .or(self.resume_position);
        // An immediately rejected refreshed URL must not lose the original resume point.
        self.resume_position = position;
        // Account streams get one fresh account resolution, not an anonymous loop.
        self.attempts = if cookies_only { 2 } else { 1 };
        Some(Request {
            cookies_only,
            position,
            paused: playback.paused,
            error,
            cancel_generation: playback.cancel_generation,
            entry_id: media
                .entry_ids
                .get(playback.playlist_position)
                .filter(|id| !id.is_empty())
                .cloned(),
            playlist_position: playback.playlist_position,
        })
    }
}

impl Request {
    pub fn cancelled(&self, playback: &Playback) -> bool {
        self.cancel_generation != playback.cancel_generation
    }

    pub fn select(&self, mut media: MediaInfo) -> Result<MediaInfo, String> {
        let index = if let Some(id) = &self.entry_id {
            media
                .entry_ids
                .iter()
                .position(|entry| entry == id)
                .ok_or_else(|| {
                    "El elemento que fallo ya no esta disponible en la lista".to_owned()
                })?
        } else if self.playlist_position == 0 && media.playlist.is_empty() {
            0
        } else {
            return Err(
                "No se puede identificar con seguridad el elemento de la lista que fallo"
                    .to_owned(),
            );
        };
        if index > 0 {
            if index > media.playlist.len() {
                return Err("Posicion de lista recuperada no valida".to_owned());
            }
            media.url = media.playlist[index - 1].clone();
            media.playlist.drain(..index);
            media.entry_ids.drain(..index);
        }
        Ok(media)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn failure() -> Playback {
        let mut playback = Playback::default();
        playback.fail_stream("loading failed".to_owned());
        playback
    }

    fn session() -> Session {
        Session::new(
            "https://youtu.be/RecoverTest1".to_owned(),
            PlaybackMode::Audio,
            MediaInfo::default(),
        )
    }

    fn queue_media(ids: &[&str], pending: bool) -> MediaInfo {
        MediaInfo {
            url: format!("https://cdn.example/{}", ids[0]),
            playlist: ids[1..]
                .iter()
                .map(|id| format!("https://cdn.example/{id}"))
                .collect(),
            entry_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
            playlist_pending: pending,
            ..MediaInfo::default()
        }
    }

    #[test]
    fn progressive_snapshots_append_only_the_new_suffix_without_reloading() {
        let mut session = Session::new(
            "list".into(),
            PlaybackMode::Audio,
            queue_media(&["a"], true),
        );
        let playback = Playback {
            active: true,
            paused: true,
            position: Some(12.0),
            ..Playback::default()
        };
        let (sender, receiver) = mpsc::channel();
        session.continue_queue(receiver);
        assert!(session.queue_pending());
        let mut appended = Vec::new();
        sender.send(Ok(queue_media(&["a", "b"], true))).unwrap();
        session.poll_queue(&playback, |url| {
            appended.push(url.to_owned());
            Ok(())
        });
        assert!(session.queue_pending());
        sender
            .send(Ok(queue_media(&["a", "b", "c"], false)))
            .unwrap();
        session.poll_queue(&playback, |url| {
            appended.push(url.to_owned());
            Ok(())
        });
        assert_eq!(appended, ["https://cdn.example/b", "https://cdn.example/c"]);
        assert!(!session.queue_pending());
        assert!(playback.paused);
        assert_eq!(playback.position, Some(12.0));
        assert_eq!(session.media.entry_ids, ["a", "b", "c"]);
        assert!(
            matches!(session.take_updates().as_slice(), [Update::Extended(media)] if !media.playlist_pending)
        );
    }

    #[test]
    fn progressive_queue_survives_eof_but_stop_discards_late_items() {
        let mut session = Session::new(
            "list".into(),
            PlaybackMode::Audio,
            queue_media(&["a"], true),
        );
        let (sender, receiver) = mpsc::channel();
        session.continue_queue(receiver);
        let mut playback = Playback {
            ended: true,
            ..Playback::default()
        };
        sender.send(Ok(queue_media(&["a", "b"], true))).unwrap();
        let mut count = 0;
        session.poll_queue(&playback, |_| {
            count += 1;
            Ok(())
        });
        assert_eq!(count, 1);
        assert!(session.queue_pending());
        playback.stop();
        sender
            .send(Ok(queue_media(&["a", "b", "c"], false)))
            .unwrap();
        session.poll_queue(&playback, |_| panic!("late append after Stop"));
        assert!(!session.queue_pending());
    }

    #[test]
    fn progressive_queue_rejects_reordering_session_changes_and_disconnection() {
        for result in [
            Some(Ok(queue_media(&["b", "a"], false))),
            Some(Ok(MediaInfo {
                used_cookies: true,
                ..queue_media(&["a", "b"], false)
            })),
            Some(Err("network failed".into())),
            None,
        ] {
            let mut session = Session::new(
                "list".into(),
                PlaybackMode::Audio,
                queue_media(&["a"], true),
            );
            let (sender, receiver) = mpsc::channel();
            session.continue_queue(receiver);
            if let Some(result) = result {
                sender.send(result).unwrap();
            }
            drop(sender);
            session.poll_queue(&Playback::default(), |_| panic!("invalid append"));
            assert!(!session.queue_pending());
            assert!(
                matches!(session.take_updates().as_slice(), [Update::QueueFailed(error)] if error.contains("conservan"))
            );
            assert_eq!(session.media.entry_ids, ["a"]);
        }
    }

    #[test]
    fn cancelling_or_discarding_a_progressive_queue_never_consumes_late_results() {
        for cancel in [true, false] {
            let mut session = session();
            let (sender, receiver) = mpsc::channel();
            session.continue_queue(receiver);
            if cancel {
                session.cancel();
            } else {
                session.discard_queue();
            }
            assert!(sender.send(Ok(MediaInfo::default())).is_err());
            session.poll_queue(&Playback::default(), |_| panic!("late append"));
            assert!(!session.queue_pending());
        }
    }

    #[test]
    fn service_and_gui_ticks_share_one_pending_request_and_budget() {
        let mut session = session();
        let mut playback = failure();
        let (sender, receiver) = mpsc::channel();
        session.tick(
            &mut playback,
            |input, mode, cookies| {
                assert_eq!(input, "https://youtu.be/RecoverTest1");
                assert_eq!(mode, PlaybackMode::Audio);
                assert!(!cookies);
                Ok(receiver)
            },
            || Ok(()),
            |_, _, _, _| panic!("not ready"),
        );
        for _ in 0..4 {
            session.tick(
                &mut playback,
                |_, _, _| panic!("duplicate extraction"),
                || panic!("duplicate stop"),
                |_, _, _, _| panic!("not ready"),
            );
        }
        sender.send(Ok(MediaInfo::default())).unwrap();
        session.tick(
            &mut playback,
            |_, _, _| panic!("duplicate extraction"),
            || panic!("duplicate stop"),
            |_, _, _, _| Ok(()),
        );
        assert!(playback.active);
        assert!(session.pending().is_none());
        playback.fail_stream("still forbidden".into());
        let (sender, receiver) = mpsc::channel();
        session.tick(
            &mut playback,
            |_, _, cookies| {
                assert!(cookies);
                Ok(receiver)
            },
            || Ok(()),
            |_, _, _, _| panic!("not ready"),
        );
        sender.send(Ok(MediaInfo::default())).unwrap();
        session.tick(
            &mut playback,
            |_, _, _| panic!("duplicate extraction"),
            || panic!("duplicate stop"),
            |_, _, _, _| Ok(()),
        );
        playback.fail_stream("still forbidden".into());
        session.tick(
            &mut playback,
            |_, _, _| panic!("budget exhausted"),
            || Ok(()),
            |_, _, _, _| panic!("budget exhausted"),
        );
        assert!(session.blocked);
        let updates = session.take_updates();
        assert_eq!(updates.len(), 5);
        assert!(matches!(&updates[0], Update::Started(notice) if notice.contains("sin cuenta")));
        assert!(matches!(&updates[1], Update::Loaded(media) if !media.used_cookies));
        assert!(matches!(&updates[4], Update::Failed(error) if error.contains("forbidden")));
        assert!(session.take_updates().is_empty());
    }

    #[test]
    fn pause_controls_during_renewal_are_restored_without_restarting_history() {
        let mut session = session();
        let mut playback = failure();
        playback.last_position = Some(43.5);
        let (sender, receiver) = mpsc::channel();
        session.tick(
            &mut playback,
            |_, _, _| Ok(receiver),
            || Ok(()),
            |_, _, _, _| panic!("not ready"),
        );
        session.set_paused(true);
        sender.send(Ok(MediaInfo::default())).unwrap();
        session.tick(
            &mut playback,
            |_, _, _| panic!("duplicate extraction"),
            || panic!("duplicate stop"),
            |_, mode, position, paused| {
                assert_eq!(mode, PlaybackMode::Audio);
                assert_eq!(position, Some(43.5));
                assert!(paused);
                Ok(())
            },
        );
        assert!(playback.paused);
        assert!(playback.active);
    }

    #[test]
    fn stop_discards_success_and_error_and_prevents_another_renewal() {
        for result in [
            Ok(MediaInfo::default()),
            Err("extraction failed".to_owned()),
        ] {
            let mut session = session();
            let mut playback = failure();
            let (sender, receiver) = mpsc::channel();
            session.tick(
                &mut playback,
                |_, _, _| Ok(receiver),
                || Ok(()),
                |_, _, _, _| panic!("not ready"),
            );
            playback.cancel();
            session.cancel();
            sender.send(result).unwrap();
            session.tick(
                &mut playback,
                |_, _, _| panic!("cancelled extraction"),
                || panic!("cancelled stop"),
                |_, _, _, _| panic!("cancelled result loaded"),
            );
            assert!(playback.stopped);
            assert!(playback.error.is_none());
            assert!(matches!(
                session.take_updates().last(),
                Some(Update::Cancelled)
            ));
        }
    }

    #[test]
    fn late_start_file_after_stop_cannot_restart_recovery() {
        let mut session = session();
        let mut playback = failure();
        playback.cancel();
        session.cancel();
        playback.loading();
        playback.fail_stream("late queued failure".into());
        session.tick(
            &mut playback,
            |_, _, _| panic!("stopped session restarted"),
            || panic!("stopped session restarted"),
            |_, _, _, _| panic!("stopped session restarted"),
        );
        assert!(session.pending().is_none());
    }

    #[test]
    fn disconnected_worker_is_an_explicit_terminal_error() {
        let mut session = session();
        let mut playback = failure();
        let (sender, receiver) = mpsc::channel();
        session.tick(
            &mut playback,
            |_, _, _| Ok(receiver),
            || Ok(()),
            |_, _, _, _| panic!("not ready"),
        );
        drop(sender);
        session.tick(
            &mut playback,
            |_, _, _| panic!("duplicate extraction"),
            || Ok(()),
            |_, _, _, _| panic!("no result"),
        );
        assert!(playback.error.as_ref().unwrap().contains("worker"));
        assert!(session.terminal_error().unwrap().contains("worker"));
        assert!(!playback.stream_failure);
        assert!(session.blocked);
    }

    #[test]
    fn refreshes_guest_then_cookies_and_never_loops() {
        let mut recovery = Recovery::default();
        let media = MediaInfo::direct("https://cdn.example/video".to_owned());
        assert!(!recovery.next(&failure(), &media).unwrap().cookies_only);
        assert!(recovery.next(&failure(), &media).unwrap().cookies_only);
        assert!(recovery.next(&failure(), &media).is_none());
        recovery.reset();
        assert!(recovery.next(&failure(), &media).is_some());
    }

    #[test]
    fn account_stream_is_refreshed_once() {
        let mut recovery = Recovery::default();
        let media = MediaInfo {
            used_cookies: true,
            ..MediaInfo::default()
        };
        assert!(recovery.next(&failure(), &media).unwrap().cookies_only);
        assert!(recovery.next(&failure(), &media).is_none());
    }

    #[test]
    fn immediately_rejected_refresh_preserves_original_resume_point() {
        let mut recovery = Recovery::default();
        let media = MediaInfo::default();
        let mut playback = failure();
        playback.last_position = Some(43.5);
        assert_eq!(
            recovery.next(&playback, &media).unwrap().position,
            Some(43.5)
        );
        assert_eq!(
            recovery.next(&failure(), &media).unwrap().position,
            Some(43.5)
        );
        recovery.reset();
        assert_eq!(recovery.next(&failure(), &media).unwrap().position, None);
    }

    #[test]
    fn controls_and_output_errors_are_not_retried() {
        let mut recovery = Recovery::default();
        let media = MediaInfo::default();
        let mut playback = Playback::default();
        playback.fail("audio output failed".to_owned());
        assert!(recovery.next(&playback, &media).is_none());
        playback = failure();
        playback.stop();
        assert!(recovery.next(&playback, &media).is_none());
        assert!(recovery.next(&Playback::default(), &media).is_none());
    }

    #[test]
    fn preserves_position_pause_and_playlist_identity_after_skipped_entries() {
        let mut recovery = Recovery::default();
        let mut playback = failure();
        playback.playlist_position = 1;
        playback.last_position = Some(43.5);
        playback.paused = true;
        let media = MediaInfo {
            entry_ids: vec!["a".into(), "b".into(), "c".into()],
            ..MediaInfo::default()
        };
        let request = recovery.next(&playback, &media).unwrap();
        assert_eq!(request.position, Some(43.5));
        assert!(request.paused);
        let refreshed = MediaInfo {
            url: "https://cdn.example/x".into(),
            playlist: vec![
                "https://cdn.example/b".into(),
                "https://cdn.example/c".into(),
            ],
            entry_ids: vec!["x".into(), "b".into(), "c".into()],
            ..MediaInfo::default()
        };
        let selected = request.select(refreshed).unwrap();
        assert_eq!(selected.url, "https://cdn.example/b");
        assert_eq!(selected.playlist, vec!["https://cdn.example/c"]);
        assert_eq!(selected.entry_ids, vec!["b", "c"]);
        assert!(request.select(MediaInfo::default()).is_err());
    }

    #[test]
    fn does_not_restart_unidentified_playlist_at_wrong_item() {
        let mut recovery = Recovery::default();
        let request = recovery.next(&failure(), &MediaInfo::default()).unwrap();
        let media = MediaInfo {
            playlist: vec!["https://cdn.example/b".into()],
            ..MediaInfo::default()
        };
        assert!(request.select(media).is_err());
        let mut playback = failure();
        playback.playlist_position = usize::MAX;
        let request = recovery.next(&playback, &MediaInfo::default()).unwrap();
        assert!(request.select(MediaInfo::default()).is_err());
    }

    #[test]
    fn user_stop_cancels_pending_retry_even_across_loading() {
        let mut recovery = Recovery::default();
        let mut playback = failure();
        let request = recovery.next(&playback, &MediaInfo::default()).unwrap();
        assert_eq!(request.error, "loading failed");
        assert!(!request.cancelled(&playback));
        playback.stop();
        assert!(!request.cancelled(&playback)); // Recovery's own stop is not cancellation.
        playback.cancel();
        playback.loading();
        assert!(request.cancelled(&playback));
    }
}
