/// Entry point for the Lanweave executable.
fn main() -> anyhow::Result<()> {
    if let Some(argument) = std::env::args().nth(1) {
        match argument.as_str() {
            "-V" | "--version" => {
                println!("lanweave {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-h" | "--help" => {
                println!(
                    "lanweave {}\n\nUsage: lanweave\n\nRun without arguments to open the terminal interface.\n",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            other => {
                anyhow::bail!("unrecognized argument `{other}`; run `lanweave --help`");
            }
        }
    }

    lanweave::run()
}
