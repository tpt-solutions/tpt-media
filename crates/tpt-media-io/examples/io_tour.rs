//! A tour of `tpt-media-io`'s byte-exact audio and raster I/O.
//!
//! Generates a sine tone, serializes it to WAV bytes, re-parses them,
//! then does the same with a checkerboard test pattern in PPM — no
//! external crates, no files unless you pass an output directory:
//!
//! ```text
//! cargo run -p tpt-media-io --example io_tour [output-dir]
//! ```

use std::path::Path;

use tpt_media_io::IoResult;
use tpt_media_io::{checkerboard, decode_wav, read_raster, sine_wave, write_raster, write_wav};

fn main() -> IoResult<()> {
    let out_dir = std::env::args().nth(1);
    if let Some(dir) = &out_dir {
        std::fs::create_dir_all(dir).map_err(|e| tpt_media_io::IoError::Other(e.to_string()))?;
    }

    // --- Audio: tone → WAV bytes → parse back -------------------------
    let tone = sine_wave(440.0, 48_000, 1.0, 0.5);
    let wav = write_wav(std::slice::from_ref(&tone))?;
    let reparsed = decode_wav(&wav)?;
    assert_eq!(
        reparsed.sample_count, tone.sample_count,
        "WAV must round-trip"
    );
    assert_eq!(
        reparsed.samples, tone.samples,
        "WAV codec path is byte-exact"
    );

    let rms: f64 = (0..reparsed.sample_count as usize)
        .map(|i| {
            let v = reparsed.s16(0, i).unwrap() as f64;
            v * v
        })
        .sum::<f64>()
        / reparsed.sample_count as f64;
    println!(
        "audio: {} Hz, {} samples, WAV {} bytes, tone RMS {:.1} LSB",
        reparsed.sample_rate,
        reparsed.sample_count,
        wav.len(),
        rms.sqrt()
    );

    // --- Video: checkerboard → PPM bytes → parse back -----------------
    let pattern = checkerboard(64, 64, tpt_media_core::PixelFormat::Rgb24, 8)?;
    let ppm = write_raster(&pattern)?;
    let frame = read_raster(&ppm)?;
    assert_eq!((frame.width, frame.height), (64, 64));
    assert_eq!(frame.pixels, pattern.pixels, "PPM must round-trip");
    println!(
        "video : {}x{} {:?}, PPM {} bytes",
        frame.width,
        frame.height,
        frame.format,
        ppm.len()
    );

    // Optionally drop both files on disk to inspect them elsewhere.
    if let Some(dir) = out_dir {
        let wav_path = Path::new(&dir).join("tone.wav");
        let ppm_path = Path::new(&dir).join("checker.ppm");
        std::fs::write(&wav_path, &wav).map_err(|e| tpt_media_io::IoError::Other(e.to_string()))?;
        std::fs::write(&ppm_path, &ppm).map_err(|e| tpt_media_io::IoError::Other(e.to_string()))?;
        println!("wrote  {} and {}", wav_path.display(), ppm_path.display());
    }
    Ok(())
}
