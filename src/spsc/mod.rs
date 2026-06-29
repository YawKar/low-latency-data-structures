pub mod consumer;
pub mod producer;
mod queue;

pub use queue::{new, new_hugepage_backed};
