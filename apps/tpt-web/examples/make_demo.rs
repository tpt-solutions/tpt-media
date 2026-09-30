//! Generate the demo asset `web/demo.tpt` (4 s of synthetic Cinder audio and
//! 160x96 Thrum video at 15 fps, encoded by the real encoders; see
//! `tpt_web::synth`), or the benchmark inputs. Deterministic: rerunning
//! produces identical bytes.
//!
//! ```text
//! cargo run -p tpt-web --example make_demo                      # web/demo.tpt
//! cargo run -p tpt-web --example make_demo -- out.tpt           # anywhere else
//! cargo run -p tpt-web --example make_demo -- --bench-dir DIR   # all benchmark inputs
//! ```

use std::path::PathBuf;
use tpt_web::synth::{self, AudioKind};

fn write(path: &PathBuf, bytes: &[u8]) {
    if let Err(e) = std::fs::write(path, bytes) {
        eprintln!("cannot write {}: {e}", path.display());
        std::process::exit(1);
    }
    println!("wrote {} ({} bytes)", path.display(), bytes.len());
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    match args.next() {
        Some(flag) if flag == "--bench-dir" => {
            let Some(dir) = args.next().map(PathBuf::from) else {
                eprintln!("--bench-dir needs a directory");
                std::process::exit(2);
            };
            if let Err(e) = std::fs::create_dir_all(&dir) {
                eprintln!("cannot create {}: {e}", dir.display());
                std::process::exit(1);
            }
            for fx in synth::benchmark_fixtures() {
                write(&dir.join(format!("{}.tpt", fx.id)), &fx.bytes);
            }
        }
        other => {
            let out = other.map(PathBuf::from).unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("web")
                    .join("demo.tpt")
            });
            write(
                &out,
                &synth::av_tpt(AudioKind::Cinder, 48_000, 4.0, 160, 96, 15),
            );
        }
    }
}
