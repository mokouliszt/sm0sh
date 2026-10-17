use sm0sh::{config::Config, server::Server};
use std::{path::PathBuf, process::ExitCode};
use tokio::sync::watch;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sm0sh: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut check = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => path = Some(PathBuf::from(args.next().ok_or("--config needs a path")?)),
            "--check-config" => check = true,
            "--version" | "-V" => {
                println!("sm0sh {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!("sm0sh [--config PATH] [--check-config]\n\nSLMP 3E binary command server for Ubuntu.\nWithout a config: TCP 127.0.0.1:5000, loopback clients only.");
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    let config = match path {
        Some(path) => Config::load(&path)?,
        None => Config::default(),
    };
    config.validate()?;
    if check {
        println!("configuration OK");
        return Ok(());
    }
    let transport = config.transport;
    let server = Server::bind(config).await?;
    eprintln!(
        "sm0sh {} listening on {} ({transport:?})",
        env!("CARGO_PKG_VERSION"),
        server.local_addr()
    );
    let (shutdown_sender, shutdown) = watch::channel(false);
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::spawn(async move {
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
        let _ = shutdown_sender.send(true);
    });
    server.run(shutdown).await?;
    Ok(())
}
