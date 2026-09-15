use std::{net::SocketAddr, sync::Arc};

use crabbyconsole_misc::{
    FutureTracyExt as _,
    gd::async_node::{AsyncGd, AsyncNode, TOKIO_RUNTIME},
};
use flume::{Receiver, Sender};
use futures_lite::StreamExt;
use godot::prelude::*;
use indexmap::IndexMap;
use scopeguard::defer;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener as TokioTcpListener, TcpStream as TokioTcpStream},
};
use tokio_util::sync::CancellationToken;

use crate::gd::console::{
    CrabConsole,
    job::{Job, JobEvent, JobExpression, JobResult},
};

#[derive(Debug)]
pub(super) struct ConsoleServer {
    pub(super) connections: IndexMap<String, ConsoleClient>,
    pub(super) ip: String,
    pub(super) port: u16,
}

#[derive(Default, Clone, Debug)]
pub(super) struct ConsoleClient {
    pub(super) commands_run: usize, // How many commands has this client run?
}

pub(super) enum ConsoleClientEvent {
    Connected,
    Disconnected,
    RanCommand,
}

impl CrabConsole {
    /// Waits on a loop on `connection_rx` to handle incoming connection events.
    /// It will update `self.server.connections` to keep track of active clients and their state.
    /// Stops the loop once the sender is dropped.
    #[tracing::instrument(skip_all)]
    pub(super) async fn handle_console_client_events(
        mut self: AsyncGd<Self>,
        connection_rx: Receiver<(SocketAddr, ConsoleClientEvent)>,
    ) {
        while let Ok((addr, connected)) = connection_rx.recv_async().await {
            let key = addr.to_string();

            {
                let server = &mut self.bind_mut().server;
                if let Some(server) = server {
                    let conns = &mut server.connections;
                    match connected {
                        ConsoleClientEvent::Connected => {
                            let old = conns.insert(key.clone(), ConsoleClient::default());
                            if let Some(old) = old {
                                tracing::warn!(
                                    "Client ({:?}, {:?}) was already \
                                        present in connection list - may indicate bug",
                                    key,
                                    old
                                );
                            }
                        }
                        ConsoleClientEvent::Disconnected => {
                            conns.shift_remove(&key);
                        }
                        ConsoleClientEvent::RanCommand => {
                            let client = conns.get_mut(&key);

                            match client {
                                Some(client) => {
                                    client.commands_run += 1;
                                }
                                None => tracing::warn!("Missing client {}", key),
                            }
                        }
                    }
                } else {
                    // This happens since we get disconnection events when the server shuts down
                    tracing::warn!(
                        "Received connection from ({:?}) while server was offline",
                        key
                    );
                }
            } // drop bind
        }

        tracing::warn!("wait_for_connection_events stopped, since connection_tx was dropped");
    }

    /// Wait for remote console to be started and then start it.
    ///
    /// If start failed, it will try again in a loop so we can request another start.
    #[tracing::instrument(skip_all)]
    pub(super) async fn remote_console_task(self: AsyncGd<Self>, job_tx: Sender<Job>) {
        let this = self.gd();
        let mut this3 = this.clone(); // Need a third one for emitting the start_failed
        loop {
            let job_tx = job_tx.clone();

            let (connection_tx, connection_rx) =
                flume::unbounded::<(SocketAddr, ConsoleClientEvent)>();

            let mut this = this.clone();
            let mut this2 = this.clone(); // Need a second one for defer!

            this.bind_mut()
                .bound_task()
                .new(async |this| this.handle_console_client_events(connection_rx).await)
                .spawn();

            // TODO can't wrap this in TOKIO_RUNTIME.spawn...
            // since `this` is !Send...
            let main_task = async move {
                let _guard = TOKIO_RUNTIME.enter(); // <-- TODO this may cause panics later, use tokio spawn instead
                // although it seems to work fine so far, since there is only 1 remote console task active at a time

                tracing::debug!("waiting for remote_console_start_requested...");

                let start_requested = this
                    .bind_mut()
                    .signals()
                    .remote_console_start_requested()
                    .to_fallible_future();
                let (requested_host, requested_port) = start_requested.await?; // Need to split out the await to avoid long-lasting bind

                // Do this on the main thread, so it's easier to get the resulting port and send it back to the console.
                // Seems to be non-blocking since 1. Tokio has its own background thread and 2. the bind() syscall is allegedly instant.
                let listener = TokioTcpListener::bind((requested_host.to_string(), requested_port))
                    .with_tracy_non_continuous_frame("TokioTcpListener::bind")
                    .await?;

                let addr = listener.local_addr()?;
                let ip = addr.ip().to_string();
                let port = addr.port();

                // Initialize connection list, since server is started now
                this.bind_mut().server = Some(ConsoleServer {
                    connections: Default::default(),
                    ip: ip.clone(),
                    port,
                });

                // Un-set the server when we shutdown, handling panics and other stuff gracefully.
                defer! {
                    this2.bind_mut().server.take();
                }

                this.bind_mut()
                    .signals()
                    .remote_console_started()
                    .emit(&ip.clone(), port);
                tracing::info!("Listening on {ip}:{port} - connect with: rlwrap nc {ip} {port}");

                // Token used to stop the server when requested.
                let stoptoken = CancellationToken::new();

                // Cancel the stoptoken when remote_console_stop_requested signal is triggered.
                {
                    let stoptoken = stoptoken.clone();
                    this.bind_mut()
                        .bound_task()
                        .new(async move |mut this| {
                            let stop_requested = this
                                .bind_mut()
                                .signals()
                                .remote_console_stop_requested()
                                .to_fallible_future();
                            let _ = stop_requested.await; // TODO handle err case?

                            stoptoken.cancel();
                        })
                        .spawn();
                }

                // We need to check if the stoptoken was cancelled in two places.
                //1. The listener.accept() loop
                //2. The tokio::spawn task
                //Since 1 and 2 run on different threads, cancelling one does not cancel the other.

                let stoptoken2 = stoptoken.clone();

                stoptoken
                    .run_until_cancelled(async move {
                        loop {
                            // TODO make a method for this accept_connections or something, too much indentation

                            // accept doesn't seem to block the main thread, so this is fine
                            let Ok((stream, peer_addr)) = listener.accept().await else {
                                continue;
                            };

                            // Notify the main thread we got a new connection.
                            let connection_tx = connection_tx.clone();
                            let _ = connection_tx
                                .send_async((peer_addr, ConsoleClientEvent::Connected))
                                .await;

                            // Handle the connection in a background thread.
                            let job_tx = job_tx.clone();
                            let tokio_task = async move {
                                defer! {
                                    // Notify the main thread when the client disconnected.
                                    // Since we use defer!, this is called even when handle_connection panics.
                                    // Warning - non-async send, make sure connection_tx is unbounded or this could deadlock
                                    let _ = connection_tx
                                        .clone()
                                        .send((peer_addr, ConsoleClientEvent::Disconnected));
                                }

                                tracing::debug!("handle_connection start");
                                handle_connection(peer_addr, stream, job_tx, connection_tx.clone())
                                    .await; // This keeps running until the client disconnects
                                tracing::debug!("handle_connection end");
                            }
                            .with_tracy_non_continuous_frame("handle_connection");

                            let stoptoken = stoptoken2.clone();

                            tokio::spawn(async move {
                                let _ = stoptoken.run_until_cancelled(tokio_task).await;
                                tracing::debug!("tokio task done");
                            }); // don't await the tokio::spawn, or only 1 connection can be active at a time
                        }
                    })
                    .await;

                Ok(())
            };

            let result: color_eyre::Result<()> = main_task.await;
            match result {
                Ok(()) => {
                    tracing::debug!("main task done");
                }
                Err(error) => {
                    tracing::warn!(?error, "remote_console_task failed"); // TODO maybe emit remote_console_failed in that case
                    this3
                        .bind_mut()
                        .signals()
                        .remote_console_start_failed()
                        .emit(&error.to_string()); // We have to turn it into a String, so we can pass it to Godot, so unfortunately we lose some error information
                }
            }
        }
    }
}

/// TCP connection handler - finishes when the client disconnects
pub(super) async fn handle_connection(
    peer_addr: SocketAddr,
    stream: TokioTcpStream,
    tx: Sender<Job>,
    connection_tx: Sender<(SocketAddr, ConsoleClientEvent)>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    tracing::debug!("Client connected");
    writer
        .write_all(b"Remote console ready! Type :help to get started.\n")
        .await
        .ok(); // TODO error ignored

    while let Ok(Some(line)) = lines.next_line().await {
        let expression = line.trim().to_string();
        if expression.is_empty() {
            continue; // Skip empty expressions
        }

        let (reply_tx, reply_rx) = flume::unbounded();

        let _ = connection_tx
            .send_async((peer_addr, ConsoleClientEvent::RanCommand))
            .await;

        if tx
            .send_async(Job {
                expression: JobExpression::String(Arc::from(expression)),
                reply_tx,
                use_timeout: true,
            })
            .await
            .is_err()
        {
            writer
                .write_all(
                    b"Error: main thread unavailable - did you delete the CrabConsole node?\n",
                )
                .await
                .ok();
            break;
        }

        while let Some(response) = reply_rx.stream().next().await {
            let response = match response {
                JobEvent::Done(JobResult {
                    result: Ok(result), ..
                }) => format!("{result:?}\n"), // TODO print execution time
                JobEvent::Done(JobResult { result: Err(e), .. }) => format!("Error: {e}\n"),
                JobEvent::WaitingSignal => "Waiting for signal...".to_owned(),
                JobEvent::WaitingAsync => "Waiting for async...".to_owned(),
            };

            //NOTE - can't use godot_print! here because not on main thread. (update: might've been fixed in recent version of gdext)

            // Don't use tracing here! This must be always visible, regardless of RUST_LOG filters
            println!("REMOTE: {line} => {response}");

            writer.write_all(response.as_bytes()).await.ok(); // TODO error ignored here
        }
    }
}
