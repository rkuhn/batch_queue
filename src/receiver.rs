use super::*;
use futures::Future;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// Error type returned from [`try_recv`](struct.Receiver.html#method.try_recv)
#[derive(Debug, Clone, PartialEq)]
pub enum TryRecvError {
    Empty,
    Closed,
}

/// The receiving end of this batching queue
///
/// Since this is a single-consume queue, this handle cannot be cloned or shared.
/// Dropping this handle will eventually lead to the sender signaling that this
/// queue has been closed. Items that were in flight will be dropped.
pub struct Receiver<T, const N: usize> {
    inner: Option<Arc<Inner<T, N>>>,
}

impl<T, const N: usize> Drop for Receiver<T, N> {
    fn drop(&mut self) {
        let inner = self.inner.take().unwrap();
        inner.reader.waker.take();
        let waker = inner.writer.waker.take();
        // other side will notice by checking Arc ref count
        drop(inner);
        if let Some(waker) = waker {
            // important: wake AFTER dropping the ref count
            waker.wake();
        }
    }
}

unsafe impl<T, const N: usize> Send for Receiver<T, N> {}

impl<T, const N: usize> Receiver<T, N> {
    pub(crate) fn new(inner: Arc<Inner<T, N>>) -> Self {
        Self { inner: Some(inner) }
    }

    fn inner(&self) -> &Inner<T, N> {
        self.inner.as_ref().unwrap()
    }

    fn strong_count(&self) -> usize {
        Arc::strong_count(self.inner.as_ref().unwrap())
    }

    /// Check if a batch is currently available and fill them into a fresh Vec
    ///
    /// If no batch is available it returns `TryRecvError::Empty`. If no batch will
    /// ever become available because the sender has been dropped it returns
    /// `TryRecvError::Closed`.
    ///
    /// If the next thing you’ll do is to iterate over the vector, prefer
    /// [`try_recv`](#method.try_recv) instead to save one allocation.
    pub fn try_recv_batch(&mut self) -> Result<Vec<T>, TryRecvError> {
        match self.inner().do_recv() {
            Some(read_pos) => {
                let mut v = Vec::new();
                v.extend(BucketIter::new(self.inner(), read_pos));
                Ok(v)
            }
            None => {
                if self.strong_count() == 1 {
                    Err(TryRecvError::Closed)
                } else {
                    Err(TryRecvError::Empty)
                }
            }
        }
    }

    /// Check if a batch is currently available and return an iterator of its items
    ///
    /// If no batch is available it returns `TryRecvError::Empty`. If no batch will
    /// ever become available because the sender has been dropped it returns
    /// `TryRecvError::Closed`.
    ///
    /// See [`recv`](#method.recv) for more information on the returned iterator.
    pub fn try_recv(&mut self) -> Result<BucketIter<'_, T, N>, TryRecvError> {
        match self.inner().do_recv() {
            Some(read_pos) => Ok(BucketIter::new(self.inner(), read_pos)),
            None => {
                if self.strong_count() == 1 {
                    Err(TryRecvError::Closed)
                } else {
                    Err(TryRecvError::Empty)
                }
            }
        }
    }

    /// A Future that will wait for the next batch and return an iterator of its items
    ///
    /// The iterator should be consumed quickly since it borrows the queue bucket that
    /// holds the items, meaning that the queue space is not handed back to the sender
    /// until the iterator is dropped.
    pub fn recv(&mut self) -> ReceiveFuture<'_, T, N> {
        ReceiveFuture {
            inner: self.inner.as_ref().unwrap(),
        }
    }

    /// Wait for the next batch and fill it into a fresh Vec
    ///
    /// If the next thing you’ll do is to iterate over the vector, prefer
    /// [`recv`](#method.recv) instead to save one allocation.
    pub async fn recv_batch(&mut self) -> Result<Vec<T>, Closed> {
        Ok(self.recv().await?.collect())
    }
}

/// The Future returned from [`recv`](struct.Receiver.html#method.recv)
///
/// It will resolve once a batch becomes available or the queue is closed (by dropping the sender).
pub struct ReceiveFuture<'a, T, const N: usize> {
    inner: &'a Arc<Inner<T, N>>,
}

unsafe impl<'a, T, const N: usize> Send for ReceiveFuture<'a, T, N> {}

impl<'a, T, const N: usize> Future for ReceiveFuture<'a, T, N> {
    type Output = Result<BucketIter<'a, T, N>, Closed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.inner.do_recv() {
            Some(v) => Poll::Ready(Ok(BucketIter::new(self.inner, v))),
            None => {
                self.inner.reader.waker.register(cx.waker());
                if Arc::strong_count(self.inner) == 1 {
                    Poll::Ready(Err(Closed))
                } else {
                    match self.inner.do_recv() {
                        Some(v) => {
                            // no wakeup needed anymore
                            self.inner.reader.waker.take();
                            Poll::Ready(Ok(BucketIter::new(self.inner, v)))
                        }
                        None => Poll::Pending,
                    }
                }
            }
        }
    }
}
