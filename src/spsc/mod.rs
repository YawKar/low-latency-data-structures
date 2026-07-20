//! Single-producer, single-consumer bounded wait-free FIFO queue.

mod builder;
mod consumer;
mod producer;
mod queue;

pub use builder::Options;
pub use consumer::Consumer;
pub use producer::Producer;
pub use queue::new;
