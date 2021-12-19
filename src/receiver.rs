use super::*;
use futures::Future;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

pub struct Receiver<T, const N: usize> {
    inner: Arc<Inner<T, N>>,
}

impl<T, const N: usize> Drop for Receiver<T, N> {
    fn drop(&mut self) {
        // other side will notice by checking Arc ref count
        self.inner.reader.waker.take();
        self.inner.writer.waker.wake();
    }
}

unsafe impl<T, const N: usize> Send for Receiver<T, N> {}

impl<T, const N: usize> Receiver<T, N> {
    pub(crate) fn new(inner: Arc<Inner<T, N>>) -> Self {
        Self { inner }
    }

    pub fn try_recv_batch(&mut self) -> Option<Vec<T>> {
        self.inner.do_recv().map(|read_pos| {
            let mut v = Vec::new();
            v.extend(BucketIter::new(&self.inner, read_pos));
            v
        })
    }

    pub fn try_recv(&mut self) -> Option<BucketIter<'_, T, N>> {
        self.inner
            .do_recv()
            .map(|read_pos| BucketIter::new(&self.inner, read_pos))
    }

    pub fn recv(&mut self) -> ReceiveFuture<'_, T, N> {
        ReceiveFuture {
            inner: &*self.inner,
        }
    }

    pub async fn recv_batch(&mut self) -> Vec<T> {
        self.recv().await.collect()
    }
}

pub struct ReceiveFuture<'a, T, const N: usize> {
    inner: &'a Inner<T, N>,
}

unsafe impl<'a, T, const N: usize> Send for ReceiveFuture<'a, T, N> {}

impl<'a, T, const N: usize> Future for ReceiveFuture<'a, T, N> {
    type Output = BucketIter<'a, T, N>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.inner.do_recv() {
            Some(v) => Poll::Ready(BucketIter::new(self.inner, v)),
            None => {
                self.inner.reader.waker.register(cx.waker());
                match self.inner.do_recv() {
                    Some(v) => {
                        // no wakeup needed anymore
                        self.inner.reader.waker.take();
                        Poll::Ready(BucketIter::new(self.inner, v))
                    }
                    None => Poll::Pending,
                }
            }
        }
    }
}
