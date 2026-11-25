//! Local operator commands over a Unix socket in a private directory.

use std::{
    net::SocketAddr,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use skippy_api::{
    native_runtime::NativeRuntimeOptions,
    serving::managed::{Loader, ManagedModels, ModelStatus, StartupPolicy},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, UnixListener, UnixStream},
    task::JoinSet,
};

const MAX_COMMAND_BYTES: u64 = 64 * 1024;
const MAX_REPLY_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Args)]
pub struct ServerArgs {
    /// New socket in an owner-only directory.
    #[arg(long)]
    management_socket: PathBuf,
    /// Public inference listener address.
    #[arg(long, default_value = "127.0.0.1:9337")]
    bind_addr: SocketAddr,
    /// Eager local models in ID=PATH form. May be repeated.
    #[arg(long, value_name = "ID=PATH")]
    eager_model: Vec<String>,
    /// Serve successfully loaded eager models even if some fail.
    #[arg(long)]
    best_effort: bool,
}

#[derive(Args)]
pub struct ClientArgs {
    #[arg(long)]
    management_socket: PathBuf,
    #[command(subcommand)]
    command: Operation,
}

#[derive(Subcommand, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Operation {
    Status,
    Load {
        id: String,
        path: PathBuf,
    },
    Unload {
        id: String,
        #[arg(long)]
        instance: Option<u64>,
    },
    Drain {
        id: String,
        #[arg(long)]
        instance: Option<u64>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
}

#[derive(Serialize, Deserialize)]
struct Reply {
    models: Vec<ModelStatus>,
    error: Option<String>,
}

pub async fn serve(args: ServerArgs, native_options: NativeRuntimeOptions) -> Result<()> {
    let (control_listener, _socket) = bind_control(&args.management_socket)?;
    let models = args
        .eager_model
        .into_iter()
        .map(|value| {
            let (id, path) = value
                .split_once('=')
                .context("eager model must use ID=PATH")?;
            Ok((
                id.to_owned(),
                local_loader(id.to_owned(), path.into(), native_options.clone()),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let policy = if args.best_effort {
        StartupPolicy::BestEffort
    } else {
        StartupPolicy::FailFast
    };
    let registry = ManagedModels::start(models, policy, None).await?;
    let listener = TcpListener::bind(args.bind_addr)
        .await
        .context("bind public inference listener")?;
    skippy_commands::console::status(&format!("Inference ready at {}", listener.local_addr()?))?;
    let shutdown = crate::shutdown_signal()?;
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let public = skippy_inference_api::model_dispatch::serve_models(
        listener,
        registry.backend(),
        async move {
            let _ = stopped.await;
        },
    );
    tokio::pin!(public);
    let control = control_loop(control_listener, registry, native_options);
    tokio::pin!(control);
    tokio::select! {
        result = &mut public => return Ok(result?),
        result = &mut control => result?,
        _ = shutdown => {},
    }
    let _ = stop.send(());
    public.await?;
    Ok(())
}

pub async fn manage(args: ClientArgs) -> Result<()> {
    let mut stream = UnixStream::connect(&args.management_socket)
        .await
        .with_context(|| format!("connect to {}", args.management_socket.display()))?;
    if let Operation::Load { path, .. } = &args.command {
        ensure!(
            path.is_absolute(),
            "load requires an absolute local model path"
        );
    }
    write_message(&mut stream, &args.command).await?;
    let reply: Reply = read_message(&mut stream, MAX_REPLY_BYTES).await?;
    if let Some(error) = reply.error {
        anyhow::bail!("{error}");
    }
    skippy_commands::console::write_json(&reply.models)
}

async fn control_loop(
    listener: UnixListener,
    registry: ManagedModels,
    native_options: NativeRuntimeOptions,
) -> Result<()> {
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            connection = listener.accept(), if clients.len() < 32 => {
                let (stream, _) = connection?;
                clients.spawn(handle(stream, registry.clone(), native_options.clone()));
            }
            _ = clients.join_next(), if !clients.is_empty() => {},
        }
    }
}

async fn handle(
    mut stream: UnixStream,
    registry: ManagedModels,
    native: NativeRuntimeOptions,
) -> Result<()> {
    let command = tokio::time::timeout(
        Duration::from_secs(5),
        read_message(&mut stream, MAX_COMMAND_BYTES),
    )
    .await??;
    let result = match command {
        Operation::Status => Ok(()),
        Operation::Load { id, path } => registry
            .load(id.clone(), local_loader(id, path, native))
            .await
            .map(|_| ()),
        Operation::Unload { id, instance } => registry.unload(&id, instance).await.map(|_| ()),
        Operation::Drain {
            id,
            instance,
            timeout_secs,
        } => {
            if timeout_secs > 86400 {
                Err(anyhow::anyhow!("drain timeout must be at most one day"))
            } else {
                registry
                    .drain(&id, instance, Duration::from_secs(timeout_secs))
                    .await
                    .map(|_| ())
            }
        }
    };
    let reply = Reply {
        models: registry.status(),
        error: result.err().map(|error| format!("{error:#}")),
    };
    tokio::time::timeout(Duration::from_secs(5), write_message(&mut stream, &reply)).await??;
    Ok(())
}

fn local_loader(id: String, path: PathBuf, _native: NativeRuntimeOptions) -> Loader {
    Box::new(move || {
        ensure!(
            path.is_absolute() && path.is_file(),
            "load requires an absolute local model file"
        );
        #[cfg(feature = "dynamic-native-runtime")]
        skippy_api::native_runtime::load_local_native_runtime(&_native)?;
        let parsed = crate::serve_settings::parse([
            "skippy".into(),
            "serve".into(),
            "--model-path".into(),
            path.into_os_string(),
            "--model-id".into(),
            id.clone().into(),
        ])?;
        let crate::cli::Command::Serve(args) = parsed.command else {
            unreachable!("serve command")
        };
        let (_, request) = crate::conversion::local_openai_options(args.public)?.into_request()?;
        ManagedModels::local_loader(id, request)()
    })
}

async fn read_message<T: serde::de::DeserializeOwned>(
    input: impl AsyncRead + Unpin,
    max_len: u64,
) -> Result<T> {
    let mut line = Vec::new();
    BufReader::new(input.take(max_len))
        .read_until(b'\n', &mut line)
        .await?;
    ensure!(
        line.last() == Some(&b'\n'),
        "incomplete or oversized management message"
    );
    Ok(serde_json::from_slice(&line)?)
}

async fn write_message(output: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    output.write_all(&bytes).await?;
    Ok(())
}

struct SocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}

fn bind_control(path: &Path) -> Result<(UnixListener, SocketGuard)> {
    ensure!(
        path.is_absolute(),
        "management socket path must be absolute"
    );
    let parent = path
        .parent()
        .context("management socket needs a private parent directory")?;
    let metadata = std::fs::symlink_metadata(parent)?;
    ensure!(
        metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0,
        "management socket directory must only be accessible to its owner (chmod 700)"
    );
    let listener = UnixListener::bind(path).with_context(|| format!("bind {}", path.display()))?;
    let metadata = std::fs::symlink_metadata(path)?;
    let guard = SocketGuard {
        path: path.to_owned(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    Ok((listener, guard))
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
