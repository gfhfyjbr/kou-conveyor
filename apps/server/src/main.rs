use std::{env, net::SocketAddr, path::PathBuf};

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let mut directory = None;
    let mut assets = None;
    let mut address: SocketAddr = "127.0.0.1:8091".parse()?;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("expected value after flag")?;
        match flag.to_str() {
            Some("-session-dir" | "--session-dir") => directory = Some(PathBuf::from(value)),
            Some("-assets" | "--assets") => assets = Some(PathBuf::from(value)),
            Some("-listen" | "--listen") => address = value.to_str().ok_or("invalid listen address")?.parse()?,
            _ => return Err("usage: kou-conveyor-web-rs --session-dir DIR [--listen 127.0.0.1:8091] [--assets DIR]".into()),
        }
    }
    if !address.ip().is_loopback() {
        return Err("the migration server only listens on loopback".into());
    }
    let directory =
        directory.ok_or("--session-dir is required; no default user data directory is opened")?;
    let state = kou_server::Server::new(directory, assets)?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    eprintln!(
        "Rust migration server (read-only): http://{}",
        listener.local_addr()?
    );
    axum_serve(listener, state).await
}

async fn axum_serve(
    listener: tokio::net::TcpListener,
    state: kou_server::Server,
) -> Result<(), Box<dyn std::error::Error>> {
    kou_server::serve(listener, state).await?;
    Ok(())
}
