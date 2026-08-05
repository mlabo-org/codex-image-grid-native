use image_grid_core::DEFAULT_NATIVE_BIND;
use image_grid_server::{RuntimeConfig, router_with_shutdown};
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("image-grid-server: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let options = Options::from_env_and_args(env::args_os().skip(1))?;
    if options.help {
        print_help();
        return Ok(());
    }
    validate_loopback_bind(options.bind)?;

    let data_dir = prepare_absolute_directory(options.data_dir, "data directory")?;
    let server_root = canonical_directory(options.server_root, "server root")?;
    let workspace_dir = options
        .workspace_dir
        .map(|path| canonical_directory(path, "workspace directory"))
        .transpose()?;
    let config = RuntimeConfig::new(server_root, data_dir, workspace_dir, options.launch_target);
    config.prepare_directories()?;

    let listener = tokio::net::TcpListener::bind(options.bind).await?;
    let address = listener.local_addr()?;
    println!("listening: http://{address}");

    let (app, shutdown) = router_with_shutdown(config);
    let signal_shutdown = shutdown.clone();
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_signal().await;
            signal_shutdown.shutdown().await;
        })
        .await;
    shutdown.shutdown().await;
    serve_result?;
    Ok(())
}

fn validate_loopback_bind(bind: SocketAddr) -> Result<(), String> {
    if bind.ip().is_loopback() {
        Ok(())
    } else {
        Err(format!(
            "bind address must be loopback-only; received {bind}"
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownSignal {
    Sigint,
    Sigterm,
}

async fn shutdown_signal() -> ShutdownSignal {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                return select_shutdown_signal(
                    async {
                        let _ = tokio::signal::ctrl_c().await;
                    },
                    async {
                        let _ = terminate.recv().await;
                    },
                )
                .await;
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return ShutdownSignal::Sigint;
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        ShutdownSignal::Sigint
    }
}

async fn select_shutdown_signal<C, T>(ctrl_c: C, terminate: T) -> ShutdownSignal
where
    C: std::future::Future<Output = ()>,
    T: std::future::Future<Output = ()>,
{
    tokio::select! {
        _ = ctrl_c => ShutdownSignal::Sigint,
        _ = terminate => ShutdownSignal::Sigterm,
    }
}

struct Options {
    bind: SocketAddr,
    data_dir: PathBuf,
    server_root: PathBuf,
    workspace_dir: Option<PathBuf>,
    launch_target: String,
    help: bool,
}

impl Options {
    fn from_env_and_args(arguments: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let bind = env::var("IMAGE_GRID_NATIVE_BIND")
            .unwrap_or_else(|_| DEFAULT_NATIVE_BIND.to_owned())
            .parse()
            .map_err(|error| format!("invalid IMAGE_GRID_NATIVE_BIND: {error}"))?;
        let data_dir = env::var_os("IMAGE_GRID_NATIVE_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_data_directory);
        let server_root = env::var_os("IMAGE_GRID_NATIVE_SERVER_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(default_server_root);
        let workspace_dir = env::var_os("IMAGE_GRID_NATIVE_WORKSPACE_DIR").map(PathBuf::from);
        let launch_target =
            env::var("IMAGE_GRID_NATIVE_LAUNCH_TARGET").unwrap_or_else(|_| "server".to_owned());

        let mut options = Self {
            bind,
            data_dir,
            server_root,
            workspace_dir,
            launch_target,
            help: false,
        };
        let mut arguments = arguments;
        while let Some(argument) = arguments.next() {
            match argument.to_string_lossy().as_ref() {
                "--bind" => {
                    let value = next_value(&mut arguments, "--bind")?;
                    options.bind = value
                        .to_string_lossy()
                        .parse()
                        .map_err(|error| format!("invalid --bind value: {error}"))?;
                }
                "--data-root" => {
                    options.data_dir = PathBuf::from(next_value(&mut arguments, "--data-root")?);
                }
                "--server-root" => {
                    options.server_root =
                        PathBuf::from(next_value(&mut arguments, "--server-root")?);
                }
                "--workspace-root" => {
                    options.workspace_dir = Some(PathBuf::from(next_value(
                        &mut arguments,
                        "--workspace-root",
                    )?));
                }
                "--launch-target" => {
                    options.launch_target = next_value(&mut arguments, "--launch-target")?
                        .to_string_lossy()
                        .into_owned();
                }
                "--help" | "-h" => options.help = true,
                unknown => return Err(format!("unknown argument: {unknown}")),
            }
        }

        Ok(options)
    }
}

fn next_value(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<OsString, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn default_data_directory() -> PathBuf {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library")
        .join("Application Support")
        .join("codex-image-grid")
}

fn default_server_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("server crate lives under the workspace root")
        .to_path_buf()
}

fn prepare_absolute_directory(path: PathBuf, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label} must be an absolute path"));
    }
    fs::create_dir_all(&path).map_err(|error| format!("could not create {label}: {error}"))?;
    fs::canonicalize(&path).map_err(|error| format!("could not resolve {label}: {error}"))
}

fn canonical_directory(path: PathBuf, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label} must be an absolute path"));
    }
    let resolved =
        fs::canonicalize(&path).map_err(|error| format!("could not resolve {label}: {error}"))?;
    if !resolved.is_dir() {
        return Err(format!("{label} must point to a directory"));
    }
    Ok(resolved)
}

fn print_help() {
    println!(
        "image-grid-server\n\
         \n\
         Options:\n\
           --bind <HOST:PORT>          Loopback listener (default {DEFAULT_NATIVE_BIND})\n\
           --data-root <PATH>          Native runtime data root\n\
           --server-root <PATH>        Runtime package root reported by health\n\
           --workspace-root <PATH>     Workspace path reported by health\n\
           --launch-target <NAME>      Launch identity reported by health\n"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{pending, ready};

    #[tokio::test]
    async fn shutdown_contract_maps_sigint_and_sigterm_to_the_same_runtime_boundary() {
        assert_eq!(
            select_shutdown_signal(ready(()), pending()).await,
            ShutdownSignal::Sigint
        );
        assert_eq!(
            select_shutdown_signal(pending(), ready(())).await,
            ShutdownSignal::Sigterm
        );
    }

    #[test]
    fn public_network_bind_is_rejected() {
        assert!(validate_loopback_bind("127.0.0.1:4322".parse().unwrap()).is_ok());
        assert!(validate_loopback_bind("[::1]:4322".parse().unwrap()).is_ok());
        assert_eq!(
            validate_loopback_bind("0.0.0.0:4322".parse().unwrap()).unwrap_err(),
            "bind address must be loopback-only; received 0.0.0.0:4322"
        );
    }
}
