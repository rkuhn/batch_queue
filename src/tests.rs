use super::*;
use crate::{receiver::TryRecvError, sender::TrySendError};
use itertools::Itertools;
use std::sync::atomic::AtomicU64;

#[test]
fn capacity() {
    let (mut tx, mut rx) = batching_queue::<u32, 3>(3);

    assert_eq!(tx.try_send(1), Ok(false));
    tx.close_batch();
    assert_eq!(tx.try_send(2), Ok(false));
    assert_eq!(tx.try_send(3), Ok(false));
    assert_eq!(tx.try_send(4), Ok(true));
    assert_eq!(tx.try_send(5), Ok(false));
    tx.close_batch();
    assert_eq!(tx.try_send(6), Err(TrySendError::Full(6)));

    assert_eq!(rx.try_recv_batch(), Ok(vec![1]));
    assert_eq!(rx.try_recv_batch(), Ok(vec![2, 3, 4]));
    assert_eq!(rx.try_recv_batch(), Ok(vec![5]));
    assert_eq!(rx.try_recv_batch(), Err(TryRecvError::Empty));

    assert_eq!(tx.try_send(1), Ok(false));
    tx.close_batch();
    assert_eq!(tx.try_send(2), Ok(false));
    assert_eq!(tx.try_send(3), Ok(false));
    assert_eq!(tx.try_send(4), Ok(true));
    assert_eq!(tx.try_send(5), Ok(false));
    tx.close_batch();
    assert_eq!(tx.try_send(6), Err(TrySendError::Full(6)));

    assert_eq!(rx.try_recv_batch(), Ok(vec![1]));
    assert_eq!(tx.try_send(6), Ok(false));
    assert_eq!(rx.try_recv_batch(), Ok(vec![2, 3, 4]));
    assert_eq!(rx.try_recv_batch(), Ok(vec![5]));
    assert_eq!(rx.try_recv_batch(), Err(TryRecvError::Empty));
    tx.close_batch();
    assert_eq!(rx.try_recv_batch(), Ok(vec![6]));
    assert_eq!(rx.try_recv_batch(), Err(TryRecvError::Empty));

    drop(tx);
    assert_eq!(rx.try_recv_batch(), Err(TryRecvError::Closed));
}

#[test]
fn drop_rx() {
    let (mut tx, rx) = batching_queue::<u32, 2>(3);

    assert_eq!(tx.try_send(1), Ok(false));
    drop(rx);
    assert_eq!(tx.try_send(2), Err(TrySendError::Closed));
}

#[test]
fn drop_items() {
    static POOL: AtomicU64 = AtomicU64::new(1);

    #[derive(Debug, PartialEq)]
    struct D(u64);
    impl D {
        pub fn d(self) -> u64 {
            let ret = self.0;
            std::mem::forget(self);
            ret
        }
    }
    impl Drop for D {
        fn drop(&mut self) {
            POOL.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v * self.0))
                .ok();
        }
    }

    let (mut tx, mut rx) = batching_queue::<D, 2>(3);

    assert_eq!(tx.try_send(D(2)), Ok(false));
    assert_eq!(tx.try_send(D(3)), Ok(true));
    assert_eq!(tx.try_send(D(5)), Ok(false));
    tx.close_batch();
    assert_eq!(tx.try_send(D(7)), Ok(false));
    assert_eq!(tx.try_send(D(11)), Ok(true));

    assert_eq!(
        rx.try_recv().unwrap().take(1).map(D::d).collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(POOL.load(Ordering::Relaxed), 3);

    drop(rx);
    assert_eq!(POOL.load(Ordering::Relaxed), 3);

    assert_eq!(tx.try_send(D(13)), Err(TrySendError::Closed));
    assert_eq!(POOL.load(Ordering::Relaxed), 3 * 13);

    drop(tx);
    assert_eq!(POOL.load(Ordering::Relaxed), 3 * 13 * 5 * 7 * 11);
}

#[tokio::test]
async fn drop_async() {
    let (mut tx, mut rx) = batching_queue::<u32, 5>(3);
    let handle = tokio::spawn(async move {
        tx.send(1).await?;
        Result::<_, Closed>::Ok(())
    });
    assert_eq!(rx.recv_batch().await, Ok(vec![1]));
    assert_eq!(rx.recv_batch().await, Err(Closed));
    handle.await.unwrap().unwrap();

    let (mut tx1, mut rx1) = batching_queue::<u32, 5>(3);
    let (tx2, mut rx2) = batching_queue::<u32, 5>(3);
    let handle = tokio::spawn(async move {
        rx1.recv().await?;
        drop(rx1);
        drop(tx2);
        Result::<_, Closed>::Ok(())
    });
    assert_eq!(tx1.send(1).await, Ok(false));
    assert_eq!(tx1.send(2).await, Ok(false));
    assert_eq!(tx1.send(3).await, Ok(false));
    tx1.close_batch();
    assert_eq!(rx2.recv_batch().await, Err(Closed));
    assert_eq!(tx1.send(4).await, Err(Closed));
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn stress() {
    let (mut tx, mut rx) = batching_queue::<u32, 5>(3);
    #[cfg(debug_assertions)]
    const N: u32 = 1_000_000;
    #[cfg(not(debug_assertions))]
    const N: u32 = 10_000_000;

    let handle = tokio::spawn(async move {
        for i in 0..N {
            tx.send(i).await?;
        }
        Result::<_, Closed>::Ok(())
    });

    for i in (0..N).chunks(5).into_iter() {
        assert_eq!(rx.recv_batch().await.unwrap(), i.collect::<Vec<_>>());
    }

    handle.await.unwrap().unwrap();
    assert_eq!(rx.recv_batch().await, Err(Closed));
}

#[tokio::test]
async fn stream() {
    let (tx, mut rx) = batching_queue::<u32, 128>(20);
    #[cfg(debug_assertions)]
    const N: u32 = 10_000_000;
    #[cfg(not(debug_assertions))]
    const N: u32 = 1_000_000_000;

    tokio::spawn(async move {
        let stream = futures::stream::iter(0..N);
        pipe(stream, tx).await;
    });

    let mut x = 0;
    while let Ok(v) = rx.recv().await {
        for y in v {
            assert_eq!(y, x);
            x += 1;
        }
    }
    assert_eq!(x, N);
}
