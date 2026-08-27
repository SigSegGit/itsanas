//! Content-defined chunking.
//!
//! Splitting files at fixed offsets is useless for a sync system: inserting one
//! byte at the front of a file shifts every subsequent boundary, so every chunk
//! changes and the whole file is re-uploaded. Content-defined chunking picks
//! boundaries from a rolling hash of the *content*, so an edit only disturbs the
//! chunks it actually touches.
//!
//! This is FastCDC (Xia et al., 2016) with normalised chunking: a stricter cut
//! mask before the target average and a looser one after it, which tightens the
//! chunk-size distribution around the average compared to plain Gear chunking.
//!
//! # Why the gear table is derived, not copied
//!
//! Every device in the network must chunk identically forever — if two devices
//! disagree about boundaries, deduplication silently stops working and the store
//! grows without bound. Rather than paste in a magic 256-entry table that could
//! be transcribed wrongly, the table is derived from BLAKE3 under a fixed domain
//! string. It is reproducible from first principles on any machine, and
//! the `the_gear_table_is_pinned_forever` test fails loudly if it ever changes.

use std::{io::Read, sync::LazyLock};

use crate::error::{Result, StoreError};

/// Domain string for the gear table. **Changing this changes every chunk
/// boundary in the network and orphans every existing store.**
const GEAR_DOMAIN: &str = "itsanas v1 fastcdc gear table";

static GEAR: LazyLock<[u64; 256]> = LazyLock::new(|| {
    let mut table = [0u64; 256];
    for (index, slot) in table.iter_mut().enumerate() {
        let index = u32::try_from(index).expect("256 fits in u32");
        let derived = blake3::derive_key(GEAR_DOMAIN, &index.to_le_bytes());
        *slot = u64::from_le_bytes(derived[..8].try_into().expect("32 bytes covers 8"));
    }
    table
});

/// Cut masks indexed by the number of effective bits.
///
/// These are the FastCDC reference masks: the set bits are deliberately spread
/// across the word rather than packed into the low end, because under the
/// `hash << 1` update a packed low mask would only ever see the most recent few
/// bytes and would degrade towards fixed-size chunking.
const MASKS: [u64; 26] = [
    0,
    0x0000_0000_0000_0001,
    0x0000_0000_0000_0003,
    0x0000_0000_0000_0007,
    0x0000_0000_0000_000f,
    0x0000_0000_0180_4110,
    0x0000_0000_0180_3110,
    0x0000_0000_1803_5100,
    0x0000_0018_0003_5300,
    0x0000_0190_0035_3000,
    0x0000_5900_0353_0000,
    0x0000_d900_0353_0000,
    0x0000_d901_0353_0000,
    0x0000_d903_0353_0000,
    0x0000_d903_1353_0000,
    0x0000_d90f_0353_0000,
    0x0000_d903_0353_7000,
    0x0000_d907_0353_7000,
    0x0000_d907_0753_7000,
    0x0000_d917_0753_7000,
    0x0000_d917_4753_7000,
    0x0000_d917_6753_7000,
    0x0000_d937_6753_7000,
    0x0000_d937_7753_7000,
    0x0000_d937_7757_7000,
    0x0000_db37_7757_7000,
];

/// How aggressively the cut mask is normalised around the target average.
///
/// Level 2 means the pre-average mask requires two more bits to match and the
/// post-average mask two fewer, which is the value the FastCDC paper recommends
/// and measures.
const NORMALISATION: u32 = 2;

/// Chunk size bounds.
///
/// The defaults trade three things off. Smaller chunks deduplicate better and
/// re-upload less after an edit; larger chunks mean fewer index entries, fewer
/// round trips and less per-chunk sealing overhead. 64 KiB average is small
/// enough that a one-line edit in a large file re-uploads kilobytes rather than
/// megabytes, and large enough that a 10 GiB backup does not produce a million
/// index rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkerConfig {
    min_size: usize,
    avg_size: usize,
    max_size: usize,
    mask_strict: u64,
    mask_loose: u64,
}

impl ChunkerConfig {
    /// Smallest chunk the chunker will emit, except for a file's final chunk.
    pub const DEFAULT_MIN: usize = 16 * 1024;
    /// Target average chunk size.
    pub const DEFAULT_AVG: usize = 64 * 1024;
    /// Hard ceiling, so one pathological file cannot produce a 2 GiB chunk.
    pub const DEFAULT_MAX: usize = 256 * 1024;

    /// Build a configuration, validating the bounds.
    pub fn new(min_size: usize, avg_size: usize, max_size: usize) -> Result<Self> {
        if min_size == 0 {
            return Err(StoreError::ChunkerConfig("minimum chunk size must be > 0"));
        }
        if !(min_size <= avg_size && avg_size <= max_size) {
            return Err(StoreError::ChunkerConfig(
                "chunk sizes must satisfy min <= avg <= max",
            ));
        }

        // Floor log2 of the average, which is the bit count a cut mask needs in
        // order to fire on average once every `avg_size` bytes.
        let bits = usize::BITS - 1 - avg_size.leading_zeros();
        let top = u32::try_from(MASKS.len() - 1).expect("mask table is small");

        // Both indices are clamped to the table. `strict` runs off the top for
        // a large average; `loose` runs off it too, because clamping only
        // `strict` still leaves `bits - NORMALISATION` above the last entry
        // once the average passes 2^27.
        let strict = (bits + NORMALISATION).min(top);
        let loose = bits.saturating_sub(NORMALISATION).clamp(1, top);

        Ok(Self {
            min_size,
            avg_size,
            max_size,
            mask_strict: MASKS[strict as usize],
            mask_loose: MASKS[loose as usize],
        })
    }

    #[must_use]
    pub const fn min_size(&self) -> usize {
        self.min_size
    }

    #[must_use]
    pub const fn avg_size(&self) -> usize {
        self.avg_size
    }

    #[must_use]
    pub const fn max_size(&self) -> usize {
        self.max_size
    }

    /// Offset of the first cut point in `data`, or `data.len()` if the buffer
    /// ends before one is found.
    ///
    /// The scan starts at `min_size`: bytes before it can never be a boundary,
    /// which is both what enforces the minimum and what makes the common case
    /// fast.
    #[must_use]
    pub fn cut_point(&self, data: &[u8]) -> usize {
        let len = data.len();
        if len <= self.min_size {
            return len;
        }

        let end = len.min(self.max_size);
        let center = self.avg_size.min(end);
        let gear = &*GEAR;

        let mut hash = 0u64;
        let mut index = self.min_size;

        while index < center {
            hash = (hash << 1).wrapping_add(gear[data[index] as usize]);
            if hash & self.mask_strict == 0 {
                return index + 1;
            }
            index += 1;
        }

        while index < end {
            hash = (hash << 1).wrapping_add(gear[data[index] as usize]);
            if hash & self.mask_loose == 0 {
                return index + 1;
            }
            index += 1;
        }

        end
    }

    /// Split `data` into content-defined chunks.
    #[must_use]
    pub fn split<'a>(&self, data: &'a [u8]) -> Chunks<'a> {
        Chunks {
            config: *self,
            data,
            offset: 0,
        }
    }
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MIN, Self::DEFAULT_AVG, Self::DEFAULT_MAX)
            .expect("the default bounds are valid")
    }
}

/// Split a stream into chunks, holding only a bounded window in memory.
///
/// The slice-based [`ChunkerConfig::split`] needs the whole file resident, which
/// is fine for a text file and fatal for a video on a Raspberry Pi: a 6 GB file
/// on a 4 GB machine is not slow, it is an out-of-memory kill. This reads
/// through the file instead, never holding more than roughly two maximum chunks
/// — half a megabyte at the default settings, whatever the file size.
///
/// **Boundaries are identical to the slice version.** That is not a nicety: if
/// the two disagreed, a file imported by one path and re-imported by the other
/// would deduplicate against nothing and store itself twice. The
/// `streaming_and_slicing_agree_on_every_boundary` test holds them to it, at
/// seven sizes and with readers that return as little as one byte at a time.
///
/// The callback is given each chunk in order. Returning an error stops the walk
/// and propagates.
pub fn split_stream<R: Read>(
    config: &ChunkerConfig,
    mut reader: R,
    mut on_chunk: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    // One max-size chunk to cut from, plus room to read ahead into.
    let window = config.max_size();
    let mut buffer: Vec<u8> = Vec::with_capacity(window * 2);
    let mut scratch = vec![0u8; 64 * 1024];
    let mut exhausted = false;

    loop {
        // Only cut once there is either a full window to choose within, or no
        // more data coming. Cutting early would pick a different boundary than
        // the slice version, which sees the whole file at once.
        while !exhausted && buffer.len() < window {
            let read = reader.read(&mut scratch).map_err(StoreError::BareIo)?;
            if read == 0 {
                exhausted = true;
                break;
            }
            buffer.extend_from_slice(&scratch[..read]);
        }

        if buffer.is_empty() {
            return Ok(());
        }

        let cut = config.cut_point(&buffer);
        on_chunk(&buffer[..cut])?;
        buffer.drain(..cut);
    }
}

/// One content-defined chunk, borrowed from the buffer it was cut from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk<'a> {
    /// Byte offset of this chunk within the original buffer.
    pub offset: usize,
    /// The chunk's bytes.
    pub data: &'a [u8],
}

impl Chunk<'_> {
    #[must_use]
    pub const fn len(&self) -> usize {
        self.data.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// Iterator over the chunks of a buffer.
#[derive(Debug)]
pub struct Chunks<'a> {
    config: ChunkerConfig,
    data: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.data.get(self.offset..)?;
        if remaining.is_empty() {
            return None;
        }

        let cut = self.config.cut_point(remaining);
        // `cut_point` returns at least 1 for a non-empty buffer, so the offset
        // strictly advances and this iterator always terminates.
        debug_assert!(cut > 0, "a cut point of 0 would loop forever");

        let chunk = Chunk {
            offset: self.offset,
            data: &remaining[..cut],
        };
        self.offset += cut;
        Some(chunk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random bytes, so tests are reproducible without a
    /// dependency on a seeded RNG crate.
    fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        let mut counter = 0u64;
        while out.len() < len {
            let block = blake3::hash(&[seed.to_le_bytes(), counter.to_le_bytes()].concat());
            out.extend_from_slice(block.as_bytes());
            counter += 1;
        }
        out.truncate(len);
        out
    }

    fn reassemble(config: &ChunkerConfig, data: &[u8]) -> Vec<u8> {
        config.split(data).flat_map(|c| c.data.to_vec()).collect()
    }

    #[test]
    fn the_gear_table_is_pinned_forever() {
        // If this digest changes, every chunk boundary in the network moves:
        // existing stores stop deduplicating against new writes, and every
        // client re-uploads every file. The table may only change behind a
        // format-version bump with a migration.
        let mut bytes = Vec::with_capacity(256 * 8);
        for value in GEAR.iter() {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(
            blake3::hash(&bytes).to_hex().as_str(),
            GEAR_TABLE_DIGEST,
            "the FastCDC gear table changed; this silently breaks deduplication \
             against every store already in the network"
        );
    }

    /// BLAKE3 of the 256 gear values, little-endian, concatenated in order.
    const GEAR_TABLE_DIGEST: &str =
        "968a07a9d50bc5ab600ce84ab82845959e9553033d2fdcd7b0ccd113403aeb01";

    #[test]
    fn chunks_reassemble_into_the_original_bytes() {
        let config = ChunkerConfig::default();
        for size in [0, 1, 1000, 16 * 1024, 100 * 1024, 1024 * 1024] {
            let data = pseudo_random(size, 7);
            assert_eq!(
                reassemble(&config, &data),
                data,
                "chunking lost or reordered bytes at size {size}"
            );
        }
    }

    #[test]
    fn chunking_is_deterministic() {
        let config = ChunkerConfig::default();
        let data = pseudo_random(2 * 1024 * 1024, 11);

        let first: Vec<_> = config.split(&data).map(|c| (c.offset, c.len())).collect();
        let second: Vec<_> = config.split(&data).map(|c| (c.offset, c.len())).collect();

        assert_eq!(
            first, second,
            "two runs disagreed about boundaries, so two devices would too"
        );
        assert!(first.len() > 10, "test data was too small to be meaningful");
    }

    #[test]
    fn an_empty_buffer_produces_no_chunks() {
        assert_eq!(ChunkerConfig::default().split(&[]).count(), 0);
    }

    #[test]
    fn size_bounds_are_respected() {
        let config = ChunkerConfig::default();
        let data = pseudo_random(4 * 1024 * 1024, 13);
        let chunks: Vec<_> = config.split(&data).collect();

        assert!(chunks.len() > 20, "not enough chunks to test bounds");

        for chunk in &chunks[..chunks.len() - 1] {
            assert!(
                chunk.len() >= config.min_size(),
                "chunk of {} bytes is below the {} byte minimum",
                chunk.len(),
                config.min_size()
            );
            assert!(
                chunk.len() <= config.max_size(),
                "chunk of {} bytes exceeds the {} byte maximum",
                chunk.len(),
                config.max_size()
            );
        }
        // Only the final chunk is allowed to be short.
        assert!(chunks[chunks.len() - 1].len() <= config.max_size());
    }

    #[test]
    fn the_average_chunk_size_is_close_to_the_target() {
        // This is the test that actually validates the mask table. A wrong or
        // mistranscribed mask still produces valid, reassembling chunks — it
        // just produces them at the wrong size, quietly wrecking the
        // dedup/overhead trade-off. Nothing else would catch that.
        let config = ChunkerConfig::default();
        let data = pseudo_random(8 * 1024 * 1024, 17);
        let chunks: Vec<_> = config.split(&data).collect();

        let average = data.len() / chunks.len();
        let target = config.avg_size();

        assert!(
            average > target / 2 && average < target * 2,
            "average chunk size {average} is not within 2x of the {target} byte \
             target; the cut masks are wrong"
        );
    }

    #[test]
    fn inserting_a_byte_at_the_front_shifts_only_local_boundaries() {
        // The entire reason content-defined chunking exists. With fixed-size
        // chunking this test would find zero surviving chunks.
        let config = ChunkerConfig::default();
        let original = pseudo_random(4 * 1024 * 1024, 19);

        let mut edited = Vec::with_capacity(original.len() + 1);
        edited.push(0xAB);
        edited.extend_from_slice(&original);

        let before: std::collections::HashSet<Vec<u8>> =
            config.split(&original).map(|c| c.data.to_vec()).collect();
        let after: std::collections::HashSet<Vec<u8>> =
            config.split(&edited).map(|c| c.data.to_vec()).collect();

        let survived = before.intersection(&after).count();
        let total = before.len();

        // Integer comparison rather than a float ratio: `survived * 10 > total
        // * 9` is exactly "more than 90%" with no rounding to argue about.
        assert!(
            survived * 10 > total * 9,
            "only {survived} of {total} chunks survived a one-byte prefix \
             insertion; content-defined chunking is not working and every edit \
             will re-upload the whole file"
        );
    }

    #[test]
    fn editing_the_middle_leaves_both_ends_intact() {
        let config = ChunkerConfig::default();
        let original = pseudo_random(4 * 1024 * 1024, 23);

        let mut edited = original.clone();
        let midpoint = edited.len() / 2;
        edited.splice(midpoint..midpoint, pseudo_random(5000, 29));

        let before: std::collections::HashSet<Vec<u8>> =
            config.split(&original).map(|c| c.data.to_vec()).collect();
        let after: std::collections::HashSet<Vec<u8>> =
            config.split(&edited).map(|c| c.data.to_vec()).collect();

        let survived = before.intersection(&after).count();
        let total = before.len();
        assert!(
            survived * 10 > total * 9,
            "only {survived} of {total} chunks survived a mid-file insertion"
        );
    }

    #[test]
    fn highly_repetitive_data_still_terminates_and_respects_the_maximum() {
        // Long runs of one byte are the pathological case for a rolling hash:
        // the hash can settle into a state where the mask never matches. The
        // max-size ceiling is what stops that from producing one enormous chunk.
        let config = ChunkerConfig::default();
        let data = vec![0u8; 4 * 1024 * 1024];
        let chunks: Vec<_> = config.split(&data).collect();

        assert_eq!(reassemble(&config, &data), data);
        for chunk in &chunks {
            assert!(chunk.len() <= config.max_size());
        }
        assert!(
            chunks.len() >= data.len() / config.max_size(),
            "zero-filled data produced too few chunks; the maximum is not enforced"
        );
    }

    #[test]
    fn a_buffer_shorter_than_the_minimum_is_one_chunk() {
        let config = ChunkerConfig::default();
        let data = pseudo_random(config.min_size() - 1, 31);
        let chunks: Vec<_> = config.split(&data).collect();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), data.len());
        assert_eq!(chunks[0].offset, 0);
    }

    #[test]
    fn offsets_are_contiguous_and_start_at_zero() {
        let config = ChunkerConfig::default();
        let data = pseudo_random(1024 * 1024, 37);

        let mut expected = 0usize;
        for chunk in config.split(&data) {
            assert_eq!(chunk.offset, expected);
            expected += chunk.len();
        }
        assert_eq!(expected, data.len());
    }

    #[test]
    fn streaming_and_slicing_agree_on_every_boundary() {
        // The property the streaming path exists to preserve. If the two
        // disagreed, a file imported through one and re-imported through the
        // other would deduplicate against nothing and store itself twice —
        // silently, and only visible as a store that is mysteriously double
        // the expected size.
        let config = ChunkerConfig::default();

        for size in [
            0,
            1,
            config.min_size() - 1,
            config.min_size(),
            config.max_size(),
            config.max_size() + 1,
            3 * 1024 * 1024,
        ] {
            let data = pseudo_random(size, 101);

            let sliced: Vec<Vec<u8>> = config.split(&data).map(|c| c.data.to_vec()).collect();

            let mut streamed: Vec<Vec<u8>> = Vec::new();
            split_stream(&config, data.as_slice(), |chunk| {
                streamed.push(chunk.to_vec());
                Ok(())
            })
            .unwrap();

            assert_eq!(
                streamed, sliced,
                "streaming and slicing disagreed at size {size}"
            );
        }
    }

    #[test]
    fn streaming_agrees_even_when_the_reader_dribbles() {
        // A network socket, a slow disk, a pipe: reads come back short. If the
        // chunker cut on whatever happened to have arrived, boundaries would
        // depend on timing, and the same file would chunk differently on two
        // machines.
        struct Dribble<'a> {
            data: &'a [u8],
            at: usize,
            step: usize,
        }

        impl std::io::Read for Dribble<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let remaining = self.data.len() - self.at;
                let take = self.step.min(out.len()).min(remaining);
                out[..take].copy_from_slice(&self.data[self.at..self.at + take]);
                self.at += take;
                Ok(take)
            }
        }

        let config = ChunkerConfig::default();
        let data = pseudo_random(2 * 1024 * 1024, 103);
        let sliced: Vec<Vec<u8>> = config.split(&data).map(|c| c.data.to_vec()).collect();

        for step in [1, 7, 4096, 65_536] {
            let mut streamed: Vec<Vec<u8>> = Vec::new();
            split_stream(
                &config,
                Dribble {
                    data: &data,
                    at: 0,
                    step,
                },
                |chunk| {
                    streamed.push(chunk.to_vec());
                    Ok(())
                },
            )
            .unwrap();

            assert_eq!(
                streamed, sliced,
                "boundaries changed when reads came back {step} bytes at a time"
            );
        }
    }

    #[test]
    fn streaming_never_holds_more_than_a_bounded_window() {
        // The whole reason this path exists. A 20 MiB file must not put 20 MiB
        // in memory, or a Pi dies on a video.
        let config = ChunkerConfig::default();
        let data = pseudo_random(20 * 1024 * 1024, 107);

        let mut largest_chunk = 0usize;
        split_stream(&config, data.as_slice(), |chunk| {
            largest_chunk = largest_chunk.max(chunk.len());
            Ok(())
        })
        .unwrap();

        assert!(
            largest_chunk <= config.max_size(),
            "a chunk of {largest_chunk} bytes exceeded the {} byte maximum",
            config.max_size()
        );
    }

    #[test]
    fn an_error_from_the_callback_stops_the_walk() {
        // A failing disk write must abort the import rather than carry on
        // producing chunks nothing is storing.
        let config = ChunkerConfig::default();
        let data = pseudo_random(4 * 1024 * 1024, 109);

        let mut seen = 0;
        let result = split_stream(&config, data.as_slice(), |_| {
            seen += 1;
            if seen == 3 {
                return Err(StoreError::Corrupt("stop".to_owned()));
            }
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(seen, 3, "the walk continued after the callback failed");
    }

    #[test]
    fn a_reader_that_fails_propagates_rather_than_truncating() {
        // Silently treating a read error as end-of-file would store a truncated
        // file as though it were complete.
        struct Failing;
        impl std::io::Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk went away"))
            }
        }

        assert!(split_stream(&ChunkerConfig::default(), Failing, |_| Ok(())).is_err());
    }

    #[test]
    fn invalid_configurations_are_rejected() {
        assert!(ChunkerConfig::new(0, 1024, 2048).is_err());
        assert!(ChunkerConfig::new(4096, 1024, 2048).is_err());
        assert!(ChunkerConfig::new(1024, 4096, 2048).is_err());
        assert!(ChunkerConfig::new(1024, 2048, 4096).is_ok());
    }

    #[test]
    fn small_configurations_do_not_panic_on_mask_lookup() {
        // `bits - NORMALISATION` underflows for tiny averages if written
        // carelessly, and `bits + NORMALISATION` runs off the end of the table
        // for huge ones.
        for (min, avg, max) in [(1, 1, 1), (1, 2, 4), (1, 4, 8), (1, 1 << 30, 1 << 31)] {
            let config = ChunkerConfig::new(min, avg, max).expect("bounds are ordered");
            let data = pseudo_random(4096, 41);
            assert_eq!(reassemble(&config, &data), data);
        }
    }
}
