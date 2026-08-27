//! `itsanas bench` — how fast is this machine, and can it hold a terabyte?
//!
//! # Why this is a product command and not a `cargo bench`
//!
//! The question this answers is not "did that change make chunking 3% faster".
//! It is **"will this work on my Raspberry Pi"**, and the only person who can
//! answer that is the person holding the Pi. A `criterion` harness in the
//! repository measures the developer's laptop, which is the machine that was
//! never in doubt.
//!
//! So it ships, it needs no toolchain, and it prints the number that actually
//! decides things: how long the initial upload of a full disk would take.
//!
//! # What it measures, and on what
//!
//! Every stage runs on **incompressible, non-repeating data** generated on the
//! fly by a BLAKE3 extendable-output function. That is the pessimistic case and
//! it is chosen deliberately: real folders contain duplicate blocks, and
//! deduplication would flatter every number here. A photo library or a set of
//! virtual machine images is much closer to this than to a folder of text.
//!
//! The data is generated rather than held, so `--size 8G` costs no more memory
//! than `--size 64M`. A benchmark that needed the RAM it claims to prove you do
//! not need would be an odd thing to ship.
//!
//! # It also checks the answer
//!
//! Everything written is read back and compared by hash. A benchmark that
//! silently measures a broken path is worse than no benchmark, because it
//! produces a confident number.

use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

use itsanas_crypto::{DeviceKeys, MasterSecret, UserKeys};
use itsanas_store::{ChunkerConfig, Store, chunker};

use crate::{
    config::format_size,
    error::{CliError, Result},
};

/// Deterministic, incompressible bytes, generated as they are read.
///
/// A `Read` rather than a buffer so the benchmark's own memory use does not
/// depend on the size being measured — which is the property the store claims
/// and which this command is partly here to demonstrate.
struct Generated {
    xof: blake3::OutputReader,
    remaining: u64,
}

impl Generated {
    fn new(bytes: u64) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"itsanas benchmark corpus v1");
        Self {
            xof: hasher.finalize_xof(),
            remaining: bytes,
        }
    }
}

impl Read for Generated {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let take = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        self.xof.fill(&mut buffer[..take]);
        self.remaining -= take as u64;
        Ok(take)
    }
}

/// Counts and hashes whatever is written to it, and keeps none of it.
struct Sink {
    hasher: blake3::Hasher,
    bytes: u64,
}

impl Sink {
    fn new() -> Self {
        Self {
            hasher: blake3::Hasher::new(),
            bytes: 0,
        }
    }
}

impl Write for Sink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.hasher.update(data);
        self.bytes += data.len() as u64;
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// One measured stage.
struct Stage {
    name: &'static str,
    bytes: u64,
    elapsed: Duration,
    note: String,
}

impl Stage {
    /// Bytes per second, or zero for an immeasurably fast stage.
    ///
    /// Floating point is fine here and nowhere else in this project: these are
    /// human-facing measurements printed to three significant figures, not
    /// values two machines have to agree on. The rule about `f64` exists
    /// because `f64::ln` is libm-dependent and placement must be identical
    /// everywhere; a throughput readout has no such requirement.
    #[allow(clippy::cast_precision_loss)]
    fn rate(&self) -> f64 {
        let seconds = self.elapsed.as_secs_f64();
        if seconds <= 0.0 {
            return 0.0;
        }
        self.bytes as f64 / seconds
    }
}

/// Run the benchmark and print a report.
pub fn run(size: u64, quick: bool) -> Result<()> {
    let size = size.max(1024 * 1024);
    measure(size, quick)
}

/// Time each stage against a real store in a scratch directory.
fn measure(size: u64, quick: bool) -> Result<()> {
    let dir = tempfile::Builder::new()
        .prefix("itsanas-bench-")
        .tempdir()
        .map_err(|error| CliError::Usage(format!("could not make a scratch directory: {error}")))?;

    preamble(size, dir.path());

    let master = MasterSecret::generate()
        .map_err(|error| CliError::Usage(format!("could not generate a key: {error}")))?;
    let user = UserKeys::derive(&master);
    let device = DeviceKeys::generate()
        .map_err(|error| CliError::Usage(format!("could not generate a device key: {error}")))?;

    let mut stages = crypto_stages(size, &user)?;

    let store = Store::open(dir.path().join("store"), user, device)
        .map_err(|error| CliError::Usage(format!("could not open the scratch store: {error}")))?;

    // Streamed from a fresh generator, so nothing is buffered and the figure is
    // the store's own cost rather than the cost of holding the input.
    let start = Instant::now();
    store
        .write_stream("bench/data.bin", Generated::new(size))
        .map_err(|error| CliError::Usage(format!("store write failed: {error}")))?;
    stages.push(Stage {
        name: "  + store write",
        bytes: size,
        elapsed: start.elapsed(),
        note: "chunk, seal, write blobs, index, log".to_owned(),
    });

    let mut sink = Sink::new();
    let start = Instant::now();
    let found = store
        .read_stream("bench/data.bin", &mut sink)
        .map_err(|error| CliError::Usage(format!("store read failed: {error}")))?;
    let read_elapsed = start.elapsed();

    if !found {
        return Err(CliError::Usage(
            "the benchmark wrote a file and then could not find it".to_owned(),
        ));
    }
    verify_round_trip(&sink, size)?;

    // Captured before the latency stage writes its own samples into the same
    // store. Reading it afterwards reported the overhead of the throughput file
    // as five times its input, which is the sort of confident wrong number a
    // benchmark exists to avoid producing.
    let throughput_stats = store.stats().ok();

    let latency = latency_stage(&store, quick)?;

    stages.push(Stage {
        name: "  + store read",
        bytes: size,
        elapsed: read_elapsed,
        note: "index, read blobs, open, verify, reassemble".to_owned(),
    });

    report(&stages, size, throughput_stats.as_ref());
    report_latency(&latency);
    Ok(())
}

/// What is about to be measured, and on what.
fn preamble(size: u64, scratch: &std::path::Path) {
    println!("itsanas bench");
    println!(
        "  data      {} of incompressible, non-repeating bytes",
        format_size(size)
    );
    println!("  scratch   {}", scratch.display());
    println!(
        "  chunker   {} average",
        format_size(ChunkerConfig::DEFAULT_AVG as u64)
    );
    println!();
    println!("Nothing here touches your account. A throwaway identity is generated");
    println!("for the run and thrown away with the directory.");
    println!();
}

/// Chunking, and chunking plus sealing, with nothing written anywhere.
///
/// Separated from the store stages because the gap between them is the finding:
/// when sealing runs at ten times the rate of a store write, the bottleneck is
/// the filesystem, and no amount of cryptographic tuning will move it.
#[allow(clippy::cast_precision_loss)]
fn crypto_stages(size: u64, user: &UserKeys) -> Result<Vec<Stage>> {
    let mut stages = Vec::new();

    let mut count = 0u64;
    let start = Instant::now();
    chunker::split_stream(&ChunkerConfig::default(), Generated::new(size), |_| {
        count += 1;
        Ok(())
    })
    .map_err(|error| CliError::Usage(format!("chunking failed: {error}")))?;
    stages.push(Stage {
        name: "chunking (FastCDC)",
        bytes: size,
        elapsed: start.elapsed(),
        note: format!(
            "{count} chunks, {} average",
            format_size(size / count.max(1))
        ),
    });

    let mut sealed_bytes = 0u64;
    let start = Instant::now();
    chunker::split_stream(&ChunkerConfig::default(), Generated::new(size), |chunk| {
        let (_, sealed) = user.seal_chunk(chunk)?;
        sealed_bytes += sealed.len() as u64;
        Ok(())
    })
    .map_err(|error| CliError::Usage(format!("sealing failed: {error}")))?;
    let overhead = sealed_bytes.saturating_sub(size);
    stages.push(Stage {
        name: "  + sealing",
        bytes: size,
        elapsed: start.elapsed(),
        note: format!(
            "{} of overhead, {:.2}%",
            format_size(overhead),
            (overhead as f64 / size as f64) * 100.0
        ),
    });

    Ok(stages)
}

/// Sizes a person actually saves, and how many times each is measured.
///
/// The throughput figures above answer "how long does the archive take". This
/// answers the question that decides whether the thing is usable: **when I hit
/// save, does it feel instant?** A film taking two hours is a background job. A
/// document taking two seconds is a tool nobody keeps.
const DOCUMENT_SIZES: [(&str, u64, usize); 5] = [
    ("a note", 4 * 1024, 60),
    ("a spreadsheet", 64 * 1024, 60),
    ("a Word document", 512 * 1024, 40),
    ("a big PDF", 4 * 1024 * 1024, 20),
    ("a photo burst", 32 * 1024 * 1024, 5),
];

/// The threshold below which a save is indistinguishable from instant.
///
/// Not a number picked for comfort: it is roughly the point at which a person
/// stops perceiving a delay as a delay. Anything under it, the operation feels
/// like it already happened.
const FEELS_INSTANT: Duration = Duration::from_millis(100);

/// How long one save takes, from a caller handing over bytes to the file being
/// durably stored and announced.
struct Latency {
    label: &'static str,
    size: u64,
    samples: Vec<Duration>,
}

impl Latency {
    /// The sample at the given percentile, samples already sorted.
    fn at(&self, percentile: f64) -> Duration {
        if self.samples.is_empty() {
            return Duration::ZERO;
        }
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let index = (((self.samples.len() - 1) as f64) * percentile).round() as usize;
        self.samples[index]
    }

    fn worst(&self) -> Duration {
        self.samples.last().copied().unwrap_or(Duration::ZERO)
    }
}

/// Time repeated saves of realistic document sizes.
///
/// Each iteration writes to a **different logical path**, so nothing is
/// measuring an overwrite of something already chunked and already on disk.
/// Reusing one path would let deduplication answer instantly and produce a
/// number that means nothing.
fn latency_stage(store: &Store, quick: bool) -> Result<Vec<Latency>> {
    let mut out = Vec::new();

    for (label, size, iterations) in DOCUMENT_SIZES {
        let iterations = if quick { iterations.min(5) } else { iterations };
        let mut samples = Vec::with_capacity(iterations);

        for index in 0..iterations {
            // Fresh bytes each time: identical content would deduplicate to
            // nothing and the second save would be free.
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"itsanas latency sample");
            hasher.update(&size.to_le_bytes());
            hasher.update(&index.to_le_bytes());
            let mut generator = Generated {
                xof: hasher.finalize_xof(),
                remaining: size,
            };

            let path = format!("bench/save-{size}-{index}.bin");
            let start = Instant::now();
            store
                .write_stream(&path, &mut generator)
                .map_err(|error| CliError::Usage(format!("save failed: {error}")))?;
            // A save is not finished when the bytes are on this disk: it is
            // finished when the change has been sealed into a log segment that
            // peers can pull. Measuring only the write would flatter the
            // number by leaving out the part the user is waiting for.
            store
                .flush_segment()
                .map_err(|error| CliError::Usage(format!("flush failed: {error}")))?;
            samples.push(start.elapsed());
        }

        samples.sort_unstable();
        out.push(Latency {
            label,
            size,
            samples,
        });
    }

    Ok(out)
}

/// Print the latency table and say plainly whether it is good enough.
fn report_latency(rows: &[Latency]) {
    println!();
    println!("saving a file — the number that decides whether this is usable");
    println!(
        "{:<18} {:>10} {:>10} {:>10} {:>10}",
        "what", "size", "typical", "p95", "worst"
    );
    let rule = "-".repeat(62);
    println!("{rule}");

    let mut worst_offender: Option<&Latency> = None;
    for row in rows {
        println!(
            "{:<18} {:>10} {:>10} {:>10} {:>10}",
            row.label,
            format_size(row.size),
            millis(row.at(0.5)),
            millis(row.at(0.95)),
            millis(row.worst())
        );
        if row.at(0.95) > FEELS_INSTANT
            && worst_offender.is_none_or(|current| row.at(0.95) > current.at(0.95))
        {
            worst_offender = Some(row);
        }
    }

    println!();
    match worst_offender {
        None => println!(
            "  Every one of these is under {}. Saving a document is instant.",
            millis(FEELS_INSTANT)
        ),
        Some(row) => println!(
            "  {} ({}) takes {} at the 95th percentile, over the {} that reads \
             as instant. That is the thing to fix.",
            row.label,
            format_size(row.size),
            millis(row.at(0.95)),
            millis(FEELS_INSTANT)
        ),
    }
}

/// A duration in the unit a person reads without converting.
fn millis(duration: Duration) -> String {
    let ms = duration.as_secs_f64() * 1000.0;
    if ms < 10.0 {
        format!("{ms:.1}ms")
    } else if ms < 1000.0 {
        format!("{ms:.0}ms")
    } else {
        format!("{:.2}s", ms / 1000.0)
    }
}

/// Confirm what came back is what went in.
///
/// A benchmark of a broken path is worse than no benchmark, because it produces
/// a confident number. If this fails, no figures are printed at all.
fn verify_round_trip(sink: &Sink, size: u64) -> Result<()> {
    if sink.bytes != size {
        return Err(CliError::Usage(format!(
            "read back {} of a {} file",
            format_size(sink.bytes),
            format_size(size)
        )));
    }

    let mut reference = Generated::new(size);
    let mut check = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = reference
            .read(&mut buffer)
            .map_err(|error| CliError::Usage(format!("generator failed: {error}")))?;
        if read == 0 {
            break;
        }
        check.update(&buffer[..read]);
    }

    if sink.hasher.clone().finalize() != check.finalize() {
        return Err(CliError::Usage(
            "the data read back did not match what was written, so these numbers \
             would be meaningless and are not printed"
                .to_owned(),
        ));
    }
    Ok(())
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    // The header literals stay as arguments so their column widths sit next to
    // the row format below and cannot drift apart.
    clippy::print_literal
)]
fn report(stages: &[Stage], size: u64, stats: Option<&itsanas_store::StoreStats>) {
    println!(
        "{:<22} {:>12} {:>12}   {}",
        "stage", "throughput", "elapsed", "notes"
    );
    let rule = "-".repeat(78);
    println!("{rule}");
    for stage in stages {
        println!(
            "{:<22} {:>10}/s {:>12}   {}",
            stage.name,
            format_size(stage.rate() as u64),
            format!("{:.2}s", stage.elapsed.as_secs_f64()),
            stage.note
        );
    }

    // The number that decides things. Everything above is detail.
    if let Some(write) = stages.iter().find(|s| s.name.contains("store write")) {
        let rate = write.rate();
        if rate > 0.0 {
            println!();
            println!("what that means");
            for (label, bytes) in [
                ("10 GB", 10u64 << 30),
                ("100 GB", 100u64 << 30),
                ("1 TB", 1024u64 << 30),
            ] {
                let seconds = bytes as f64 / rate;
                println!("  {label:>7} would take {}", human_time(seconds));
            }
            println!();
            println!("  Local work only. A real first upload is also limited by the");
            println!("  network and by the slowest peer accepting it.");
        }
    }

    if let Some(stats) = stats {
        println!();
        println!("on disk");
        println!(
            "  {} in {} chunks",
            format_size(stats.bytes_on_disk),
            stats.live_chunks
        );
        let ratio = stats.bytes_on_disk as f64 / size as f64;
        println!("  {ratio:.3}x the input, which is sealing overhead and nothing else");
    }

    if let Some(peak) = peak_memory() {
        println!();
        println!("memory");
        println!("  peak {} for the whole run", format_size(peak));
        println!("  The store streams, so this should not grow with --size.");
        println!("  If it does, that is a bug worth reporting.");
    }
}

/// Highest resident memory this process has reached, if the platform says.
///
/// Linux only, deliberately. That is where the Raspberry Pi and the Freebox VM
/// live, which are the machines whose memory is actually in question; adding a
/// Windows dependency to measure a laptop that has 32 GB would be spending an
/// audit cost on the machine nobody worries about.
fn peak_memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kilobytes: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kilobytes * 1024);
        }
    }
    None
}

/// Seconds as something a person can act on.
fn human_time(seconds: f64) -> String {
    if seconds < 90.0 {
        format!("{seconds:.0} seconds")
    } else if seconds < 5400.0 {
        format!("{:.0} minutes", seconds / 60.0)
    } else if seconds < 172_800.0 {
        format!("{:.1} hours", seconds / 3600.0)
    } else {
        format!("{:.1} days", seconds / 86_400.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_produces_exactly_what_was_asked_for() {
        // The benchmark divides by this. A generator that quietly delivered
        // fewer bytes would inflate every throughput figure it reports.
        for size in [1u64, 1000, 65_536, 1_048_577] {
            let mut generated = Generated::new(size);
            let mut sink = Sink::new();
            std::io::copy(&mut generated, &mut sink).unwrap();
            assert_eq!(sink.bytes, size, "asked for {size}");
        }
    }

    #[test]
    fn the_generator_is_deterministic_so_the_check_at_the_end_is_meaningful() {
        // The correctness check compares the data read back against a second
        // run of the generator. If the two differed, every run would fail; if
        // the generator were constant, the check would prove nothing.
        let mut first = Vec::new();
        std::io::copy(&mut Generated::new(4096), &mut first).unwrap();
        let mut second = Vec::new();
        std::io::copy(&mut Generated::new(4096), &mut second).unwrap();
        assert_eq!(first, second);
        assert_ne!(first[..64], first[64..128], "the data must not be constant");
    }

    #[test]
    fn a_stage_that_took_no_measurable_time_reports_zero_rather_than_infinity() {
        // Dividing by a zero duration produces `inf`, which formats as a
        // nonsense size and looks like a spectacular result.
        let stage = Stage {
            name: "x",
            bytes: 1000,
            elapsed: Duration::ZERO,
            note: String::new(),
        };
        assert!(stage.rate() < f64::EPSILON, "got {}", stage.rate());
    }

    #[test]
    fn durations_are_reported_in_units_a_person_can_act_on() {
        assert_eq!(human_time(45.0), "45 seconds");
        assert_eq!(human_time(600.0), "10 minutes");
        assert_eq!(human_time(7200.0), "2.0 hours");
        assert_eq!(human_time(864_000.0), "10.0 days");
    }
}
