//! Step 11: the compressed bundle and its manifest entry, for the timetable workflow.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};

use btt_timetable::LoadedTimetableBundle;
use btt_timetable::format::BUNDLE_FORMAT_VERSION;
use btt_timetable::manifest::{MINIMUM_APP_VERSION, ManifestEntry, last_valid_day};
use sha2::{Digest, Sha256};

use crate::BuildError;

/// Compression level of the download. The app only decompresses, which costs the same at every
/// level, so the build pays for the smallest file.
const COMPRESSION_LEVEL: i32 = 19;

/// The files the workflow uploads to the release of a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedFiles {
    /// The compressed bundle.
    pub compressed_bundle: PathBuf,
    /// The SHA-256 of the compressed bundle, in the format of `sha256sum`.
    pub checksum: PathBuf,
    /// The manifest entry as JSON.
    pub manifest_entry: PathBuf,
}

/// Compresses a bundle and writes its checksum and manifest entry next to it.
///
/// The release of a bundle is tagged `timetable-<version>` and holds the compressed file, so the
/// entry points at `<download_base_url>/timetable-<version>/timetable-<version>.bundle.zst`.
///
/// # Errors
///
/// Returns an error when a file cannot be read or written.
pub fn write_published_files(
    bundle_path: &Path,
    bundle: &LoadedTimetableBundle,
    download_base_url: &str,
) -> Result<PublishedFiles, BuildError> {
    let stem = bundle_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| io::Error::other("the bundle path has no file name"))?;
    let compressed_bundle = bundle_path.with_extension("bundle.zst");
    compress(bundle_path, &compressed_bundle)?;

    let sha256 = sha256_hex(&compressed_bundle)?;
    let compressed_name = format!("{stem}.bundle.zst");
    let checksum = compressed_bundle.with_extension("zst.sha256");
    std::fs::write(&checksum, format!("{sha256}  {compressed_name}\n"))?;

    let validity_start = bundle.validity_start();
    let entry = ManifestEntry {
        version: bundle.bundle_version(),
        format_version: BUNDLE_FORMAT_VERSION,
        valid_from: validity_start,
        valid_until: last_valid_day(validity_start, bundle.validity_days()),
        url: format!(
            "{}/{stem}/{compressed_name}",
            download_base_url.trim_end_matches('/')
        ),
        size_bytes: std::fs::metadata(&compressed_bundle)?.len(),
        sha256,
        minimum_app_version: MINIMUM_APP_VERSION.to_owned(),
    };
    let manifest_entry = bundle_path.with_extension("json");
    let json = serde_json::to_string_pretty(&entry).map_err(io::Error::other)?;
    std::fs::write(&manifest_entry, json)?;
    Ok(PublishedFiles {
        compressed_bundle,
        checksum,
        manifest_entry,
    })
}

/// Compresses a file with zstd, on as many threads as the machine has.
fn compress(source: &Path, destination: &Path) -> io::Result<()> {
    let mut input = BufReader::new(File::open(source)?);
    let mut encoder = zstd::stream::Encoder::new(
        BufWriter::new(File::create(destination)?),
        COMPRESSION_LEVEL,
    )?;
    let workers = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    encoder.multithread(u32::try_from(workers).unwrap_or(1))?;
    io::copy(&mut input, &mut encoder)?;
    encoder.finish()?;
    Ok(())
}

/// Returns the SHA-256 of a file as lowercase hexadecimal.
fn sha256_hex(path: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut file = File::open(path)?;
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read_bytes = file.read(&mut buffer)?;
        if read_bytes == 0 {
            break;
        }
        hasher.update(&buffer[..read_bytes]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            // Writing to a string never fails.
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compressed_file_decompresses_to_the_original_and_has_a_known_checksum() {
        let directory = std::env::temp_dir().join(format!("btt-publish-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source.bin");
        let compressed = directory.join("source.bin.zst");
        let contents: Vec<u8> = (0..100_000_u32)
            .flat_map(|n| (n % 251).to_le_bytes())
            .collect();
        std::fs::write(&source, &contents).unwrap();

        compress(&source, &compressed).unwrap();

        let restored = zstd::stream::decode_all(File::open(&compressed).unwrap()).unwrap();
        assert_eq!(restored, contents);
        assert!(std::fs::metadata(&compressed).unwrap().len() < contents.len() as u64);

        std::fs::write(&source, b"abc").unwrap();
        assert_eq!(
            sha256_hex(&source).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
