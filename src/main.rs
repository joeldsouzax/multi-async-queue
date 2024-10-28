use async_task::{Runnable, Task};
use flume::{Receiver, Sender};
use futures_lite::future;
use std::panic::catch_unwind;
use std::thread;
use std::{future::Future, sync::LazyLock, task::Poll, time::Duration};

// its public because you can get in both high queue and low queue handlers
pub static HIGH_CHANNEL: LazyLock<(Sender<Runnable>, Receiver<Runnable>)> =
    LazyLock::new(flume::unbounded);

// its public because you can get it in both high queue and low queue handlers
pub static LOW_CHANNEL: LazyLock<(Sender<Runnable>, Receiver<Runnable>)> =
    LazyLock::new(flume::unbounded);

pub static HIGH_QUEUE: LazyLock<Sender<Runnable>> = LazyLock::new(|| {
    let high_num = std::env::var("HIGH_NUM").unwrap().parse::<usize>().unwrap();
    for _ in 0..high_num {
        thread::spawn({
            let high_receiver = HIGH_CHANNEL.1.clone();
            let low_receiver = LOW_CHANNEL.1.clone();
            move || loop {
                match high_receiver.try_recv() {
                    Ok(runnable) => {
                        let _ = catch_unwind(|| runnable.run());
                    }
                    Err(_) => match low_receiver.try_recv() {
                        Ok(runnable) => {
                            let _ = catch_unwind(|| runnable.run());
                        }
                        Err(_) => thread::sleep(Duration::from_millis(100)),
                    },
                }
            }
        });
    }

    HIGH_CHANNEL.0.clone()
});

pub static LOW_QUEUE: LazyLock<Sender<Runnable>> = LazyLock::new(|| {
    let low_num = std::env::var("LOW_NUM").unwrap().parse::<usize>().unwrap();
    for _ in 0..low_num {
        thread::spawn({
            let high_receiver = HIGH_CHANNEL.1.clone();
            let low_reciever = LOW_CHANNEL.1.clone();
            move || loop {
                match low_reciever.try_recv() {
                    Ok(runnable) => {
                        let _ = catch_unwind(|| runnable.run());
                    }
                    Err(_) => match high_receiver.try_recv() {
                        Ok(runnable) => {
                            let _ = catch_unwind(|| runnable.run());
                        }
                        Err(_) => {
                            thread::sleep(Duration::from_millis(100));
                        }
                    },
                }
            }
        });
    }

    LOW_CHANNEL.0.clone()
});

// macro as default argument for function only works on global level
// no need to specify type here because compiler can infer based on function here
macro_rules! spawn_task {
    ($future:expr) => {
        spawn_task!($future, FutureType::Low)
    };
    ($future:expr, $order:expr) => {
        spawn_task($future, $order)
    };
}

macro_rules! join {
    ($($future:expr),*) => {
        {
            let mut results = Vec::new();
            $(
                results.push(future::block_on($future));
            )*
            results
        }
    };
}

macro_rules! try_join {
    ($($future:expr),*) => {
        {
            let mut results = Vec::new();
            $(
                let result = catch_unwind(|| future::block_on($future));
                results.push(result);
            )*
            results
        }

    };
}

struct Runtime {
    // number of consuming threads for high priority channel
    high_num: usize,
    // number of consuming threads for low priority channel
    low_num: usize,
}

impl Runtime {
    pub fn new() -> Self {
        // get cores that are available.
        let num_cores = std::thread::available_parallelism().unwrap().get();
        Self {
            high_num: num_cores - 2,
            low_num: 1,
        }
    }

    pub fn with_high_num(mut self, num: usize) -> Self {
        self.high_num = num;
        self
    }

    pub fn with_low_num(mut self, num: usize) -> Self {
        self.low_num = num;
        self
    }

    pub fn run(&self) {
        std::env::set_var("HIGH_NUM", self.high_num.to_string());
        std::env::set_var("LOW_NUM", self.low_num.to_string());

        let high = spawn_task!(async {}, FutureType::High);
        let low = spawn_task!(async {}, FutureType::Low);

        join!(high, low);
    }
}

#[derive(Debug, Clone, Copy)]
enum FutureType {
    High,
    Low,
}

fn spawn_task<F, T>(future: F, order: FutureType) -> Task<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let schedule = match order {
        FutureType::High => |runnable| HIGH_QUEUE.send(runnable).unwrap(),
        FutureType::Low => |runnable| LOW_QUEUE.send(runnable).unwrap(),
    };

    let (runnable, task) = async_task::spawn(future, schedule);
    runnable.schedule();
    task
}

struct CounterFuture {
    count: u32,
}

impl Future for CounterFuture {
    type Output = u32;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.count += 1;
        println!("polling with result: {}", self.count);
        std::thread::sleep(Duration::from_secs(1));

        if self.count < 3 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(self.count)
        }
    }
}

async fn async_fn() {
    thread::sleep(Duration::from_secs(1));
    println!("async fn");
}

#[derive(Clone, Copy, Debug)]
struct BackgrounProcess;

impl Future for BackgrounProcess {
    type Output = ();

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        println!("background process firing");
        std::thread::sleep(Duration::from_secs(1));
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

fn main() {
    Runtime::new().run();
    spawn_task!(BackgrounProcess {}).detach();

    let one = CounterFuture { count: 0 };
    let two = CounterFuture { count: 0 };

    let t_one = spawn_task!(one, FutureType::High);
    let t_two = spawn_task!(two);

    let t_three = spawn_task!(async_fn());

    let t_four = spawn_task!(
        async {
            async_fn().await;
            async_fn().await;
        },
        FutureType::High
    );

    let outcome_1: Vec<u32> = join!(t_one, t_two);
    let outcome_2: Vec<()> = join!(t_three, t_four);
}
