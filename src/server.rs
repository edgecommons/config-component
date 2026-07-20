//! Runtime server for the dedicated ConfigComponent.

use std::sync::Arc;

use edgecommons::messaging::{message_handler, Message, MessageBuilder};
use edgecommons::prelude::*;
use serde_json::Value;

use crate::bootstrap::{message_body, optional_bool};
use crate::catalog::is_error_body;
use crate::coordinator::{CatalogCoordinator, PushBundle};
use crate::source::{source_from_descriptor, CatalogSource};
use crate::tokens::sanitize_token;

pub const COMPONENT_NAME: &str = "com.mbreissi.edgecommons.ConfigComponent";
pub const DEFAULT_COMPONENT_TOKEN: &str = "edgecommons-config-component";
pub const GET_TOPIC_TEMPLATE: &str = "ecv1/{device}/config/cmd/get-configuration";
pub const UPDATE_TOPIC_TEMPLATE: &str = "ecv1/{device}/config/cmd/update-catalog";

const SUBSCRIPTION_QUEUE_SIZE: usize = 16;
const SERIAL_CONCURRENCY: usize = 1;

/// Subscribes to the CONFIG_COMPONENT rendezvous and serves catalog bundles.
pub struct ConfigComponentServer {
    messaging: Arc<dyn MessagingService>,
    config: Arc<Config>,
    coordinator: Arc<CatalogCoordinator>,
    get_topic: String,
    update_topic: String,
}

impl ConfigComponentServer {
    pub fn from_runtime(runtime: &EdgeCommons) -> anyhow::Result<Self> {
        let config = runtime.config();
        let messaging = runtime.messaging()?;
        let component_config = config
            .global()
            .get("configComponent")
            .ok_or_else(|| anyhow::anyhow!("component.global.configComponent is required"))?;
        let source =
            source_from_descriptor(component_config.get("catalogSource").ok_or_else(|| {
                anyhow::anyhow!("component.global.configComponent.catalogSource is required")
            })?)?;
        let push_on_catalog_reload = optional_bool(component_config, "pushOnCatalogReload", true)?;
        let allow_volatile_catalog_updates =
            optional_bool(component_config, "allowVolatileCatalogUpdates", false)?;

        let device_token = sanitize_token(&config.thing_name);
        let source: Arc<dyn CatalogSource> = Arc::from(source);
        let coordinator = Arc::new(CatalogCoordinator::with_volatile_updates(
            source,
            device_token.clone(),
            push_on_catalog_reload,
            allow_volatile_catalog_updates,
        ));
        coordinator.load_initial();

        Ok(Self {
            messaging,
            config,
            coordinator,
            get_topic: GET_TOPIC_TEMPLATE.replace("{device}", &device_token),
            update_topic: UPDATE_TOPIC_TEMPLATE.replace("{device}", &device_token),
        })
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        let get_coordinator = self.coordinator.clone();
        let get_messaging = self.messaging.clone();
        let get_config = self.config.clone();
        self.messaging
            .subscribe(
                &self.get_topic,
                message_handler(move |_topic, request| {
                    let coordinator = get_coordinator.clone();
                    let messaging = get_messaging.clone();
                    let config = get_config.clone();
                    async move {
                        let body = message_body(&request);
                        let reply_body = coordinator.bundle_for_request(&body);
                        let name = if is_error_body(&reply_body) {
                            "ConfigurationError"
                        } else {
                            "Configuration"
                        };
                        if let Err(error) =
                            reply_if_requested(&messaging, &config, &request, name, reply_body)
                                .await
                        {
                            tracing::warn!(error = %error, "failed to reply to get-configuration request");
                        }
                    }
                }),
                SUBSCRIPTION_QUEUE_SIZE,
                SERIAL_CONCURRENCY,
            )
            .await?;

        let update_coordinator = self.coordinator.clone();
        let update_messaging = self.messaging.clone();
        let update_config = self.config.clone();
        self.messaging
            .subscribe(
                &self.update_topic,
                message_handler(move |_topic, request| {
                    let coordinator = update_coordinator.clone();
                    let messaging = update_messaging.clone();
                    let config = update_config.clone();
                    async move {
                        let body = message_body(&request);
                        let result = coordinator.update_from_message(&body);
                        if let Err(error) = reply_if_requested(
                            &messaging,
                            &config,
                            &request,
                            "CatalogUpdateAck",
                            result.ack,
                        )
                        .await
                        {
                            tracing::warn!(error = %error, "failed to reply to update-catalog request");
                        }
                        publish_pushes(&messaging, &config, result.pushes).await;
                    }
                }),
                SUBSCRIPTION_QUEUE_SIZE,
                SERIAL_CONCURRENCY,
            )
            .await?;

        self.start_source_watch();
        tracing::info!(
            get_topic = %self.get_topic,
            update_topic = %self.update_topic,
            "ConfigComponent subscribed"
        );
        Ok(())
    }

    pub async fn stop(&self) -> anyhow::Result<()> {
        let mut result = Ok(());
        if let Err(error) = self.messaging.unsubscribe(&self.get_topic).await {
            result = Err(error.into());
        }
        if let Err(error) = self.messaging.unsubscribe(&self.update_topic).await {
            result = Err(error.into());
        }
        result
    }

    fn start_source_watch(&self) {
        let Some(mut rx) = self.coordinator.watch_source() else {
            return;
        };
        let coordinator = self.coordinator.clone();
        let messaging = self.messaging.clone();
        let config = self.config.clone();
        tokio::spawn(async move {
            while let Some(snapshot) = rx.recv().await {
                let pushes = coordinator.reload_from_source_snapshot(snapshot);
                publish_pushes(&messaging, &config, pushes).await;
            }
        });
    }
}

async fn reply_if_requested(
    messaging: &Arc<dyn MessagingService>,
    config: &Config,
    request: &Message,
    name: &str,
    body: Value,
) -> edgecommons::Result<()> {
    if request.header.reply_to.is_none() {
        return Ok(());
    }
    let reply = MessageBuilder::new(name, "1.0")
        .from_config(config)
        .payload(body)
        .build();
    messaging.reply(request, reply).await
}

async fn publish_pushes(
    messaging: &Arc<dyn MessagingService>,
    config: &Config,
    pushes: Vec<PushBundle>,
) {
    for push in pushes {
        let message = MessageBuilder::new("SetConfig", "1.0")
            .from_config(config)
            .payload(push.body)
            .build();
        match messaging.publish(&push.topic, &message).await {
            Ok(()) => tracing::info!(
                topic = %push.topic,
                version = %push.version,
                "pushed catalog bundle"
            ),
            Err(error) => tracing::warn!(
                topic = %push.topic,
                version = %push.version,
                error = %error,
                "failed to push catalog bundle"
            ),
        }
    }
}
