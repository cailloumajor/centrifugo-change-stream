use arcstr::ArcStr;
use clap::Args;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, info_span, instrument, Instrument};
use url::Url;

use crate::channel::{roundtrip_channel, RoundtripSender};
use crate::model::UpdateEvent;

#[derive(Args)]
#[group(skip)]
pub(crate) struct Config {
    /// Centrifugo server base URL
    #[arg(env, long, default_value = "http://centrifugo:8000")]
    centrifugo_url: Url,

    /// Centrifugo API key
    #[arg(env, long)]
    centrifugo_api_key: String,
}

pub(crate) type HealthChannel = RoundtripSender<(), bool>;

pub(crate) type TagsUpdateChannel = mpsc::Sender<UpdateEvent>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum PublishResponse {
    Result {},
    Error { code: u16, message: String },
}

#[derive(Clone)]
pub(crate) struct Client {
    base_url: Url,
    api_key: ArcStr,
    http: HttpClient,
}

impl Client {
    pub(crate) fn new(config: &Config) -> Self {
        let base_url = config.centrifugo_url.clone();
        let api_key = ArcStr::from(&config.centrifugo_api_key);
        let http = HttpClient::new();

        Self {
            base_url,
            api_key,
            http,
        }
    }

    #[instrument(name = "centrifugo_publish", skip_all)]
    async fn publish(&self, channel: &str, data: impl Serialize) -> Result<(), ()> {
        let url = self.base_url.join("/api/publish").unwrap();
        let json = json!({
            "channel": channel,
            "data": data,
        });
        debug!(%json);

        let resp = self
            .http
            .post(url)
            .header("X-API-Key", self.api_key.as_str())
            .json(&json)
            .send()
            .await
            .map_err(|err| {
                error!(kind = "request sending", %err);
            })?;

        let status_code = resp.status();
        if !status_code.is_success() {
            error!(kind = "bad status code", %status_code);
            return Err(());
        }

        let response: PublishResponse = resp.json().await.map_err(|err| {
            error!(kind = "response deserialization", %err);
        })?;

        if let PublishResponse::Error { code, message } = response {
            error!(kind = "Centrifugo error", code, message);
            return Err(());
        }

        Ok(())
    }

    pub(crate) fn handle_tags_update(&self, buffer: usize) -> (TagsUpdateChannel, JoinHandle<()>) {
        let (tx, mut rx) = mpsc::channel::<UpdateEvent>(buffer);
        let cloned_self = self.clone();

        let task = tokio::spawn(
            async move {
                info!(status = "started");

                while let Some(update_event) = rx.recv().await {
                    let (channel, data) = update_event.into_centrifugo();
                    let _ = cloned_self.publish(&channel, data).await;
                }

                info!(status = "terminating");
            }
            .instrument(info_span!("centrifugo_tags_update_handler")),
        );

        (tx, task)
    }

    pub(crate) fn handle_health(&self) -> (HealthChannel, JoinHandle<()>) {
        let (tx, mut rx) = roundtrip_channel(1);
        let cloned_self = self.clone();

        let task = tokio::spawn(
            async move {
                info!(status = "started");

                while let Some((_, response_tx)) = rx.recv().await {
                    let outcome = cloned_self.publish("_", ()).await.is_ok();
                    if response_tx.send(outcome).is_err() {
                        error!(kind = "response channel sending");
                    }
                }

                info!(status = "terminating");
            }
            .instrument(info_span!("centrifugo_health_handler")),
        );

        (tx, task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod client {
        use super::*;

        mod publish {
            use mockito::{Mock, Server};

            use super::*;

            fn server_mock(server: &mut Server) -> Mock {
                server
                    .mock("POST", "/api/publish")
                    .match_header("X-API-Key", "somekey")
                    .match_header("Content-Type", "application/json")
                    .match_body(r#"{"channel":"somechannel","data":"somedata"}"#)
            }

            #[tokio::test]
            async fn request_send_failure() {
                let server = Server::new_async().await;
                let config = Config {
                    centrifugo_url: server.url().parse().unwrap(),
                    centrifugo_api_key: "\0".to_string(),
                };
                let client = Client::new(&config);
                let result = client.publish("somechannel", "somedata").await;
                assert!(result.is_err());
            }

            #[tokio::test]
            async fn bad_status_code() {
                let mut server = Server::new_async().await;
                let mock = server_mock(&mut server)
                    .with_status(500)
                    .create_async()
                    .await;
                let config = Config {
                    centrifugo_url: server.url().parse().unwrap(),
                    centrifugo_api_key: "somekey".to_string(),
                };
                let client = Client::new(&config);
                let result = client.publish("somechannel", "somedata").await;
                mock.assert_async().await;
                assert!(result.is_err());
            }

            #[tokio::test]
            async fn unknown_response() {
                let mut server = Server::new_async().await;
                let mock = server_mock(&mut server)
                    .with_body(r#"{"unknown":null}"#)
                    .create_async()
                    .await;
                let config = Config {
                    centrifugo_url: server.url().parse().unwrap(),
                    centrifugo_api_key: "somekey".to_string(),
                };
                let client = Client::new(&config);
                let result = client.publish("somechannel", "somedata").await;
                mock.assert_async().await;
                assert!(result.is_err());
            }

            #[tokio::test]
            async fn centrifugo_error() {
                let mut server = Server::new_async().await;
                let mock = server_mock(&mut server)
                    .with_body(r#"{"error":{"code":42,"message":"a message"}}"#)
                    .create_async()
                    .await;
                let config = Config {
                    centrifugo_url: server.url().parse().unwrap(),
                    centrifugo_api_key: "somekey".to_string(),
                };
                let client = Client::new(&config);
                let result = client.publish("somechannel", "somedata").await;
                mock.assert_async().await;
                assert!(result.is_err());
            }

            #[tokio::test]
            async fn success() {
                let mut server = Server::new_async().await;
                let mock = server_mock(&mut server)
                    .with_body(r#"{"result":{}}"#)
                    .create_async()
                    .await;
                let config = Config {
                    centrifugo_url: server.url().parse().unwrap(),
                    centrifugo_api_key: "somekey".to_string(),
                };
                let client = Client::new(&config);
                let result = client.publish("somechannel", "somedata").await;
                mock.assert_async().await;
                assert!(result.is_ok());
            }
        }
    }
}
