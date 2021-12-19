use super::*;
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
    assert_eq!(tx.try_send(6), Err(6));

    assert_eq!(rx.try_recv_batch(), Some(vec![1]));
    assert_eq!(rx.try_recv_batch(), Some(vec![2, 3, 4]));
    assert_eq!(rx.try_recv_batch(), Some(vec![5]));
    assert_eq!(rx.try_recv_batch(), None);

    assert_eq!(tx.try_send(1), Ok(()));
    tx.close_batch();
    assert_eq!(tx.try_send(2), Ok(()));
    assert_eq!(tx.try_send(3), Ok(()));
    assert_eq!(tx.try_send(4), Ok(()));
    assert_eq!(tx.try_send(5), Ok(()));
    tx.close_batch();
    assert_eq!(tx.try_send(6), Err(6));

    assert_eq!(rx.try_recv_batch(), Some(vec![1]));
    assert_eq!(tx.try_send(6), Ok(()));
    assert_eq!(rx.try_recv_batch(), Some(vec![2, 3, 4]));
    assert_eq!(rx.try_recv_batch(), Some(vec![5]));
    assert_eq!(rx.try_recv_batch(), None);
    tx.close_batch();
    assert_eq!(rx.try_recv_batch(), Some(vec![6]));
    assert_eq!(rx.try_recv_batch(), None);
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
        assert_eq!(rx.recv_batch().await, i.collect::<Vec<_>>());
    }
}
