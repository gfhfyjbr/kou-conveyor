//! The host that keeps a Claude Code apart from the app, for the tests
//! (`tests/host.rs`): its command line is the host's own, as the app's
//! binary takes it after `claude-host`.

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(kou_claude::host::run(&arguments));
}
