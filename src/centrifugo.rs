use std::sync::Arc;

use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, info_span, instrument, Instrument};
use trillium_client::Client as HttpClient;
use trillium_tokio::ClientConfig;
use url::Url;

use crate::model::CentrifugoClientRequest;

#[derive(Args)]
#[group(skip)]
pub(crate) struct Config {
    /// Centrifugo server API URL
    #[arg(env, long, default_value = "http://centrifugo:8000/api")]
    centrifugo_api_url: Url,

    /// Centrifugo API key
    #[arg(env, long)]
    centrifugo_api_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum PublishResponse {
    Result {},
    Error { code: u16, message: String },
}

#[derive(Clone)]
pub(crate) struct Client {
    api_url: Url,
    auth_header: String,
    http: HttpClient,
}

impl Client {
    pub(crate) fn new(config: &Config) -> Self {
        let http = HttpClient::new(ClientConfig::default()).with_default_pool();
        let auth_header = format!("apikey {}", config.centrifugo_api_key);

        Self {
            api_url: config.centrifugo_api_url.clone(),
            auth_header,
            http,
        }
    }

    #[instrument(name = "centrifugo_publish", skip_all)]
    async fn publish(&self, channel: &str, data: impl Serialize) -> Result<(), ()> {
        let json = json!({
            "method": "publish",
            "params": {
                "channel": channel,
                "data": data
            }
        });
        debug!(%json);

        let mut conn = match self
            .http
            .post(self.api_url.clone())
            .with_header("Authorization", self.auth_header.clone())
            .with_json_body(&json)
            .expect("serialization error")
            .await
        {
            Ok(conn) => conn,
            Err(err) => {
                error!(kind = "request sending", %err);
                return Err(());
            }
        };

        let status_code = conn.status().expect("missing status code");
        if !status_code.is_success() {
            error!(kind = "bad status code", %status_code);
            return Err(());
        }

        let response: PublishResponse = match conn.response_json().await {
            Ok(response) => response,
            Err(err) => {
                error!(kind = "response deserialization", %err);
                return Err(());
            }
        };

        if let PublishResponse::Error { code, message } = response {
            error!(kind = "Centrifugo error", code, message);
            return Err(());
        }

        Ok(())
    }

    pub(crate) fn handle_requests(
        self: Arc<Self>,
    ) -> (mpsc::Sender<CentrifugoClientRequest>, JoinHandle<()>) {
        let (tx, mut rx) = mpsc::channel(2);

        let task = tokio::spawn(
            async move {
                info!(status = "started");

                while let Some(request) = rx.recv().await {
                    match request {
                        CentrifugoClientRequest::TagsUpdate(event) => {
                            let (channel, data) = event.into_centrifugo();
                            let _ = self.publish(&channel, data).await;
                        }
                        CentrifugoClientRequest::Health(outcome_tx) => {
                            let outcome = self.publish("_", ()).await.is_ok();
                            if outcome_tx.send(outcome).is_err() {
                                error!(kind = "outcome channel sending");
                            }
                        }
                    }
                }

                info!(status = "terminating");
            }
            .instrument(info_span!("centrifugo_handle_requests")),
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
            use trillium_testing::prelude::Conn;

            use super::*;

            #[test]
            fn request_send_failure() {
                async fn handler(_conn: Conn) -> Conn {
                    panic!()
                }

                trillium_testing::with_server(handler, |url| async move {
                    let config = Config {
                        centrifugo_api_url: url,
                        centrifugo_api_key: Default::default(),
                    };
                    let client = Client::new(&config);
                    client
                        .publish("achannel", "somedata")
                        .await
                        .expect_err("unexpected publish success");

                    Ok(())
                })
            }

            #[test]
            fn bad_status_code() {
                async fn handler(conn: Conn) -> Conn {
                    conn.with_status(500)
                }

                trillium_testing::with_server(handler, |url| async move {
                    let config = Config {
                        centrifugo_api_url: url,
                        centrifugo_api_key: Default::default(),
                    };
                    let client = Client::new(&config);
                    client
                        .publish("achannel", "somedata")
                        .await
                        .expect_err("unexpected publish success");

                    Ok(())
                })
            }

            #[test]
            fn unknown_response() {
                async fn handler(conn: Conn) -> Conn {
                    conn.ok(r#"{"unknown":null}"#)
                }

                trillium_testing::with_server(handler, |url| async move {
                    let config = Config {
                        centrifugo_api_url: url,
                        centrifugo_api_key: Default::default(),
                    };
                    let client = Client::new(&config);
                    client
                        .publish("achannel", "somedata")
                        .await
                        .expect_err("unexpected publish success");

                    Ok(())
                })
            }

            #[test]
            fn centrifugo_error() {
                async fn handler(conn: Conn) -> Conn {
                    conn.ok(r#"{"error":{"code":42,"message":"a message"}}"#)
                }

                trillium_testing::with_server(handler, |url| async move {
                    let config = Config {
                        centrifugo_api_url: url,
                        centrifugo_api_key: Default::default(),
                    };
                    let client = Client::new(&config);
                    client
                        .publish("achannel", "somedata")
                        .await
                        .expect_err("unexpected publish success");

                    Ok(())
                })
            }

            #[test]
            fn success() {
                async fn handler(mut conn: Conn) -> Conn {
                    let req_body = conn.request_body_string().await.unwrap();
                    assert_eq!(
                        req_body,
                        r#"{"method":"publish","params":{"channel":"achannel","data":"somedata"}}"#
                    );
                    conn.ok(r#"{"result":{}}"#)
                }

                trillium_testing::with_server(handler, |url| async move {
                    let config = Config {
                        centrifugo_api_url: url,
                        centrifugo_api_key: Default::default(),
                    };
                    let client = Client::new(&config);
                    client
                        .publish("achannel", "somedata")
                        .await
                        .expect("unexpected publish error");

                    Ok(())
                })
            }
        }
    }
}
