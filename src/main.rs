//! Entry point for `com.mbreissi.edgecommons.ConfigComponent`.

use config_component::bootstrap::reject_recursive_bootstrap;
use config_component::server::{ConfigComponentServer, COMPONENT_NAME};
use edgecommons::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    reject_recursive_bootstrap(std::env::args_os())?;

    let runtime = EdgeCommonsBuilder::new(COMPONENT_NAME)
        .args(std::env::args_os())
        .build()
        .await?;

    tracing::info!(
        component = runtime.component_name(),
        identity = %runtime.config().identity().path(),
        "ConfigComponent starting"
    );

    let server = ConfigComponentServer::from_runtime(&runtime)?;
    server.start().await?;
    runtime.shutdown_signal().await;
    server.stop().await?;

    tracing::info!("ConfigComponent stopped");
    Ok(())
}
