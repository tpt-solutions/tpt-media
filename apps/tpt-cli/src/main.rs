//! `tpt` — the TPT media command-line binary.

use std::fs;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use tpt_cli::{self, Outcome};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let args_without_prog: Vec<String> = argv.iter().skip(1).cloned().collect();

    let ctx = tpt_cli::commands::Ctx {
        read_file: &|path| fs::read(path).map_err(|e| e.to_string()),
        read_stdin: &|| read_all_stdin().map_err(|e| e.to_string()),
        write_file: &|path, bytes| fs::write(path, bytes).map_err(|e| e.to_string()),
    };

    let outcome: Outcome = tpt_cli::run(&args_without_prog, &ctx);
    if !outcome.out.is_empty() {
        print!("{}", outcome.out);
        io::stdout().flush().ok();
    }
    if let Some(bytes) = outcome.binary_stdout {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(&bytes);
        let _ = stdout.flush();
    }
    if !outcome.err.is_empty() {
        eprint!("{}", outcome.err);
    }
    let code = u8::try_from(outcome.code).unwrap_or(1);
    ExitCode::from(code)
}

fn read_all_stdin() -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    io::stdin().lock().read_to_end(&mut buf)?;
    Ok(buf)
}
