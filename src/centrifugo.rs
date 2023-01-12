use actix::prelude::*;
use clap::Args;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info_span, instrument, Instrument};
use trillium_client::Conn;
use trillium_tokio::TcpConnector;
use url::Url;

use crate::errors::{TracedError, TracedErrorContext};
use crate::health::{HealthPing, HealthResult};
use crate::model::TagsUpdateData;

type HttpClient = trillium_client::Client<TcpConnector>;

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
        let http = HttpClient::new().with_default_pool();
        let auth_header = String::from("apikey ") + config.centrifugo_api_key.as_str();

        Self {
            api_url: config.centrifugo_api_url.to_owned(),
            auth_header,
            http,
        }
    }

    fn conn(&self) -> Conn<TcpConnector> {
        self.http
            .build_conn("POST", self.api_url.to_owned())
            .with_header("Authorization", self.auth_header.to_owned())
    }

    #[instrument(skip_all)]
    pub(crate) async fn publish(
        &self,
        channel: &str,
        data: impl Serialize,
    ) -> Result<(), TracedError> {
        let json = json!({
            "method": "publish",
            "params": {
                "channel": channel,
                "data": data
            }
        });
        debug!(%json);

        let mut conn = self
            .conn()
            .with_json_body(&json)
            .expect("serialization error");

        conn.send().await.context_during("request send")?;

        let status_code = conn.status().expect("missing status code");
        if !status_code.is_success() {
            return Err(TracedError::from_kind(
                "bad status code",
                &status_code.to_string(),
            ));
        }

        let response: PublishResponse = conn
            .response_json()
            .await
            .context_during("deserializing response")?;

        if let PublishResponse::Error { code, message } = response {
            return Err(TracedError::from_centrifugo_error(code, &message));
        }

        Ok(())
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct TagsUpdate {
    pub(crate) namespace: String,
    pub(crate) channel_name: String,
    pub(crate) data: TagsUpdateData,
}

pub(crate) struct CentrifugoActor {
    pub(crate) client: Client,
}

impl Actor for CentrifugoActor {
    type Context = Context<Self>;
}

impl Handler<TagsUpdate> for CentrifugoActor {
    type Result = ResponseFuture<()>;

    fn handle(&mut self, msg: TagsUpdate, _ctx: &mut Self::Context) -> Self::Result {
        let client = self.client.clone();
        async move {
            debug!(msg.namespace, msg.channel_name, ?msg.data);
            let channel = msg.namespace + ":" + msg.channel_name.as_str();

            if let Err(err) = client.publish(&channel, msg.data).await {
                err.trace_error();
            }
        }
        .instrument(info_span!("tags update handler"))
        .boxed()
    }
}

impl Handler<HealthPing> for CentrifugoActor {
    type Result = ResponseFuture<HealthResult>;

    fn handle(&mut self, _msg: HealthPing, ctx: &mut Self::Context) -> Self::Result {
        let client = self.client.clone();
        let state = ctx.state();
        async move {
            if state != ActorState::Running {
                return Err(format!("actor is in `{:?}` state", state));
            }

            if let Err(err) = client.publish("_", ()).await {
                err.trace_error();
                return Err("publish error".into());
            }

            Ok(())
        }
        .instrument(info_span!("Centrifugo health handler"))
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod client {
        use trillium_testing::Method;

        use super::*;

        #[test]
        fn conn() {
            let config = Config {
                centrifugo_api_url: "http://example.com/test".parse().unwrap(),
                centrifugo_api_key: String::from("secretapikey"),
            };
            let client = Client::new(&config);
            let mut conn = client.conn();

            assert_eq!(conn.method(), Method::Post);
            assert_eq!(conn.url().as_str(), "http://example.com/test");
            let auth_header_value = conn
                .request_headers()
                .get_values("Authorization")
                .unwrap()
                .one()
                .unwrap();
            assert_eq!(auth_header_value, "apikey secretapikey");
        }

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
