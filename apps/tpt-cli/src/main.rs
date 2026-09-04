use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "tpt",
    version,
    about = "tpt-media: encode, decode, and probe .tpt media files",
    long_about = "tpt-media is a 100% patent-free codec suite (MIT OR Apache-2.0).\n\
                  See https://github.com/tpt-solutions/tpt-media for details."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Encode an input media file into a .tpt container.
    Encode {
        /// Input path.
        #[arg(short, long)]
        input: String,
        /// Output .tpt path.
        #[arg(short, long)]
        output: String,
        /// Codec id: pulse | aura | vox | livelink | vault | whisper.
        #[arg(short, long)]
        codec: String,
        /// Enable lossless mode (only meaningful for aura).
        #[arg(long, default_value_t = false)]
        lossless: bool,
    },
    /// Decode a .tpt container back to a raw media file.
    Decode {
        /// Input .tpt path.
        #[arg(short, long)]
        input: String,
        /// Output path.
        #[arg(short, long)]
        output: String,
    },
    /// Print container and track metadata for a .tpt file.
    Probe {
        /// Input .tpt path.
        #[arg(short, long)]
        input: String,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Encode { input, output, codec, lossless } => {
            eprintln!("encode: {input} -> {output} (codec={codec}, lossless={lossless}) [stub]");
        }
        Command::Decode { input, output } => {
            eprintln!("decode: {input} -> {output} [stub]");
        }
        Command::Probe { input } => {
            eprintln!("probe: {input} [stub]");
        }
    }
}