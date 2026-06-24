//! Per-cpu local timer interrupt counters from /proc/interrupts. With
//! nohz_full configured correctly these should stay near zero during a run,
//! so the diff before-vs-after is a quick health check.

/// Read LOC counters for the requested cpus. Returns `None` for any cpu not
/// present in /proc/interrupts (e.g. currently offlined).
///
/// /proc/interrupts columns are positional and only include online CPUs, so
/// the header is parsed to map CPU ids to column indices.
pub fn read(cpus: &[usize]) -> Vec<Option<u64>> {
    let Ok(contents) = std::fs::read_to_string("/proc/interrupts") else {
        return vec![None; cpus.len()];
    };
    let mut lines = contents.lines();
    let Some(header) = lines.next() else {
        return vec![None; cpus.len()];
    };
    let columns: Vec<usize> = header
        .split_whitespace()
        .filter_map(|tok| tok.strip_prefix("CPU").and_then(|n| n.parse().ok()))
        .collect();

    let loc_line = lines.find(|l| l.trim_start().starts_with("LOC:"));
    let counts: Vec<u64> = match loc_line {
        Some(l) => l
            .trim_start()
            .trim_start_matches("LOC:")
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect(),
        None => return vec![None; cpus.len()],
    };

    cpus.iter()
        .map(|&cpu| {
            let col = columns.iter().position(|&c| c == cpu)?;
            counts.get(col).copied()
        })
        .collect()
}
