//! The MCP server and the command line client as one program, the way
//! `kou-desktop` holds both: `kou-mcp mcp` (or no arguments) serves MCP on
//! stdio, and `kou-mcp pane list` and the other client commands run the
//! command line. It exists so both can be driven as real processes, by this
//! crate's tests and by an MCP client, before the app is wired to them.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args == ["mcp"] {
        let env = kou_mcp::Env::from_process();
        let status = kou_mcp::serve(std::io::stdin().lock(), std::io::stdout(), &env);
        std::process::exit(status);
    }
    let status = kou_ipc::cli::run(&args).unwrap_or_else(|| {
        eprintln!("kou-mcp: give `mcp` to serve MCP, or a command line client command");
        kou_ipc::cli::EXIT_USAGE
    });
    // Not a return: a hook may have left a thread blocked on a stdin that its
    // harness keeps open.
    std::process::exit(status);
}
