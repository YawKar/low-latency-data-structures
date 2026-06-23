mod spsc_latency;
mod spsc_throughput;

fn main() {
    // Criterion part
    spsc_throughput::benches();
    criterion::Criterion::default()
        .configure_from_args()
        .final_summary();
    // Ad-hoc benches
    spsc_latency::benches();
}
