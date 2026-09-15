use std::{hint::black_box, rc::Rc, sync::Arc};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

// This benchmark compares various methods of string cloning.

/// Turns a number into a string.
///
/// Let's use string that are equally long compared to typical expressions you type in the console.
/// Example: `var node = Node3d.new(); node.name = 'foo'; scene().add_child(node)` -> 67 characters.
/// Median line length from my history is ~27 characters but I measured in an inaccurate way (includes garbage at the end).
/// The longest line in my history is 436 characters.
fn i_to_string(i: u64) -> String {
    "#".repeat(i as usize)
}

fn bench_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_cloning");

    // Try string lengths 3, 30, 300 and 3000.
    // 3000 seems excessive, we almost never have strings that long.
    for i in [3u64, 30u64, 300u64, 3000u64].iter() {
        group.bench_with_input(BenchmarkId::new("string_clone", i), i, |bench, i| {
            let string = i_to_string(*i);

            // Clone the String itself
            // (note this also measures dealloc time of the string)
            bench.iter(|| black_box(string.clone()))
        });
        group.bench_with_input(BenchmarkId::new("rc_clone", i), i, |bench, i| {
            let string: Rc<str> = Rc::from(i_to_string(*i));

            // Clone the Rc<str>
            bench.iter(|| black_box(Rc::clone(&string)))
        });
        group.bench_with_input(BenchmarkId::new("arc_clone", i), i, |bench, i| {
            let string: Arc<str> = Arc::from(i_to_string(*i));

            // Clone the Arc<str>
            bench.iter(|| black_box(Arc::clone(&string)))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_clone);
criterion_main!(benches);
