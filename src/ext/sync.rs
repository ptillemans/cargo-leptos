use std::{net::SocketAddr, time::Duration};

use crate::run::watch::Watched;
use anyhow_ext::{bail, Result};
use tokio::{
    net::TcpStream,
    process::Child,
    sync::{broadcast, oneshot, RwLock},
    time::sleep,
};

lazy_static::lazy_static! {
  /// Interrupts current serve or cargo operation. Used for watch
  pub static ref MSG_BUS: broadcast::Sender<Msg> = {
      broadcast::channel(10).0
  };
  pub static ref SHUTDOWN: RwLock<bool> = RwLock::new(false);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// sent by ctrl-c
    ShutDown,
    /// sent when a source file is changed
    SrcChanged,
    /// sent when an asset file changed
    AssetsChanged(Watched),
    /// sent when a style file changed
    StyleChanged,
    /// messages sent to reload server (forwarded to browser)
    Reload(String),
}

pub async fn send_reload() {
    if !*SHUTDOWN.read().await {
        if let Err(e) = MSG_BUS.send(Msg::Reload("reload".to_string())) {
            log::error!("Leptos failed to send reload: {e}");
        }
    }
}

pub async fn wait_for<F>(cond: F)
where
    F: Fn(&Msg) -> bool + Send + 'static,
{
    let mut rx = MSG_BUS.subscribe();
    loop {
        match rx.recv().await {
            Ok(msg) if cond(&msg) => break,
            Err(e) => {
                log::error!("Leptos error recieving {e}");
                break;
            }
            _ => {}
        }
    }
}

pub fn src_or_style_change(msg: &Msg) -> bool {
    match msg {
        Msg::ShutDown | Msg::SrcChanged | Msg::StyleChanged => true,
        _ => false,
    }
}

pub fn shutdown_msg(msg: &Msg) -> bool {
    match msg {
        Msg::ShutDown => true,
        _ => false,
    }
}

pub fn oneshot_when<F>(cond: F, to: &str) -> oneshot::Receiver<()>
where
    F: Fn(&Msg) -> bool + Send + 'static,
{
    let (tx, rx) = oneshot::channel::<()>();

    let mut interrupt = MSG_BUS.subscribe();

    let to = to.to_string();
    tokio::spawn(async move {
        loop {
            match interrupt.recv().await {
                Ok(Msg::ShutDown) => break,
                Ok(msg) if cond(&msg) => {
                    if let Err(e) = tx.send(()) {
                        log::trace!("{to} could not send {msg:?} due to: {e:?}");
                    }
                    return;
                }
                Err(e) => {
                    log::trace!("{to } error recieving from MSG_BUS: {e}");
                    return;
                }
                Ok(_) => {}
            }
        }
    });

    rx
}

pub async fn run_interruptible<F>(stop_on: F, name: &str, mut process: Child) -> Result<()>
where
    F: Fn(&Msg) -> bool + Send + 'static,
{
    let stop_rx = oneshot_when(stop_on, name);
    tokio::select! {
        res = process.wait() => match res {
                Ok(exit) => match exit.success() {
                    true => Ok(()),
                    false => bail!("Process exited with code {exit}")
                },
                Err(e) => bail!("Command failed due to: {e}"),
        },
        _ = stop_rx => {
            process.kill().await.map(|_| true).expect("Could not kill process");
            Ok(())
        }
    }
}

pub async fn wait_for_localhost(port: u16) -> bool {
    let duration = Duration::from_millis(500);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    for _ in 0..20 {
        if let Ok(_) = TcpStream::connect(&addr).await {
            log::trace!("Autoreload server port {port} open");
            return true;
        }
        sleep(duration).await;
    }
    log::warn!("Autoreload timed out waiting for port {port}");
    false
}
