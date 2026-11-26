//! Single-slot mailbox for replaceable worker snapshots.
use std::{
    sync::{Arc, Condvar, Mutex, mpsc},
    time::Duration,
};

#[derive(Debug)]
struct State<T> {
    pending: Option<T>,
    senders: usize,
    receiving: bool,
}
#[derive(Debug)]
struct Shared<T>(Mutex<State<T>>, Condvar);

/// Nonblocking producer. Replaces an unread value rather than queueing it.
#[derive(Debug)]
pub struct Sender<T>(Arc<Shared<T>>);
/// The only consumer of a mailbox.
#[derive(Debug)]
pub struct Receiver<T>(Arc<Shared<T>>);

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared(
        Mutex::new(State {
            pending: None,
            senders: 1,
            receiving: true,
        }),
        Condvar::new(),
    ));
    (Sender(shared.clone()), Receiver(shared))
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.0.0.lock().unwrap().senders += 1;
        Self(self.0.clone())
    }
}
impl<T> Sender<T> {
    /// Discards queued work and wakes the consumer on owner shutdown.
    pub fn cancel(&self) {
        let old = {
            let mut state = self.0.0.lock().unwrap();
            state.receiving = false;
            state.pending.take()
        };
        self.0.1.notify_one();
        drop(old);
    }

    pub fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        let old = {
            let mut state = self.0.0.lock().unwrap();
            if !state.receiving {
                return Err(mpsc::SendError(value));
            }
            state.pending.replace(value)
        };
        self.0.1.notify_one();
        drop(old);
        Ok(())
    }

    /// Coalesces separate kinds of work into one pending value. Return replaced
    /// payloads from the callback to retire them after releasing the lock.
    pub fn update<R>(&self, update: impl FnOnce(&mut T) -> R) -> Result<R, mpsc::SendError<()>>
    where
        T: Default,
    {
        let result = {
            let mut state = self.0.0.lock().unwrap();
            if !state.receiving {
                return Err(mpsc::SendError(()));
            }
            update(state.pending.get_or_insert_with(T::default))
        };
        self.0.1.notify_one();
        Ok(result)
    }
}
impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.0.0.lock().unwrap().senders -= 1;
        self.0.1.notify_one();
    }
}
impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        let mut state = self.0.0.lock().unwrap();
        state
            .pending
            .take()
            .ok_or(if state.receiving && state.senders > 0 {
                mpsc::TryRecvError::Empty
            } else {
                mpsc::TryRecvError::Disconnected
            })
    }
    pub fn recv(&self) -> Result<T, mpsc::RecvError> {
        let state = self.0.0.lock().unwrap();
        let mut state = self.0.1.wait_while(state, empty).unwrap();
        state.pending.take().ok_or(mpsc::RecvError)
    }
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, mpsc::RecvTimeoutError> {
        let state = self.0.0.lock().unwrap();
        let (mut state, _) = self.0.1.wait_timeout_while(state, timeout, empty).unwrap();
        state
            .pending
            .take()
            .ok_or(if state.receiving && state.senders > 0 {
                mpsc::RecvTimeoutError::Timeout
            } else {
                mpsc::RecvTimeoutError::Disconnected
            })
    }
}
fn empty<T>(state: &mut State<T>) -> bool {
    state.pending.is_none() && state.receiving && state.senders > 0
}
impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.0.0.lock().unwrap();
        state.receiving = false;
        let old = state.pending.take();
        drop(state);
        drop(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_snapshots_are_replaced_and_released_on_close() {
        let (tx, rx) = channel();
        let old = Arc::new(0);
        tx.send(old.clone()).unwrap();
        for n in 1..1000 {
            tx.send(Arc::new(n)).unwrap();
        }
        assert_eq!(Arc::strong_count(&old), 1);
        assert_eq!(*rx.try_recv().unwrap(), 999);
        assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        tx.send(old.clone()).unwrap();
        drop(rx);
        assert_eq!(Arc::strong_count(&old), 1);
    }
}
