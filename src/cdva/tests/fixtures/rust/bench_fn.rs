pub fn work(n: u64) -> u64 {
    n.wrapping_mul(3)
}

#[bench]
fn bench_work(b: &mut Bencher) {
    b.iter(|| work(21));
}
