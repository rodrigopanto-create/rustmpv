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
}

type Reply<T> = Receiver<Result<T, String>>;

enum State<T> {
    /// El `Arc` mantiene viva la peticion en la cola del worker.
    Pending(Reply<T>, Arc<()>),
    Ready(T, Instant),
    Failed,
}

pub enum Taken<T> {
    Ready(T, Instant),
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
                    State::Ready(_, at) => now.saturating_duration_since(*at) < TTL,
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
        let State::Pending(reply, _) = state else {
            return None;
        };
        let result = match reply.try_recv() {
            Ok(Ok(media)) => {
                *state = State::Ready(media, now);
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(TryRecvError::Empty) => return None,
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
            Some(State::Ready(media, at)) if now.saturating_duration_since(at) < TTL => {
                Taken::Ready(media, at)
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
    fn ready_result_is_taken_once_and_expires() {
        let mut prefetch = Prefetcher::default();
        let start = Instant::now();
        let wanted = key("https://youtu.be/a", true);
        let (sender, receiver) = mpsc::channel();
        prefetch.start(wanted.clone(), receiver, Arc::new(()));
        assert_eq!(prefetch.observe(Some(wanted.clone()), start), None);
        assert_eq!(
            prefetch.observe(Some(wanted.clone()), start + DEBOUNCE),
            None
        );
        assert!(prefetch.poll(start).is_none());
        sender.send(Ok(7)).unwrap();
        assert_eq!(prefetch.poll(start), Some(Ok(())));
        assert!(matches!(
            prefetch.take(&key("https://youtu.be/a", false), start),
            Taken::None
        ));
        assert!(matches!(
            prefetch.take(&wanted, start + TTL / 2),
            Taken::Ready(7, at) if at == start
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
