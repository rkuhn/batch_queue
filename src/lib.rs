#![doc = include_str!("../README.md")]

use cache_padded::CachePadded;
use futures::{task::AtomicWaker, Future, Stream};
use pin_project::pin_project;
use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

mod iterator;
mod receiver;
mod sender;
#[cfg(test)]
mod tests;

pub use iterator::BucketIter;
pub use receiver::{ReceiveFuture, Receiver, TryRecvError};
pub use sender::{SendFuture, Sender, TrySendError};

/// Error generated from send or receive operations when the other side of the
/// channel has been dropped
#[derive(Debug, Clone, PartialEq)]
pub struct Closed;

struct Bucket<T, const N: usize> {
    data: [MaybeUninit<T>; N],
    items: usize,
}

impl<T, const N: usize> Default for Bucket<T, N> {
    fn default() -> Self {
        // copied from std lib (uninit_array is on nightly)
        let data = unsafe { MaybeUninit::uninit().assume_init() };
        Self { items: 0, data }
    }
}

#[derive(Default)]
struct Party {
    position: AtomicUsize,
    waker: AtomicWaker,
}

struct Inner<T, const N: usize> {
    writer: CachePadded<Party>,
    reader: CachePadded<Party>,
    buckets: Box<[UnsafeCell<Bucket<T, N>>]>,
    cycle_shift: u32,
}

impl<T, const N: usize> Drop for Inner<T, N> {
    fn drop(&mut self) {
        while let Some(read_pos) = self.do_recv() {
            let bucket = unsafe { &mut *self.buckets[self.slot(read_pos)].get() };
            for idx in 0..bucket.items {
                unsafe { std::ptr::drop_in_place(bucket.data[idx].as_mut_ptr()) };
            }
            self.reader
                .position
                .store(self.next(read_pos), Ordering::Relaxed);
        }
    }
}

impl<T, const N: usize> Inner<T, N> {
    fn do_recv(&self) -> Option<usize> {
        let write_pos = self.writer.position.load(Ordering::Acquire);
        let read_pos = self.reader.position.load(Ordering::Relaxed);
        if read_pos == write_pos {
            None
        } else {
            Some(read_pos)
        }
    }

    fn do_send(&self, value: T) -> Result<bool, T> {
        let write_pos = self.writer.position.load(Ordering::Relaxed);
        let write_slot = self.slot(write_pos);
        let read_pos = self.reader.position.load(Ordering::Acquire);
        let read_slot = self.slot(read_pos);
        if write_slot == read_slot && write_pos != read_pos {
            // writer is one cycle ahead of reader
            Err(value)
        } else {
            // this bucket belongs to the writer now, so we are allowed to create a &mut
            let bucket = unsafe { &mut *self.buckets[write_slot].get() };
            unsafe { std::ptr::write(bucket.data[bucket.items].as_mut_ptr(), value) };
            bucket.items += 1;
            if bucket.items == N {
                // bucket is full, hand it over
                let next_pos = self.next(write_pos);
                self.writer.position.store(next_pos, Ordering::Release);
                self.reader.waker.wake();
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }

    fn do_close_batch(&self) {
        let write_pos = self.writer.position.load(Ordering::Relaxed);
        let write_slot = self.slot(write_pos);
        let read_pos = self.reader.position.load(Ordering::Acquire);
        if write_slot != self.slot(read_pos) || write_pos == read_pos {
            let items = unsafe { &*self.buckets[write_slot].get() }.items;
            if items > 0 {
                let next = self.next(write_pos);
                self.writer.position.store(next, Ordering::Release);
            }
        }
    }

    fn buckets(&self) -> usize {
        self.buckets.len()
    }

    fn next(&self, pos: usize) -> usize {
        let slot_mask = (1usize << self.cycle_shift) - 1;
        let next = pos + 1;
        if next & slot_mask == self.buckets() {
            let cycle_mask = !slot_mask;
            (pos & cycle_mask).wrapping_add(1 << self.cycle_shift)
        } else {
            next
        }
    }

    fn slot(&self, pos: usize) -> usize {
        let mask = (1usize << self.cycle_shift) - 1;
        pos & mask
    }
}

/// Create a new batching queue
///
/// The bucket size is given as a const generic while the number of buckets are given as
/// a function parameter. The transfer from sender to receiver happens only in units of
/// one bucket (which may not be full, see [`close_batch`](struct.Sender.html#close_batch)).
pub fn batch_queue<T, const N: usize>(buckets: usize) -> (Sender<T, N>, Receiver<T, N>) {
    let size = buckets;
    let buckets = {
        let mut b = Vec::with_capacity(size);
        b.resize_with(size, UnsafeCell::default);
        b.into_boxed_slice()
    };
    let inner = Arc::new(Inner {
        writer: Party::default().into(),
        reader: Party::default().into(),
        buckets,
        cycle_shift: usize::BITS - size.leading_zeros(),
    });
    (Sender::new(inner.clone()), Receiver::new(inner))
}

/// Pipe a stream into a batching sender
///
/// This will poll the stream for as long as items are forthcoming and the current bucket still
/// has room. When the latter condition is hit, the task is rescheduled to make room for other
/// tasks to run on the current executor.
pub fn pipe<S: Stream, const N: usize>(source: S, sink: Sender<S::Item, N>) -> Pipe<S, N> {
    Pipe {
        source,
        sink,
        item: None,
    }
}

/// The Future created by [`pipe`](fn.pipe.html)
///
/// It will resolve once the source stream ends or the target queue is closed
/// (by dropping its receiver).
#[pin_project]
#[must_use = "streams do nothing without being polled"]
pub struct Pipe<S: Stream, const N: usize> {
    #[pin]
    source: S,
    sink: Sender<S::Item, N>,
    item: Option<S::Item>,
}

impl<S: Stream, const N: usize> Future for Pipe<S, N> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        if let Some(item) = this.item.take() {
            match this.sink.try_poll(item, cx) {
                Ok(_) => {} // continue
                Err(TrySendError::Full(item)) => {
                    *this.item = Some(item);
                    return Poll::Pending;
                }
                Err(TrySendError::Closed) => return Poll::Ready(()),
            }
        }
        let ret = loop {
            match this.source.as_mut().poll_next(cx) {
                Poll::Ready(i) => match i {
                    Some(item) => match this.sink.try_poll(item, cx) {
                        Ok(batch_closed) if batch_closed => {
                            cx.waker().wake_by_ref();
                            break Poll::Pending;
                        }
                        Ok(_) => {} // continue
                        Err(TrySendError::Full(item)) => {
                            *this.item = Some(item);
                            break Poll::Pending;
                        }
                        Err(TrySendError::Closed) => break Poll::Ready(()),
                    },
                    None => break Poll::Ready(()),
                },
                Poll::Pending => break Poll::Pending,
            };
        };
        this.sink.close_batch();
        ret
    }
}
