# mini-rt

A small from-scratch **Rust async runtime**, built to learn how executors work.

It covers the usual core pieces — tasks, a run queue, wakers, `block_on`, and `spawn` — without I/O, timers, or a multi-threaded scheduler.

```bash
cargo run
```
