use std::{
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
};

type Task = Box<dyn FnOnce() + Send + 'static>;

struct Worker {
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    fn new(id: usize, receiver: Arc<Mutex<mpsc::Receiver<Task>>>) -> Self {
        let handle = thread::spawn(move || {
            println!("worker {} starting", id);
            loop {
                let task = receiver.lock().unwrap().recv();
                match task {
                    Ok(task) => {
                        println!("worker {} executing task", id);
                        task();
                    }
                    Err(_) => break,
                }
            }
            println!("worker {} stopping", id);
        });
        Worker { handle: Some(handle) }
    }
}

pub struct ThreadPool {
    workers: Vec<Worker>,
    sender: Option<mpsc::Sender<Task>>,
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        drop(self.sender.take());
        for worker in &mut self.workers {
            if let Some(handle) = worker.handle.take() {
                handle.join().unwrap();
            }
        }
    }
}

impl ThreadPool {
    pub fn new(size: usize) -> Self {
        assert!(size > 0);
        let mut workers = Vec::with_capacity(size);
        let (sender, receiver) = mpsc::channel();
        let receiver = Arc::new(Mutex::new(receiver));
        for id in 0..size {
            workers.push(Worker::new(id, Arc::clone(&receiver)));
        }
        Self { workers, sender: Some(sender) }
    }

    pub fn execute(&mut self, task: Task) {
        // sender is only none after calling drop()
        self.sender.as_ref().unwrap().send(task).unwrap();
    }
}
