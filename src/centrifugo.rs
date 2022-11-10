use actix::prelude::*;
use clap::Args;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tracing::{debug, debug_span, error, instrument, Instrument};
use trillium_tokio::TcpConnector;
use url::Url;

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
struct ErrorResponse {
    code: u16,
    message: String,
}

#[derive(Clone)]
pub(crate) struct Client {
    api_url: Url,
    api_key: String,
    http: HttpClient,
}

impl Client {
    pub(crate) fn new(config: &Config) -> Self {
        let http = HttpClient::new().with_default_pool();

        Self {
            api_url: config.centrifugo_api_url.to_owned(),
            api_key: config.centrifugo_api_key.to_owned(),
            http,
        }
    }

    #[instrument(skip_all)]
    pub(crate) async fn publish(&self, channel: &str, data: impl Serialize) {
        let json = json!({
            "method": "publish",
            "params": {
                "channel": channel,
                "data": data
            }
        });
        debug!(%json);

        let auth_header = String::from("apikey ") + self.api_key.as_str();
        let mut conn = match self
            .http
            .post(self.api_url.as_str())
            .with_header("Authorization", auth_header)
            .with_json_body(&json)
        {
            Ok(conn) => conn,
            Err(err) => {
                error!(during = "serializing command", %err);
                return;
            }
        };

        if let Err(err) = conn.send().await {
            error!(during="request send", %err);
            return;
        };

        let status_code = conn.status().expect("missing status code");
        if !status_code.is_success() {
            error!(%status_code);
            return;
        }

        let response: Value = match conn.response_json().await {
            Ok(resp) => resp,
            Err(err) => {
                error!(during="deserializing response", %err);
                return;
            }
        };

        if response.get("result").is_none() {
            if let Some(Ok(error)) = response
                .get("error")
                .map(|err_obj| serde_json::from_value::<ErrorResponse>(err_obj.to_owned()))
            {
                error!(error.code, error.message, "error from Centrifugo");
            } else {
                error!(%response, "unknown response from Centrifugo");
            };
        }
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct TagsUpdate {
    pub(crate) namespace: String,
    pub(crate) channel_name: String,
    pub(crate) data: Map<String, Value>,
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
            client.publish(&channel, msg.data).await;
        }
        .instrument(debug_span!("tags update handler"))
        .boxed()
    }
}
