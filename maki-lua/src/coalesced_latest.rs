use std::sync::{Arc, Mutex};

pub(crate) struct CoalescedLatest<T> {
    inner: Arc<Inner<T>>,
}

struct Inner<T> {
    state: Mutex<State<T>>,
    dispatch: Box<dyn Fn(CoalescedWork<T>) -> bool + Send + Sync>,
}

struct State<T> {
    active: bool,
    pending: Option<T>,
    closed: bool,
}

pub(crate) struct CoalescedWork<T> {
    value: Option<T>,
    inner: Arc<Inner<T>>,
}

impl<T> CoalescedLatest<T> {
    pub(crate) fn new(dispatch: impl Fn(CoalescedWork<T>) -> bool + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    active: false,
                    pending: None,
                    closed: false,
                }),
                dispatch: Box::new(dispatch),
            }),
        }
    }

    pub(crate) fn submit(&self, value: T) -> bool {
        let mut state = self.inner.state.lock().unwrap();
        if state.closed {
            return false;
        }
        if state.active {
            state.pending = Some(value);
            return true;
        }
        state.active = true;
        drop(state);
        let work = CoalescedWork {
            value: Some(value),
            inner: Arc::clone(&self.inner),
        };
        if (self.inner.dispatch)(work) {
            true
        } else {
            let mut state = self.inner.state.lock().unwrap();
            state.closed = true;
            state.active = false;
            state.pending = None;
            false
        }
    }
}

impl<T> Clone for CoalescedLatest<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> CoalescedWork<T> {
    pub(crate) fn value(&self) -> &T {
        self.value.as_ref().unwrap()
    }

    pub(crate) fn finish(mut self, deliver: impl FnOnce(T)) {
        let value = self.value.take().unwrap();
        let mut state = self.inner.state.lock().unwrap();
        let next = state.pending.take();
        if next.is_none() {
            state.active = false;
        }
        drop(state);

        if let Some(next) = next {
            drop(value);
            let work = Self {
                value: Some(next),
                inner: Arc::clone(&self.inner),
            };
            if !(self.inner.dispatch)(work) {
                self.close();
            }
        } else {
            deliver(value);
        }
    }

    fn close(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.closed = true;
        state.active = false;
        state.pending = None;
    }
}

impl<T> Drop for CoalescedWork<T> {
    fn drop(&mut self) {
        if self.value.is_some() {
            self.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_pending_replaces_older_and_stale_active_is_suppressed() {
        let (tx, rx) = flume::unbounded();
        let latest = CoalescedLatest::new(move |work| tx.send(work).is_ok());

        assert!(latest.submit(1));
        assert!(latest.submit(2));
        assert!(latest.submit(3));
        let first = rx.recv().unwrap();
        let mut delivered = Vec::new();
        first.finish(|value| delivered.push(value));
        let last = rx.recv().unwrap();
        last.finish(|value| delivered.push(value));

        assert_eq!(delivered, vec![3]);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn clones_share_active_and_pending_slots() {
        let (tx, rx) = flume::unbounded();
        let latest = CoalescedLatest::new(move |work| tx.send(work).is_ok());
        let clone = latest.clone();

        assert!(latest.submit(1));
        assert!(clone.submit(2));
        assert_eq!(rx.len(), 1);
        rx.recv().unwrap().finish(drop);
        assert_eq!(*rx.recv().unwrap().value(), 2);
    }

    #[test]
    fn dropped_active_work_closes_transport_and_drops_pending() {
        let (tx, rx) = flume::unbounded();
        let latest = CoalescedLatest::new(move |work| tx.send(work).is_ok());

        assert!(latest.submit(1));
        assert!(latest.submit(2));
        drop(rx.recv().unwrap());

        assert!(!latest.submit(3));
        assert!(rx.try_recv().is_err());
    }
}
