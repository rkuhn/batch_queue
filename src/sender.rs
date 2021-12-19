use super::*;
use futures::Future;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// Error type returned from [`try_send`](struct.Sender.html#method.try_send)
#[derive(Debug, Clone, PartialEq)]
pub enum TrySendError<T> {
    Full(T),
    Closed,
}

/// The sending end of this batching queue
///
/// Since this is a single-producer queue, this handle cannot be cloned or shared.
/// Dropping this handle will eventually lead to the receiver signaling that this
/// queue has been closed.
pub struct Sender<T, const N: usize> {
    inner: Option<Arc<Inner<T, N>>>,
}

impl<T, const N: usize> Drop for Sender<T, N> {
    fn drop(&mut self) {
        let inner = self.inner.take().unwrap();
        inner.do_close_batch();
        inner.writer.waker.take();
        let waker = inner.reader.waker.take();
        // other side will notice by checking Arc ref count
        drop(inner);
        if let Some(waker) = waker {
            // important: wake AFTER dropping the ref count
            waker.wake();
        }
    }
}

unsafe impl<T, const N: usize> Send for Sender<T, N> {}

impl<T, const N: usize> Sender<T, N> {
    pub(crate) fn new(inner: Arc<Inner<T, N>>) -> Self {
        Self { inner: Some(inner) }
    }

    fn inner(&self) -> &Inner<T, N> {
        self.inner.as_ref().unwrap()
    }

    fn strong_count(&self) -> usize {
        Arc::strong_count(self.inner.as_ref().unwrap())
    }

    /// Try sending a value via the queue
    ///
    /// This may return `TrySendError::Closed` in case the receiver has been
    /// dropped (see [`send`](#method.send)). Or it may return `TrySendError::Full()`
    /// to hand you back the value in case it can currently not be accepted
    /// because the queue is full.
    pub fn try_send(&mut self, value: T) -> Result<bool, TrySendError<T>> {
        if self.strong_count() == 1 {
            Err(TrySendError::Closed)
        } else {
            match self.inner().do_send(value) {
                Ok(b) => Ok(b),
                Err(v) => Err(TrySendError::Full(v)),
            }
        }
    }

    pub(crate) fn try_poll(
        &mut self,
        value: T,
        cx: &mut Context<'_>,
    ) -> Result<bool, TrySendError<T>> {
        if self.strong_count() == 1 {
            return Err(TrySendError::Closed);
        }
        match self.inner().do_send(value) {
            Ok(b) => Ok(b),
            Err(v) => {
                self.inner().writer.waker.register(cx.waker());
                // need to check again because the other side may have dropped or consumed before the `register`
                if self.strong_count() == 1 {
                    Err(TrySendError::Closed)
                } else {
                    match self.inner().do_send(v) {
                        Ok(b) => {
                            self.inner().writer.waker.take();
                            Ok(b)
                        }
                        Err(v) => Err(TrySendError::Full(v)),
                    }
                }
            }
        }
    }

    /// Hand the current bucket over to the receiver, even if it is not full
    ///
    /// This is a no-op if the current bucket is empty.
    pub fn close_batch(&mut self) {
        self.inner().do_close_batch()
    }

    /// A future that will eventually send the value
    ///
    /// If the channel is or has been closed, the value is dropped, just as
    /// the values that are in the queue when both ends are dropped.
    ///
    /// The sent value will not be visible to the receiver until the current
    /// bucket is handed over, which happens either when the bucket is full
    /// or [`close_batch`](#method.close_batch) is called.
    pub fn send(&mut self, value: T) -> SendFuture<'_, T, N> {
        SendFuture {
            inner: self.inner.as_ref().unwrap(),
            value: Some(value),
        }
    }
}

/// The Future returned from [`send`](struct.Sender.html#method.send)
///
/// It will resolve once the given item has been sent or the queue is closed
/// (by dropping the receiver).
pub struct SendFuture<'a, T, const N: usize> {
    inner: &'a Arc<Inner<T, N>>,
    value: Option<T>,
}

unsafe impl<'a, T, const N: usize> Send for SendFuture<'a, T, N> {}

impl<'a, T, const N: usize> Future for SendFuture<'a, T, N> {
    type Output = Result<bool, Closed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        if Arc::strong_count(this.inner) == 1 {
            return Poll::Ready(Err(Closed));
        }
        let value = match this.value.take() {
            Some(v) => v,
            None => return Poll::Ready(Ok(false)),
        };
        match this.inner.do_send(value) {
            Ok(b) => Poll::Ready(Ok(b)),
            Err(v) => {
                this.inner.writer.waker.register(cx.waker());
                // need to check again because the other side may have dropped or consumed before the `register`
                if Arc::strong_count(this.inner) == 1 {
                    Poll::Ready(Err(Closed))
                } else {
                    match this.inner.do_send(v) {
                        Ok(b) => {
                            this.inner.writer.waker.take();
                            Poll::Ready(Ok(b))
                        }
                        Err(v) => {
                            this.value = Some(v);
                            Poll::Pending
                        }
                    }
                }
            }
        }
    }
}
