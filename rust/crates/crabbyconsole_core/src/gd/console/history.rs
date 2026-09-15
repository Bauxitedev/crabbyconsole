use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use color_eyre::eyre;
use crabbyconsole_misc::{
    gd::async_node::{AsyncGd, TOKIO_RUNTIME},
    profile,
    reflection::OpaqueDebug,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use tokio::task::JoinError;
use tracing::info_span;

use crate::gd::console::{CrabConsole, util::CRABBYCONSOLE_FOLDER};

/// This is needed, since std::time::Duration by default writes a tuple (seconds, microseconds).
/// This means it will fail to serialize, since csv does not support tuples
fn serialize_duration<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(d.as_secs_f64())
}

fn deserialize_duration<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    let secs = f64::deserialize(d)?;
    Ok(Duration::from_secs_f64(secs))
}

fn serialize_duration_opt<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.serialize_some(&d.as_secs_f64()),
        None => s.serialize_none(),
    }
}

fn deserialize_duration_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
    let secs: Option<f64> = Option::deserialize(d)?;
    Ok(secs.map(Duration::from_secs_f64))
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub(super) struct HistoryEntry {
    /// The time at which the command completed
    #[serde(with = "time::serde::rfc3339")]
    pub(super) timestamp: OffsetDateTime, // rfc3339 is important, otherwise it fails to serialize to csv

    pub(super) command: String,
    pub(super) success: bool,

    #[serde(
        serialize_with = "serialize_duration",
        deserialize_with = "deserialize_duration"
    )]
    pub(super) execution_time: Duration,
    #[serde(
        serialize_with = "serialize_duration_opt",
        deserialize_with = "deserialize_duration_opt"
    )]
    pub(super) async_wait_time: Option<Duration>, // Optional, only used if the result involved waiting for async

    #[serde(
        serialize_with = "serialize_duration_opt",
        deserialize_with = "deserialize_duration_opt"
    )]
    pub(super) signal_wait_time: Option<Duration>, // Optional, only used if the result involved waiting for a signal
}

impl CrabConsole {
    pub(super) fn get_default_history_entries() -> Vec<HistoryEntry> {
        [
            "viewport().size_changed",
            "tree().create_timer(1.0).timeout",
            "viewport().set_scaling_3d_scale(0.1)",
            "Engine.max_fps = 30",
            "%ConsoleLineEdit.modulate = Color.ORANGE",
            ":for x 1 4 :for y 1 4 :for z 1 4 var s = MeshInstance3D.new();s.mesh = SphereMesh.new();add_child(s);s.global_position = Vector3({x},{y},{z})",
            ":node reload CrabbyConsole", // Very useful if you messed up the state of the CrabConsole
          //  ":tween prop ConsoleLineEdit modulate Color(0.5,1,0,1) 3.5", // currently does not work due to removal of str_to_var
            "DisplayServer.beep()",
        ]
        .iter()
        .map(|command| HistoryEntry {
            timestamp: OffsetDateTime::now_utc(),
            command: (*command).to_owned(),
            success: true,
            execution_time: Default::default(),
            async_wait_time: None,
            signal_wait_time: None,
        })
        .collect()
    }

    /// Appends a history line to disk. Note - does not block main thread, will send on a channel so it will be written later.
    pub(super) fn append_history_to_disk(&mut self, entry: HistoryEntry) {
        let _ = self.history_chan.0.send(entry);
    }

    /// Gets the global file path of the `history.csv` file.
    ///
    /// Will try to create the `crabbyconsole` folder if it doesn't exist yet.
    /// Fails if it couldn't create the folder (it will only try to create it once).
    fn get_history_file_path() -> Result<String, Arc<io::Error>> {
        let folder: PathBuf = (*CRABBYCONSOLE_FOLDER.with(|folder| folder.clone()))?;
        Ok(folder
            .join("history.csv")
            .to_str() // PathBuf -> &str
            .expect("invalid utf-8 in path")
            .to_owned()) // &str -> String
    }

    #[tracing::instrument(skip_all)]
    pub(super) async fn load_history_task(mut self: AsyncGd<Self>) {
        let history_file_path = Self::get_history_file_path();

        // Now using TOKIO_RUNTIME.spawn instead of TOKIO_RUNTIME.enter here to prevent panics
        let result: impl Future<Output = Result<eyre::Result<_>, JoinError>> =
            TOKIO_RUNTIME.spawn({
                let history_file_path = history_file_path.clone(); // <-- satisfy borrow checker
                async move {
                    let history_file_path = history_file_path?; // Bail out if failed to create history file

                    let file_tokio = tokio::fs::OpenOptions::new()
                        .read(true)
                        .open(&history_file_path)
                        .await?;
                    let file_std = file_tokio.try_into_std().map_err(|_| {
                        std::io::Error::other("failed to convert tokio file handle to std")
                    })?;

                    let mut reader = csv::Reader::from_reader(file_std);

                    // Note - you may be able to use tokio::task::block_in_place here instead
                    let entries = tokio::task::spawn_blocking(move || -> eyre::Result<_> {
                        info_span!("load_history_task[spawn_blocking]").in_scope(|| {
                            profile!("load_history_task[spawn_blocking]", {
                                let mut entries = vec![];
                                for result in reader.deserialize() {
                                    let entry: HistoryEntry = result?;
                                    entries.push(entry);
                                }
                                Ok(entries)
                            })
                        })
                    })
                    .await
                    .expect("tokio task panicked")?;

                    Ok(entries)
                }
            });

        match profile!(result.await.expect("tokio task panicked")) {
            Ok(entries) => {
                tracing::info!(
                    "read {} history entries from {history_file_path:?}",
                    entries.len()
                );
                self.bind_mut().history = OpaqueDebug(entries);
            }
            Err(err) => {
                tracing::warn!(?err, "failed to read history - using default");

                // use default history entries
                let default_history = Self::get_default_history_entries();
                self.bind_mut().history = OpaqueDebug(default_history.clone());

                // now write them back to disk
                for entry in default_history {
                    self.bind_mut().append_history_to_disk(entry);
                }
            }
        }
    }

    #[tracing::instrument(skip_all)]
    pub(super) async fn save_history_task(self: AsyncGd<Self>) {
        let rx = self.bind().history_chan.1.clone();

        let history_file_path = Self::get_history_file_path();

        while let Ok(history) = rx.recv_async().await {
            // Now using TOKIO_RUNTIME.spawn instead of TOKIO_RUNTIME.enter here to prevent panics
            let result: impl Future<Output = Result<eyre::Result<_>, JoinError>> = TOKIO_RUNTIME
                .spawn({
                    let history_file_path = history_file_path.clone(); // <-- satisfy borrow checker
                    async move {
                        let history_file_path = history_file_path?; // Bail out if failed to create history file

                        // Only write csv headers if 1. the history file doesn't exist or 2. it exists, but is empty
                        // Note - this relies on OR short-circuiting, so don't introduce a metadata var:
                        let write_headers = !Path::new(&history_file_path).exists()
                            || tokio::fs::metadata(&history_file_path).await?.len() == 0;

                        let file_tokio = tokio::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&history_file_path)
                            .await?;
                        let file_std = file_tokio.try_into_std().map_err(|_| {
                            std::io::Error::other("failed to convert tokio file handle to std")
                        })?;
                        let mut writer = csv::WriterBuilder::new()
                            .has_headers(write_headers)
                            .from_writer(file_std);

                        // These two calls are blocking so use spawn_blocking
                        // Note - you may be able to use tokio::task::block_in_place here instead
                        tokio::task::spawn_blocking(move || -> eyre::Result<()> {
                            info_span!("save_history_task[spawn_blocking]").in_scope(|| {
                                writer.serialize(history)?;
                                writer.flush()?;
                                Ok(())
                            })
                        })
                        .await
                        .expect("tokio task panicked")?;

                        Ok(())
                    }
                });

            match profile!(result.await.expect("tokio task panicked")) {
                Ok(()) => tracing::debug!("appended an entry to {history_file_path:?}"),
                Err(err) => tracing::error!(?err, "failed to write history"),
            }
        }

        tracing::warn!("save_history_task ended");
    }
}
