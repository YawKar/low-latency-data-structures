//! Human-readable formatting for latency-class numbers.

/// Format a duration in nanoseconds with an auto-picked unit. Output is ASCII
/// (uses `us` rather than `µs`) so `{:>N}` width alignment works in bytes ==
/// display columns, which is what almost every terminal-table user wants.
pub fn ns(ns: u64) -> String {
    if ns < 10_000 {
        format!("{ns}ns")
    } else if ns < 10_000_000 {
        format!("{:.2}us", ns as f64 / 1_000.0)
    } else if ns < 10_000_000_000 {
        format!("{:.2}ms", ns as f64 / 1_000_000.0)
    } else {
        format!("{:.2}s", ns as f64 / 1_000_000_000.0)
    }
}
