use std::thread;

use hdrhistogram::Histogram;
use low_latency_data_structures::spsc;

#[cfg(target_arch = "x86_64")]
#[inline]
fn rdtscp() -> u64 {
    unsafe {
        let mut aux: u32 = 0;
        core::arch::x86_64::__rdtscp(&mut aux)
    }
}

fn main() {
    let capacity = 65536;
    let iterations: u64 = 1_000_000_000;
    let (producer, consumer) = spsc::new::<u64>(capacity).unwrap();

    let core_ids = core_affinity::get_core_ids().unwrap();
    // Use `lscpu --all --extended` to find cores sharing L3
    let producer_core = core_ids[0];
    let consumer_core = core_ids[1];

    // --- Consumer thread: measure latency ---
    let consumer_handle = thread::spawn(move || {
        core_affinity::set_for_current(consumer_core);

        let mut hist = Histogram::<u64>::new(3).unwrap();
        let mut received = 0u64;

        while received < iterations {
            if let Some(ts_sent) = consumer.pop() {
                let ts_received = rdtscp();
                let latency = ts_received.wrapping_sub(ts_sent);
                let _ = hist.record(latency);
                received += 1;
            } else {
                core::hint::spin_loop();
            }
        }
        hist
    });

    // --- Producer thread: push timestamps ---
    thread::spawn(move || {
        core_affinity::set_for_current(producer_core);

        for _ in 0..iterations {
            let ts = rdtscp();
            while producer.push(ts).is_some() {
                core::hint::spin_loop();
            }
        }
    });

    let hist = consumer_handle.join().unwrap();

    // --- Report ---
    let tsc_freq_ghz = estimate_tsc_freq();
    let cycles_to_ns = |c: u64| (c as f64 / tsc_freq_ghz) as u64;

    println!(
        "SPSC Latency: {} iterations, capacity {}",
        iterations, capacity
    );
    println!(
        "  p50:   {:>6} cycles  ({:>4} ns)",
        hist.value_at_quantile(0.50),
        cycles_to_ns(hist.value_at_quantile(0.50))
    );
    println!(
        "  p90:   {:>6} cycles  ({:>4} ns)",
        hist.value_at_quantile(0.90),
        cycles_to_ns(hist.value_at_quantile(0.90))
    );
    println!(
        "  p99:   {:>6} cycles  ({:>4} ns)",
        hist.value_at_quantile(0.99),
        cycles_to_ns(hist.value_at_quantile(0.99))
    );
    println!(
        "  p99.9: {:>6} cycles  ({:>4} ns)",
        hist.value_at_quantile(0.999),
        cycles_to_ns(hist.value_at_quantile(0.999))
    );
    println!(
        "  max:   {:>6} cycles  ({:>4} ns)",
        hist.max(),
        cycles_to_ns(hist.max())
    );
    println!("  mean:  {:>6.1} cycles", hist.mean());
}

/// Rough TSC frequency estimation (GHz)
fn estimate_tsc_freq() -> f64 {
    let start = rdtscp();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let end = rdtscp();
    (end - start) as f64 / 100_000_000.0 // cycles per ns
}
