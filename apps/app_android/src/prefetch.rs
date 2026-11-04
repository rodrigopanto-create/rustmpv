//! Resolucion anticipada del enlace mientras el usuario aun no pulsa Play.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Tiempo que la URL debe permanecer igual antes de resolverla (evita teclear letra a letra).
pub const DEBOUNCE: Duration = Duration::from_millis(700);
/// Vigencia de una URL de stream ya resuelta.
pub const TTL: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub input: String,
    pub audio_only: bool,
    pub session: bool,
}

type Reply<T> = Receiver<Result<T, String>>;

enum State<T> {
    /// El `Arc` mantiene viva la peticion en la cola del worker.
    Pending(Reply<T>, Arc<()>),
    Ready(T, Instant, Reply<T>, Arc<()>),
    Failed,
}

pub enum Taken<T> {
    Ready(T, Instant, Reply<T>, Arc<()>),
    Pending(Reply<T>, Arc<()>),
    None,
}

pub struct Prefetcher<T> {
    candidate: Option<(Key, Instant)>,
    current: Option<(Key, State<T>)>,
}

impl<T> Default for Prefetcher<T> {
    fn default() -> Self {
        Self {
            candidate: None,
            current: None,
        }
    }
}

impl<T> Prefetcher<T> {
    pub fn pending(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|(_, state)| matches!(state, State::Pending(..)))
    }

    pub fn ready(&self) -> Option<&T> {
        match self.current.as_ref() {
            Some((_, State::Ready(media, ..))) => Some(media),
            _ => None,
        }
    }

    /// Registra la entrada actual; devuelve la clave a resolver cuando lleva
    /// `DEBOUNCE` sin cambiar y no hay ya una precarga util para ella.
    pub fn observe(&mut self, key: Option<Key>, now: Instant) -> Option<Key> {
        let Some(key) = key else {
            self.candidate = None;
            self.current = None;
            return None;
        };
        if self
            .candidate
            .as_ref()
            .is_none_or(|(candidate, _)| *candidate != key)
        {
            self.candidate = Some((key.clone(), now));
        }
        let since = self.candidate.as_ref().map_or(now, |(_, since)| *since);
        if now.saturating_duration_since(since) < DEBOUNCE {
            return None;
        }
        let covered = self.current.as_ref().is_some_and(|(current, state)| {
            *current == key
                && match state {
                    State::Ready(_, at, ..) => now.saturating_duration_since(*at) < TTL,
                    State::Pending(..) | State::Failed => true,
                }
        });
        (!covered).then_some(key)
    }

    pub fn start(&mut self, key: Key, reply: Reply<T>, wanted: Arc<()>) {
        self.current = Some((key, State::Pending(reply, wanted)));
    }

    /// Marca la clave como fallida para no reintentarla en cada cuadro.
    pub fn fail(&mut self, key: Key) {
        self.current = Some((key, State::Failed));
    }

    /// Recoge la respuesta del worker; devuelve `Some` cuando termina.
    pub fn poll(&mut self, now: Instant) -> Option<Result<(), String>> {
        let (_, state) = self.current.as_mut()?;
        let reply = match state {
            State::Pending(reply, _) | State::Ready(_, _, reply, _) => reply,
            State::Failed => return None,
        };
        let result = match reply.try_recv() {
            Ok(Ok(media)) => {
                let old = std::mem::replace(state, State::Failed);
                let (reply, token) = match old {
                    State::Pending(reply, token) | State::Ready(_, _, reply, token) => {
                        (reply, token)
                    }
                    State::Failed => unreachable!(),
                };
                *state = State::Ready(media, now, reply, token);
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) if matches!(state, State::Ready(..)) => return None,
            Err(TryRecvError::Disconnected) => Err("el worker no respondio".to_owned()),
        };
        if result.is_err() {
            *state = State::Failed;
        }
        Some(result)
    }

    /// Entrega a Play el resultado listo o la peticion en curso para `key`.
    pub fn take(&mut self, key: &Key, now: Instant) -> Taken<T> {
        if self
            .current
            .as_ref()
            .is_none_or(|(current, _)| current != key)
        {
            return Taken::None;
        }
        match self.current.take().map(|(_, state)| state) {
            Some(State::Ready(media, at, reply, token))
                if now.saturating_duration_since(at) < TTL =>
            {
                Taken::Ready(media, at, reply, token)
            }
            Some(State::Pending(reply, wanted)) => Taken::Pending(reply, wanted),
            _ => Taken::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn key(input: &str, audio_only: bool) -> Key {
        Key {
            input: input.to_owned(),
            audio_only,
            session: false,
        }
    }

    #[test]
    fn switching_sites_drops_obsolete_results_and_tokens() {
        let mut prefetch = Prefetcher::default();
        let now = Instant::now();
        for input in [
            "https://youtube.com/watch?v=abcdefghijk",
            "https://x.com/user/status/1",
            "https://instagram.com/reel/abc/",
            "https://facebook.com/user/videos/1/",
        ] {
            let (sender, receiver) = mpsc::channel::<Result<u32, String>>();
            let token = Arc::new(());
            let weak = Arc::downgrade(&token);
            prefetch.start(key(input, true), receiver, token);
            prefetch = Prefetcher::default();
            assert!(weak.upgrade().is_none());
            assert!(sender.send(Ok(7)).is_err());
            assert!(prefetch.poll(now).is_none());
        }
    }

    #[test]
    fn waits_for_stable_input_before_starting() {
        let mut prefetch = Prefetcher::<u32>::default();
        let start = Instant::now();
        assert_eq!(
            prefetch.observe(Some(key("https://youtu.be/a", true)), start),
            None
        );
        let later = start + DEBOUNCE / 2;
        assert_eq!(
            prefetch.observe(Some(key("https://youtu.be/ab", true)), later),
            None
        );
        assert_eq!(
            prefetch.observe(Some(key("https://youtu.be/ab", true)), later + DEBOUNCE / 2),
            None
        );
        assert_eq!(
            prefetch.observe(Some(key("https://youtu.be/ab", true)), later + DEBOUNCE),
            Some(key("https://youtu.be/ab", true))
        );
        assert_eq!(
            prefetch.observe(Some(key("https://youtu.be/ab", false)), later + DEBOUNCE),
            None,
            "changing the mode restarts the debounce"
        );
    }

    #[test]
    fn session_mode_cannot_adopt_an_anonymous_prefetch() {
        let mut prefetch = Prefetcher::default();
        let now = Instant::now();
        let anonymous = key("https://music.youtube.com/playlist?list=PL123", true);
        let (sender, receiver) = mpsc::channel();
        prefetch.start(anonymous.clone(), receiver, Arc::new(()));
        sender.send(Ok(7)).unwrap();
        assert_eq!(prefetch.poll(now), Some(Ok(())));
        let account = Key {
            session: true,
            ..anonymous
        };
        assert!(matches!(prefetch.take(&account, now), Taken::None));
        assert_eq!(prefetch.observe(Some(account.clone()), now), None);
        assert_eq!(
            prefetch.observe(Some(account.clone()), now + DEBOUNCE),
            Some(account)
        );
    }

    #[test]
    fn ready_result_is_taken_once_and_expires() {
        let mut prefetch = Prefetcher::default();
        let start = Instant::now();
        let wanted = key("https://youtu.be/a", true);
        let (sender, receiver) = mpsc::channel();
        assert!(!prefetch.pending());
        prefetch.start(wanted.clone(), receiver, Arc::new(()));
        assert!(prefetch.pending());
        assert_eq!(prefetch.observe(Some(wanted.clone()), start), None);
        assert_eq!(
            prefetch.observe(Some(wanted.clone()), start + DEBOUNCE),
            None
        );
        assert!(prefetch.poll(start).is_none());
        sender.send(Ok(7)).unwrap();
        assert_eq!(prefetch.poll(start), Some(Ok(())));
        assert!(!prefetch.pending());
        assert!(matches!(
            prefetch.take(&key("https://youtu.be/a", false), start),
            Taken::None
        ));
        assert!(matches!(
            prefetch.take(&wanted, start + TTL / 2),
            Taken::Ready(7, at, ..) if at == start
        ));
        assert!(matches!(prefetch.take(&wanted, start), Taken::None));

        let (sender, receiver) = mpsc::channel();
        prefetch.start(wanted.clone(), receiver, Arc::new(()));
        sender.send(Ok(8)).unwrap();
        prefetch.poll(start);
        assert_eq!(
            prefetch.observe(Some(wanted.clone()), start + TTL),
            Some(wanted.clone())
        );
        assert!(matches!(prefetch.take(&wanted, start + TTL), Taken::None));
    }

    #[test]
    fn pending_request_is_handed_to_play_with_its_token() {
        let mut prefetch = Prefetcher::default();
        let wanted = key("https://youtu.be/a", true);
        let (sender, receiver) = mpsc::channel();
        let token = Arc::new(());
        let weak = Arc::downgrade(&token);
        prefetch.start(wanted.clone(), receiver, token);
        let Taken::Pending(receiver, token) = prefetch.take(&wanted, Instant::now()) else {
            panic!("pending request expected");
        };
        assert!(weak.upgrade().is_some());
        sender.send(Ok(1)).unwrap();
        assert_eq!(receiver.recv().unwrap(), Ok(1));
        drop(token);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn ready_prefix_keeps_its_live_receiver_when_adopted_by_play() {
        let mut prefetch = Prefetcher::default();
        let now = Instant::now();
        let wanted = key("https://youtube.com/playlist?list=PL123", true);
        let (sender, receiver) = mpsc::channel();
        let token = Arc::new(());
        let weak = Arc::downgrade(&token);
        prefetch.start(wanted.clone(), receiver, token);
        sender.send(Ok(1)).unwrap();
        assert_eq!(prefetch.poll(now), Some(Ok(())));
        assert_eq!(prefetch.ready(), Some(&1));
        let Taken::Ready(first, _, receiver, token) = prefetch.take(&wanted, now) else {
            panic!("ready prefix expected");
        };
        assert_eq!(first, 1);
        assert!(weak.upgrade().is_some());
        sender.send(Ok(2)).unwrap();
        assert_eq!(receiver.recv().unwrap(), Ok(2));
        drop(token);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn prefetch_retains_the_latest_snapshot_and_reports_a_tail_failure() {
        let mut prefetch = Prefetcher::default();
        let now = Instant::now();
        let (sender, receiver) = mpsc::channel();
        prefetch.start(
            key("https://youtube.com/playlist?list=PL123", true),
            receiver,
            Arc::new(()),
        );
        sender.send(Ok(1)).unwrap();
        prefetch.poll(now);
        sender.send(Ok(2)).unwrap();
        assert_eq!(prefetch.poll(now), Some(Ok(())));
        assert_eq!(prefetch.ready(), Some(&2));
        sender.send(Err("tail failed".into())).unwrap();
        assert_eq!(prefetch.poll(now), Some(Err("tail failed".into())));
    }

    #[test]
    fn failures_are_not_retried_and_clearing_cancels() {
        let mut prefetch = Prefetcher::<u32>::default();
        let start = Instant::now();
        let wanted = key("https://youtu.be/a", true);
        let (sender, receiver) = mpsc::channel();
        let token = Arc::new(());
        let weak = Arc::downgrade(&token);
        prefetch.start(wanted.clone(), receiver, token);
        assert!(prefetch.observe(Some(wanted.clone()), start).is_none());
        prefetch.observe(None, start);
        assert!(
            weak.upgrade().is_none(),
            "clearing the field cancels the queued request"
        );
        drop(sender);

        let (sender, receiver) = mpsc::channel();
        prefetch.start(wanted.clone(), receiver, Arc::new(()));
        sender.send(Err("offline".to_owned())).unwrap();
        assert_eq!(prefetch.poll(start), Some(Err("offline".to_owned())));
        prefetch.observe(Some(wanted.clone()), start);
        assert_eq!(
            prefetch.observe(Some(wanted.clone()), start + DEBOUNCE),
            None
        );
        assert!(matches!(prefetch.take(&wanted, start), Taken::None));

        let other = key("https://youtu.be/b", true);
        prefetch.fail(other.clone());
        prefetch.observe(Some(other.clone()), start);
        assert_eq!(prefetch.observe(Some(other), start + DEBOUNCE), None);
    }
}
