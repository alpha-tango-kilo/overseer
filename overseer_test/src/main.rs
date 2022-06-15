#![allow(unused_imports, dead_code)]

use service::docker::DockerContainer;
use service::Service;
use std::sync::Arc;
use task::FileEventTask;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

const SERVICE_STR: &str = r"
name: jellyfin
host: localhost
";

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    tracing::subscriber::set_global_default(
        FmtSubscriber::builder()
            .with_max_level(Level::TRACE)
            .with_thread_ids(true)
            .finish(),
    )
    .unwrap();
    color_eyre::install().unwrap();

    let mut jellyfin = serde_yaml::from_str::<DockerContainer>(SERVICE_STR)?;
    jellyfin.connect().await?;
    info!(?jellyfin);
    let jellyfin = Arc::new(jellyfin);
    let status = (&jellyfin).status().await?;
    info!(%status);

    /*info!("Loading from file");
    let file_task =
        FileEventTask::load_from("task/examples/file_task.yml").await?;
    eprintln!("{file_task:#?}");
    let file_task = Arc::new(file_task);
    info!("Activating");
    file_task.activate().await?.await?;
    info!("Job done");*/
    Ok(())
}
