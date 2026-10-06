//! What padding chunk sizes would cost, and what it would hide, on a real folder.
//!
//! HANDOVER §10 item 9 decided "pad, after measuring": the disk cost is measured
//! on a real folder first and the size classes are chosen from it. This is that
//! measurement, kept in the repository so the numbers can be re-taken on any
//! machine rather than believed.
//!
//! ```text
//! cargo run --release -p itsanas-store --example padding_cost -- <folder>...
//! ```
//!
//! It reads every file under the folders with the production chunker and, for
//! each candidate class set, reports two things:
//!
//! - **waste**: padded bytes over plaintext bytes, the extra disk every host pays;
//! - **recognisable**: among files of two chunks or more, the share whose
//!   sequence of padded sizes is shared with no other file in the corpus. A host
//!   holding a candidate copy of such a file recognises it from the sizes alone,
//!   which is the attack the padding is meant to close. A corpus is a small
//!   world, so this is a lower bound on what a host learns, not an upper one.
//!
//! Nothing leaves the machine and nothing is written: only aggregate counts are
//! printed, never a path or a name.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

use itsanas_store::{ChunkerConfig, split_stream};

/// A rule mapping a plaintext chunk length to the length it is padded to.
struct Classes {
    name: &'static str,
    pad: fn(u64) -> u64,
}

/// Smallest padded size, so a tiny file does not cost a whole class of its own.
const FLOOR: u64 = 4 * 1024;

fn none(len: u64) -> u64 {
    len
}

fn power_of_two(len: u64) -> u64 {
    len.max(FLOOR).next_power_of_two()
}

/// Four classes per doubling: 2^k, 2^k * 1.25, 1.5, 1.75 -- integers only.
fn quarter_steps(len: u64) -> u64 {
    let len = len.max(FLOOR);
    let base = (len + 1).next_power_of_two() / 2;
    let step = (base / 4).max(1);
    len.div_ceil(step) * step
}

/// Padmé (Nikitin et al., PETS 2019): keeps the top `log2(log2 L) + 1` bits of
/// the length and rounds the rest up. Overhead at most about 12 %, leaking
/// O(log log L) bits per length instead of O(log L).
fn padme(len: u64) -> u64 {
    let len = len.max(FLOOR);
    let exponent = 63 - u64::from(len.leading_zeros());
    let mantissa_bits = 64 - u64::from(exponent.leading_zeros());
    let dropped = exponent.saturating_sub(mantissa_bits);
    let mask = (1u64 << dropped) - 1;
    (len + mask) & !mask
}

/// Every chunk the size of the largest one: the only class set under which two
/// different files of one chunk count store sequences nobody can tell apart.
fn single_class(len: u64) -> u64 {
    let max = ChunkerConfig::DEFAULT_MAX as u64;
    if len <= max { max } else { len }
}

const CLASS_SETS: [Classes; 5] = [
    Classes {
        name: "none (today)",
        pad: none,
    },
    Classes {
        name: "Padme",
        pad: padme,
    },
    Classes {
        name: "4 per doubling",
        pad: quarter_steps,
    },
    Classes {
        name: "power of two",
        pad: power_of_two,
    },
    Classes {
        name: "one class (256 KiB)",
        pad: single_class,
    },
];

fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk(&entry.path(), out);
        } else if kind.is_file() {
            out.push(entry.path());
        }
    }
}

fn chunk_sizes(config: &ChunkerConfig, path: &Path) -> Option<Vec<u64>> {
    let file = File::open(path).ok()?;
    let mut sizes = Vec::new();
    split_stream(config, BufReader::new(file), |chunk| {
        sizes.push(chunk.len() as u64);
        Ok(())
    })
    .ok()?;
    Some(sizes)
}

#[allow(clippy::cast_precision_loss)] // a printed percentage, never accounting
fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

#[allow(clippy::cast_precision_loss)] // a printed size, never accounting
fn main() {
    let roots: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if roots.is_empty() {
        eprintln!("usage: padding_cost <folder>...");
        std::process::exit(2);
    }

    let mut paths = Vec::new();
    for root in &roots {
        walk(root, &mut paths);
    }

    let config = ChunkerConfig::default();
    let total = paths.len();
    let mut files: Vec<Vec<u64>> = Vec::with_capacity(total);
    for (done, path) in paths.iter().enumerate() {
        // Progress on stderr, so a long read is visibly alive and stdout stays
        // the result alone.
        if done % 1000 == 0 {
            eprintln!("{done} / {total} files read");
        }
        if let Some(sizes) = chunk_sizes(&config, path).filter(|sizes| !sizes.is_empty()) {
            files.push(sizes);
        }
    }

    let plaintext: u64 = files.iter().flatten().sum();
    let chunks: usize = files.iter().map(Vec::len).sum();
    let multi = files.iter().filter(|sizes| sizes.len() >= 2).count();
    println!(
        "{} files, {} chunks, {:.2} GiB of plaintext, {} files of two chunks or more",
        files.len(),
        chunks,
        plaintext as f64 / f64::from(1u32 << 30),
        multi
    );
    println!();
    println!(
        "{:<22} {:>8} {:>14} {:>16}",
        "classes", "waste", "recognisable", "distinct sizes"
    );

    for classes in &CLASS_SETS {
        let padded: u64 = files.iter().flatten().map(|&len| (classes.pad)(len)).sum();
        let mut sequences: HashMap<Vec<u64>, usize> = HashMap::new();
        let mut distinct: HashSet<u64> = HashSet::new();
        for sizes in &files {
            let padded_sizes: Vec<u64> = sizes.iter().map(|&len| (classes.pad)(len)).collect();
            for &size in &padded_sizes {
                distinct.insert(size);
            }
            if padded_sizes.len() >= 2 {
                *sequences.entry(padded_sizes).or_default() += 1;
            }
        }
        let unique = sequences.values().filter(|&&count| count == 1).count();
        println!(
            "{:<22} {:>7.1}% {:>13.1}% {:>16}",
            classes.name,
            percent(padded - plaintext, plaintext),
            percent(unique as u64, multi as u64),
            distinct.len()
        );
    }
}
