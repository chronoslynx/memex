use anyhow::{Context, Result};
use std::time::Instant;

use memex::{api, build_index};

#[derive(structopt::StructOpt)]
struct Args {
    /// Port to listen on.
    #[structopt(
        short = "t",
        long = "threads",
        env = "INGEST_THREADS",
        default_value = "8"
    )]
    threads: u16,

    /// Recursively ingest files in the provided directory.
    #[structopt(short = "s", long = "source")]
    src: String,
    /// Persist the index to the following directory. If not supplied the index will remain in RAM
    #[structopt(short = "d", long = "destination")]
    dest: Option<String>,
    /// Listen on the following host.
    #[structopt(short = "l", long = "host", default_value = "localhost")]
    host: String,
    /// Persist the index to the following directory. If not supplied the index will remain in RAM
    #[structopt(short = "d", long = "dir", default_value = "3000")]
    port: u16,
}

#[paw::main]
fn main(args: Args) -> Result<()> {
    let now = Instant::now();
    let index = build_index(args.src, args.dest, args.threads as usize)?;
    let elapsed_time = now.elapsed();
    println!("Build index in {} seconds.", elapsed_time.as_secs());
    let host = format!("{}:{}", args.host, args.port);
    api::serve(index, &host).context("Failed to serve index")
}
