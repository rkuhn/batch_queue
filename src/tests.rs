use super::*;
use crate::{receiver::TryRecvError, sender::TrySendError};
use itertools::Itertools;

#[test]
fn capacity() {
    let (mut tx, mut rx) = batching_queue::<u32, 3>(3);

    assert_eq!(tx.try_send(1), Ok(()));
    tx.close_batch();
    assert_eq!(tx.try_send(2), Ok(()));
    assert_eq!(tx.try_send(3), Ok(()));
    assert_eq!(tx.try_send(4), Ok(()));
    assert_eq!(tx.try_send(5), Ok(()));
    tx.close_batch();
    assert_eq!(tx.try_send(6), Err(TrySendError::Full(6)));

    assert_eq!(rx.try_recv_batch(), Ok(vec![1]));
    assert_eq!(rx.try_recv_batch(), Ok(vec![2, 3, 4]));
    assert_eq!(rx.try_recv_batch(), Ok(vec![5]));
    assert_eq!(rx.try_recv_batch(), Err(TryRecvError::Empty));

    assert_eq!(tx.try_send(1), Ok(()));
    tx.close_batch();
    assert_eq!(tx.try_send(2), Ok(()));
    assert_eq!(tx.try_send(3), Ok(()));
    assert_eq!(tx.try_send(4), Ok(()));
    assert_eq!(tx.try_send(5), Ok(()));
    tx.close_batch();
    assert_eq!(tx.try_send(6), Err(TrySendError::Full(6)));

    assert_eq!(rx.try_recv_batch(), Ok(vec![1]));
    assert_eq!(tx.try_send(6), Ok(()));
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

    assert_eq!(tx.try_send(1), Ok(()));
    drop(rx);
    assert_eq!(tx.try_send(2), Err(TrySendError::Closed));
}

#[tokio::test]
async fn drop_async() {
    let (mut tx, mut rx) = batching_queue::<u32, 5>(3);
    tokio::spawn(async move {
        tx.send(1).await;
    });
    assert_eq!(rx.recv_batch().await, Some(vec![1]));
    assert_eq!(rx.recv_batch().await, None);

    let (mut tx1, mut rx1) = batching_queue::<u32, 5>(3);
    let (tx2, mut rx2) = batching_queue::<u32, 5>(3);
    tokio::spawn(async move {
        rx1.recv().await;
        drop(rx1);
        drop(tx2);
    });
    assert_eq!(tx1.send(1).await, Some(()));
    assert_eq!(tx1.send(2).await, Some(()));
    assert_eq!(tx1.send(3).await, Some(()));
    tx1.close_batch();
    assert_eq!(rx2.recv_batch().await, None);
    assert_eq!(tx1.send(4).await, None);
}

#[tokio::test]
async fn stress() {
    let (mut tx, mut rx) = batching_queue::<u32, 5>(3);
    const N: u32 = 10_000_000;

    tokio::spawn(async move {
        for i in 0..N {
            tx.send(i).await;
        }
    });

    for i in (0..N).chunks(5).into_iter() {
        assert_eq!(rx.recv_batch().await.unwrap(), i.collect::<Vec<_>>());
    }
}
