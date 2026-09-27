use std::time::Duration;

use mini_rt::sleep;
use mini_rt::{block_on, spawn};

fn main() {
    let _ = block_on(async {
        let handle1 = spawn(async {
            println!("Task 1 going to sleep...");
            sleep(Duration::from_secs(5)).await;
            println!("Task 1 waking up...");
        });

        let handle2 = spawn(async {
            println!("Task 2 going to sleep...");
            sleep(Duration::from_secs(2)).await;
            println!("Task 2 waking up...");
        });

        handle1.await;
        handle2.await;
    });
}
