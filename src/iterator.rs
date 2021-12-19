use super::*;

/// Iterator over the items received in one batch
///
/// This iterator will only hand over the internal bucket to the sender for refill
/// once the iterator is dropped, so consume it quickly.
pub struct BucketIter<'a, T, const N: usize> {
    inner: &'a Inner<T, N>,
    read_pos: usize,
    pos: usize,
}

impl<'a, T, const N: usize> Drop for BucketIter<'a, T, N> {
    fn drop(&mut self) {
        // need to drop the non-consumed elements
        for _item in self.by_ref() {}
        // and now hand over the bucket to the writer
        unsafe { &mut *self.bucket() }.items = 0;
        let next = self.inner.next(self.read_pos);
        self.inner.reader.position.store(next, Ordering::Release);
        self.inner.writer.waker.wake();
    }
}

impl<'a, T, const N: usize> Iterator for BucketIter<'a, T, N> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        let bucket = unsafe { &*self.bucket() };
        if self.pos < bucket.items {
            let value = unsafe { std::ptr::read(bucket.data[self.pos].as_ptr()) };
            self.pos += 1;
            Some(value)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let s = unsafe { &*self.bucket() }.items - self.pos;
        (s, Some(s))
    }
}

impl<'a, T, const N: usize> BucketIter<'a, T, N> {
    pub(crate) fn new(inner: &'a Inner<T, N>, bucket: usize) -> Self {
        Self {
            inner,
            read_pos: bucket,
            pos: 0,
        }
    }

    #[inline(always)]
    unsafe fn bucket(&self) -> *mut Bucket<T, N> {
        self.inner.buckets[self.inner.slot(self.read_pos)].get()
    }
}
