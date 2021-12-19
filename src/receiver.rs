use super::*;
use futures::Future;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Debug, Clone, PartialEq)]
pub enum TryRecvError {
    Empty,
    Closed,
}

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

    pub fn recv(&mut self) -> ReceiveFuture<'_, T, N> {
        ReceiveFuture {
            inner: self.inner.as_ref().unwrap(),
        }
    }

    pub async fn recv_batch(&mut self) -> Option<Vec<T>> {
        Some(self.recv().await?.collect())
    }
}

pub struct ReceiveFuture<'a, T, const N: usize> {
    inner: &'a Arc<Inner<T, N>>,
}

unsafe impl<'a, T, const N: usize> Send for ReceiveFuture<'a, T, N> {}

impl<'a, T, const N: usize> Future for ReceiveFuture<'a, T, N> {
    type Output = Option<BucketIter<'a, T, N>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.inner.do_recv() {
            Some(v) => Poll::Ready(Some(BucketIter::new(self.inner, v))),
            None => {
                self.inner.reader.waker.register(cx.waker());
                if Arc::strong_count(self.inner) == 1 {
                    Poll::Ready(None)
                } else {
                    match self.inner.do_recv() {
                        Some(v) => {
                            // no wakeup needed anymore
                            self.inner.reader.waker.take();
                            Poll::Ready(Some(BucketIter::new(self.inner, v)))
                        }
                        None => Poll::Pending,
                    }
                }
            }
        }
    }
}
