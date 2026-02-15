mod api;
mod config;
mod db;
mod email;
mod httpui;
mod openrouter;
mod tui;

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::oneshot;
use tracing::Level;

#[derive(Debug, Clone)]
struct AppConfig {
    addr: String,
    data_dir: PathBuf,
    headless: bool,
}

fn is_tty() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

#[derive(Debug, PartialEq, Eq)]
enum CliExit {
    Help,
    Version,
    Error(String),
}

fn usage() -> String {
    format!(
        "\
saelora {}

Usage:
  saelora [--addr <HOST:PORT>] [--data-dir <PATH>] [--headless]

Options:
  --addr <HOST:PORT>     Bind address (default: 127.0.0.1:8080)
  --data-dir <PATH>      Data directory (default: ./data)
  --headless             Run without the TUI (server-only)
  -h, --help             Show this help and exit
  --version              Show version and exit
",
        env!("CARGO_PKG_VERSION")
    )
}

fn parse_args_from<I, S>(args: I) -> Result<AppConfig, CliExit>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    // Keep flags minimal; default experience is interactive.
    let mut addr = "127.0.0.1:8080".to_string();
    let mut data_dir = PathBuf::from("./data");
    let mut headless = false;

    let mut it = args.into_iter().map(Into::into);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Err(CliExit::Help),
            "--version" => return Err(CliExit::Version),
            "--addr" => {
                let Some(v) = it.next() else {
                    return Err(CliExit::Error("missing value for --addr".to_string()));
                };
                addr = v;
            }
            "--data-dir" => {
                let Some(v) = it.next() else {
                    return Err(CliExit::Error("missing value for --data-dir".to_string()));
                };
                data_dir = PathBuf::from(v);
            }
            "--headless" => headless = true,
            _ => return Err(CliExit::Error(format!("unknown flag: {a}"))),
        }
    }

    Ok(AppConfig {
        addr,
        data_dir,
        headless,
    })
}

fn parse_args() -> Result<AppConfig, CliExit> {
    parse_args_from(std::env::args().skip(1))
}

fn init_logging(data_dir: &PathBuf, interactive: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    if interactive {
        let _ = std::fs::create_dir_all(data_dir);
        let log_path = data_dir.join("saelora.log");
        let file_appender = tracing_appender::rolling::never(
            log_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new(".")),
            log_path.file_name().unwrap_or_default(),
        );
        let (nb, _guard) = tracing_appender::non_blocking(file_appender);

        // Keep the guard alive for the process lifetime.
        Box::leak(Box::new(_guard));

        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_max_level(Level::INFO)
            .with_writer(nb)
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_max_level(Level::INFO)
            .with_writer(std::io::stderr)
            .init();
    }
}

fn listen_addrs(addr: &str) -> Vec<SocketAddr> {
    // Mirror the Go behavior: bind both loopback families when reasonable to avoid "it doesn't run" confusion.
    if let Some(port) = addr.strip_prefix(':').and_then(|p| p.parse::<u16>().ok()) {
        // Go's ":8080" binds on all interfaces; accept the same shorthand.
        // We try both families; the second is best-effort like the loopback behavior below.
        let mut out = Vec::<SocketAddr>::new();
        if let Ok(sa) = format!("0.0.0.0:{port}").parse() {
            out.push(sa);
        }
        if let Ok(sa) = format!("[::]:{port}").parse() {
            out.push(sa);
        }
        return out;
    }

    let Ok((host, port)) = split_host_port(addr) else {
        return addr.parse().map(|sa| vec![sa]).unwrap_or_default();
    };

    let mut out = Vec::<SocketAddr>::new();
    let push = |out: &mut Vec<SocketAddr>, h: &str, p: u16| {
        if let Ok(sa) = format!("{h}:{p}").parse() {
            if !out.contains(&sa) {
                out.push(sa);
            }
        }
    };

    match host.as_str() {
        "127.0.0.1" => {
            push(&mut out, "127.0.0.1", port);
            push(&mut out, "::1", port);
        }
        "::1" | "[::1]" => {
            push(&mut out, "::1", port);
            push(&mut out, "127.0.0.1", port);
        }
        "localhost" => {
            push(&mut out, "127.0.0.1", port);
            push(&mut out, "::1", port);
        }
        _ => {
            if let Ok(sa) = addr.parse() {
                out.push(sa);
            }
        }
    }

    if out.is_empty() {
        if let Ok(sa) = addr.parse() {
            out.push(sa);
        }
    }
    out
}

fn split_host_port(s: &str) -> Result<(String, u16), ()> {
    // Very small parser sufficient for our use: "host:port" where host is not a URL.
    let idx = s.rfind(':').ok_or(())?;
    let host = s[..idx].to_string();
    let port = s[idx + 1..].parse::<u16>().map_err(|_| ())?;
    Ok((host, port))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = match parse_args() {
        Ok(cfg) => cfg,
        Err(CliExit::Help) => {
            print!("{}", usage());
            return Ok(());
        }
        Err(CliExit::Version) => {
            println!("saelora {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Err(CliExit::Error(msg)) => {
            eprintln!("{msg}\n\n{}", usage());
            std::process::exit(2);
        }
    };
    let interactive = !cfg.headless && is_tty();
    init_logging(&cfg.data_dir, interactive);

    std::fs::create_dir_all(&cfg.data_dir).ok();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let server_cfg = api::ServerConfig {
        addr: cfg.addr.clone(),
        data_dir: cfg.data_dir.clone(),
    };

    // Start the server; if the requested addr can't be bound (e.g. port in use), we'll
    // notice via the join handle and exit instead of silently "running".
    let mut server_task =
        tokio::spawn(async move { api::run_server(server_cfg, shutdown_rx).await });

    if interactive {
        // If the server fails instantly (bind error), surface it instead of opening the TUI.
        tokio::select! {
            res = &mut server_task => {
                match res {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("server task join error: {e}")),
                }?
            }
            _ = tokio::time::sleep(Duration::from_millis(120)) => {}
        }

        // Default experience: show the admin TUI.
        // Ctrl+C quits; Esc navigates back.
        let res = tui::run_tui(cfg.data_dir.clone()).await;

        // Ask the server to shut down gracefully.
        let _ = shutdown_tx.send(());
        // Give it a moment.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = server_task.await;
        return res;
    }

    // Headless mode: keep running until Ctrl+C, or exit if the server fails.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            let _ = shutdown_tx.send(());
            match server_task.await {
                Ok(r) => r,
                Err(e) => Err(anyhow::anyhow!("server task join error: {e}")),
            }
        }
        res = &mut server_task => {
            // If the server dies, propagate the error.
            match res {
                Ok(r) => r,
                Err(e) => Err(anyhow::anyhow!("server task join error: {e}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parsing_help_version_and_errors() {
        assert_eq!(parse_args_from(["--help"]).unwrap_err(), CliExit::Help);
        assert_eq!(parse_args_from(["-h"]).unwrap_err(), CliExit::Help);
        assert_eq!(
            parse_args_from(["--version"]).unwrap_err(),
            CliExit::Version
        );

        let cfg = parse_args_from(["--addr", "0.0.0.0:1234", "--headless"]).unwrap();
        assert_eq!(cfg.addr, "0.0.0.0:1234");
        assert!(cfg.headless);

        match parse_args_from(["--addr"]).unwrap_err() {
            CliExit::Error(s) => assert!(s.contains("--addr")),
            other => panic!("expected CliExit::Error, got {other:?}"),
        }

        match parse_args_from(["--nope"]).unwrap_err() {
            CliExit::Error(s) => assert!(s.contains("unknown flag")),
            other => panic!("expected CliExit::Error, got {other:?}"),
        }
    }

    #[test]
    fn listen_addrs_supports_colon_port_shorthand() {
        let addrs = listen_addrs(":8080");
        assert!(!addrs.is_empty());
        assert!(addrs.iter().any(|a| a.to_string() == "0.0.0.0:8080"));
        assert!(addrs.iter().any(|a| a.to_string() == "[::]:8080"));
    }
}
