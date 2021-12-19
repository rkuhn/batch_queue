use super::*;
use futures::Future;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Debug, Clone, PartialEq)]
pub enum TrySendError<T> {
    Full(T),
    Closed,
}

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

    pub fn try_send(&mut self, value: T) -> Result<(), TrySendError<T>> {
        if self.strong_count() == 1 {
            Err(TrySendError::Closed)
        } else {
            match self.inner().do_send(value) {
                Ok(_) => Ok(()),
                Err(v) => Err(TrySendError::Full(v)),
            }
        }
    }

    pub fn close_batch(&mut self) {
        self.inner().do_close_batch()
    }

    pub fn send(&mut self, value: T) -> SendFuture<'_, T, N> {
        SendFuture {
            inner: self.inner.as_ref().unwrap(),
            value: Some(value),
        }
    }
}

pub struct SendFuture<'a, T, const N: usize> {
    inner: &'a Arc<Inner<T, N>>,
    value: Option<T>,
}

unsafe impl<'a, T, const N: usize> Send for SendFuture<'a, T, N> {}

impl<'a, T, const N: usize> Future for SendFuture<'a, T, N> {
    type Output = Option<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        if Arc::strong_count(this.inner) == 1 {
            return Poll::Ready(None);
        }
        let value = match this.value.take() {
            Some(v) => v,
            None => return Poll::Ready(Some(())),
        };
        match this.inner.do_send(value) {
            Ok(_) => Poll::Ready(Some(())),
            Err(v) => {
                this.inner.writer.waker.register(cx.waker());
                // need to check again because the other side may have dropped or consumed before the `register`
                if Arc::strong_count(this.inner) == 1 {
                    Poll::Ready(None)
                } else {
                    match this.inner.do_send(v) {
                        Ok(_) => {
                            this.inner.writer.waker.take();
                            Poll::Ready(Some(()))
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
