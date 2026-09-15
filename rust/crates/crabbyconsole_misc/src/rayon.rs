use std::thread::available_parallelism;

/// Setup rayon thread pool and ensure we keep at least 1 logical CPU unoccupied for the main thread of the game.
pub fn setup_rayon() {
    // If we don't have enough permission to get the parallelism amount, default to 8.
    let logical_cpus = available_parallelism().map(|num| num.get()).unwrap_or(8);

    // Get the amount of cpus minus one, so the main thread of the game doesn't slow down.
    // We want at least 2 threads in any case, otherwise using Rayon at all is pointless.
    let num_rayon_threads = logical_cpus.saturating_sub(1).max(2);

    match rayon::ThreadPoolBuilder::new()
        .thread_name(|thread_id| format!("rayon{thread_id}")) // Use a short name or Tracy cuts it off
        .num_threads(num_rayon_threads)
        .build_global()
    {
        Ok(()) => tracing::info!(
            "rayon thread pool initialized ({num_rayon_threads}/{logical_cpus} threads will be used)"
        ),
        Err(err) => tracing::warn!("failed to initialize global Rayon thread pool: {err}"),
    }
}
