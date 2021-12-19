# Batching Queue

A library that implements smart batching between a producer and a consumer.
In other words, a single-producer single-consumer queue that tries to ensure that the either side runs for a while (to make use of L1 caches) before hibernating again.

One nice feature of this queue is that all buckets are preallocated, so unless you use the `recv_batch` or `try_recv_batch` methods no allocations are done as part of sending and receiving items.

## Example

```rust
# tokio_test::block_on(async {
use batch_queue::{batch_queue, pipe};

let (tx, mut rx) = batch_queue::<u32, 128>(20);
const N: u32 = 10_000_000;

tokio::spawn(async move {
    let stream = futures::stream::iter(0..N);
    pipe(stream, tx).await;
});

let mut x = 0;
while let Ok(iter) = rx.recv().await {
    for y in iter {
        assert_eq!(y, x);
        x += 1;
    }
}
assert_eq!(x, N);
# })
```

Here, the iterator returned by `recv()` allows you to consume the bucket contents without allocating.
On my AMD Hetzner box it takes about 6ns per item.

## How it works

The queue is modeled as a boxed slice of buckets, which each is an appropriately sized array of `MaybeUninit`-ialized items and a fill level.
Reader and writer maintain a current position each, which includes the bucket index as well as a cycle counter (to detect when the writer is exactly one full round ahead of the reader).

Inserting an item:

- check whether the writer’s bucket is available for writing (i.e. it is not the reader’s bucket in the previous cycle)
- if so, insert into next slot in bucket; if full, move write bucket position

Declaring end of current batch:

- check whether current write bucket has elements ⇒ move write bucket position

Taking a bucket:

- check whether the writer’s bucket is ahead of the reader’s bucket ⇒ take it and move read bucket position
