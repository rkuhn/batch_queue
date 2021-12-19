use super::*;
use futures::Future;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

pub struct Sender<T, const N: usize> {
    inner: Arc<Inner<T, N>>,
}

impl<T, const N: usize> Drop for Sender<T, N> {
    fn drop(&mut self) {
        // other side will notice by checking Arc ref count
        self.inner.writer.waker.take();
        self.inner.reader.waker.wake();
    }
}

unsafe impl<T, const N: usize> Send for Sender<T, N> {}

impl<T, const N: usize> Sender<T, N> {
    pub(crate) fn new(inner: Arc<Inner<T, N>>) -> Self {
        Self { inner }
    }

    pub fn try_send(&mut self, value: T) -> Result<(), T> {
        self.inner.do_send(value)
    }

    pub fn close_batch(&mut self) {
        self.inner.do_close_batch()
    }

    pub fn send(&mut self, value: T) -> SendFuture<'_, T, N> {
        SendFuture {
            inner: &*self.inner,
            value: Some(value),
        }
    }
}

pub struct SendFuture<'a, T, const N: usize> {
    inner: &'a Inner<T, N>,
    value: Option<T>,
}

unsafe impl<'a, T, const N: usize> Send for SendFuture<'a, T, N> {}

impl<'a, T, const N: usize> Future for SendFuture<'a, T, N> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        let value = match this.value.take() {
            Some(v) => v,
            None => return Poll::Ready(()),
        };
        match this.inner.do_send(value) {
            Ok(_) => Poll::Ready(()),
            Err(v) => {
                this.inner.writer.waker.register(cx.waker());
                match this.inner.do_send(v) {
                    Ok(_) => {
                        this.inner.writer.waker.take();
                        Poll::Ready(())
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
