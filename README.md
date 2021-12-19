# Batching Queue

A library that implements smart batching between a producer and a consumer.
In other words, a single-producer single-consumer queue that tries to ensure that the either side runs for a while (to make use of L1 caches) before hibernating again.

## Rules

Inserting an item:

- check whether a bucket is available
- insert into next slot in bucket; if full, move write bucket position

Declaring end of current batch:

- check whether current write bucket has elements ⇒ move write bucket position

Taking a bucket:

- check whether a full bucket is available ⇒ take it and move read bucket position
