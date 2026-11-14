# mini-rt

A toy asynchronous runtime written from scratch in Rust, built to demystify the logic behind it.

---

## Features

### 1. Cooperative Task Executor (Milestone 1 - `M1`)

- `block_on` as top-level execution drives an async future to completion on the current thread.
- `spawn` detaches asynchronous tasks and returns an awaitable `JoinHandle<T>`.
- Custom wakers push ready tasks into a shared run queue to resume execution without busy polling.

### 2. Binary Heap Sleep Timer (Milestone 2 - `M2`)

- `sleep()` dispatches timers to a dedicated background thread through an `mpsc` channel.
- Timers were stored inside a `BinaryHeap` and woke the corresponding task upon expiration via its `Waker`.

### 3. 6-Level Hierarchical Timer Wheel (Milestone 2.1 - `M2.1`)

- Replaces the binary heap with a 6-level hierarchical timer wheel for O(1) insertions and lookups.
- Timers are bucketed into levels based on their deadline and cascade down to Level 0 to wake expired tasks.

## Example

```rust
use std::time::Duration;
use mini_rt::{block_on, sleep, spawn};

fn main() {
    block_on(async {
        let handle1 = spawn(async {
            println!("Task 1 sleeping for 5s...");
            sleep(Duration::from_secs(5)).await;
            println!("Task 1 awake.");
        });

        let handle2 = spawn(async {
            println!("Task 2 sleeping for 2s...");
            sleep(Duration::from_secs(2)).await;
            println!("Task 2 awake.");
        });

        handle1.await;
        handle2.await;
    });
}
```

## Development & Testing

```bash
# Run the example
make run

# Run all tests
make test

# Format & lint
make fmt
make lint
```
