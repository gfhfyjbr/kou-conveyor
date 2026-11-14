//! The command line client as a program of its own. `kou-desktop` embeds the
//! same [`kou_ipc::cli::run`]; this binary exists so the client can be driven
//! as a real process (this crate's tests do) before the app is built.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = kou_ipc::cli::run(&args).unwrap_or_else(|| {
        eprintln!("kou-ipc-cli: not a client command; kou-desktop would start the app here");
        kou_ipc::cli::EXIT_USAGE
    });
    // Not a return: a hook may have left a thread blocked on a stdin that its
    // harness keeps open.
    std::process::exit(status);
}
