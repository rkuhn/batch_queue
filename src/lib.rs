#![doc = include_str!("../README.md")]

use cache_padded::CachePadded;
use futures::task::AtomicWaker;
use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

mod iterator;
mod receiver;
mod sender;
#[cfg(test)]
mod tests;

pub use iterator::BucketIter;
pub use receiver::{ReceiveFuture, Receiver};
pub use sender::{SendFuture, Sender};

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

    fn do_send(&self, value: T) -> Result<(), T> {
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
            }
            Ok(())
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

pub fn batching_queue<T, const N: usize>(buckets: usize) -> (Sender<T, N>, Receiver<T, N>) {
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
