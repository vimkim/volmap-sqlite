#[path = "../benchmarks/probe.rs"]
mod probe;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: large_probe DATABASE [RESIDENT_BYTES] [CACHE_BYTES]")?;
    let resident = args
        .next()
        .map_or(Ok(64 * 1024 * 1024), |value| value.parse::<u64>())?;
    let cache = args
        .next()
        .map_or(Ok(1024 * 1024), |value| value.parse::<u64>())?;
    println!(
        "{}",
        probe::run(std::path::Path::new(&path), resident, cache)?
    );
    Ok(())
}
