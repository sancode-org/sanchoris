use clap::{CommandFactory, Parser};

#[derive(Parser)]
#[command(
    name = "sanchoris-driver",
    version,
    about = "Runs sanchoris workspaces on this machine"
)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
    Cli::command().print_help().expect("print help");
    println!();
}
