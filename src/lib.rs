pub mod bench;
mod mem;
pub mod seqlock;
mod shim;
pub mod spmc;
pub mod spsc;

#[cfg(test)]
#[cfg(feature = "tests_dhat")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;
