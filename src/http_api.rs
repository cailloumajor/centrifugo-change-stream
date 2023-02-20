use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, instrument};
use trillium::{Conn, Handler, State, Status};
use trillium_api::{api, ApiConnExt};
use trillium_router::Router;

use crate::model::{CentrifugoClientRequest, CurrentDataResponse, EnsureObject};

#[derive(Clone)]
struct AppState {
    namespace_prefix: Arc<String>,
    centrifugo_request: mpsc::Sender<CentrifugoClientRequest>,
    current_data: mpsc::Sender<(String, oneshot::Sender<CurrentDataResponse>)>,
}

#[derive(Debug, Deserialize)]
struct SubscribeRequest {
    protocol: String,
    encoding: String,
    channel: String,
}

#[repr(u16)]
#[derive(Clone, Copy)]
enum CentrifugoProxyError {
    UnsupportedProtocol = 1000,
    UnsupportedEncoding,
    BadChannelNamespace,
    InternalError,
}

impl CentrifugoProxyError {
    fn message(&self) -> &'static str {
        match self {
            Self::UnsupportedProtocol => "unsupported protocol",
            Self::UnsupportedEncoding => "unsupported encoding",
            Self::BadChannelNamespace => "bad channel namespace",
            Self::InternalError => "internal error",
        }
    }
}

trait CentrifugoProxyErrorConnExt {
    fn with_centrifugo_proxy_error(self, proxy_error: CentrifugoProxyError) -> Self;
}

impl CentrifugoProxyErrorConnExt for Conn {
    fn with_centrifugo_proxy_error(self, proxy_error: CentrifugoProxyError) -> Self {
        let error_object = json!({
            "error": {
                "code": proxy_error as u16,
                "message": proxy_error.message()
            }
        });
        self.with_json(&error_object)
    }
}

pub(crate) fn handler(
    mongodb_namespace: String,
    centrifugo_request: mpsc::Sender<CentrifugoClientRequest>,
    current_data: mpsc::Sender<(String, oneshot::Sender<CurrentDataResponse>)>,
) -> impl Handler {
    let namespace_prefix = Arc::new(mongodb_namespace + ":");
    (
        State::new(AppState {
            namespace_prefix,
            centrifugo_request,
            current_data,
        }),
        Router::new()
            .get("/health", health_handler)
            .post("/centrifugo/subscribe", api(centrifugo_subscribe_handler)),
    )
}

#[instrument(name = "health_api_handler", skip_all)]
async fn health_handler(conn: Conn) -> Conn {
    let (tx, rx) = oneshot::channel();
    let request = CentrifugoClientRequest::Health(tx);

    let request_sender = &conn.state::<AppState>().unwrap().centrifugo_request;
    if let Err(err) = request_sender.try_send(request) {
        error!(kind = "request channel sending", %err);
        return conn.with_status(Status::InternalServerError).halt();
    }

    match rx.await {
        Ok(true) => conn.with_status(Status::NoContent).halt(),
        Ok(false) => conn.with_status(Status::InternalServerError).halt(),
        Err(err) => {
            error!(kind = "outcome channel receiving", %err);
            conn.with_status(Status::InternalServerError).halt()
        }
    }
}

#[instrument(name = "centrifugo_subscribe_api_handler", skip_all)]
async fn centrifugo_subscribe_handler(conn: Conn, req: SubscribeRequest) -> Conn {
    use CentrifugoProxyError::*;

    debug!(?req);

    if req.protocol != "json" {
        return conn.with_centrifugo_proxy_error(UnsupportedProtocol);
    }
    if req.encoding != "json" {
        return conn.with_centrifugo_proxy_error(UnsupportedEncoding);
    }

    let state = conn.state::<AppState>().unwrap();

    let Some(channel_name) = req.channel.strip_prefix(&*state.namespace_prefix) else {
        return conn.with_centrifugo_proxy_error(BadChannelNamespace);
    };

    let (response_tx, response_rx) = oneshot::channel();
    if let Err(err) = state
        .current_data
        .try_send((channel_name.to_owned(), response_tx))
    {
        error!(kind = "request channel sending", %err);
        return conn.with_status(Status::InternalServerError).halt();
    };

    let data = match response_rx.await {
        Ok(Ok(data)) => EnsureObject(data),
        Ok(Err(_)) => return conn.with_centrifugo_proxy_error(InternalError),
        Err(err) => {
            error!(kind = "outcome channel receiving", %err);
            return conn.with_status(Status::InternalServerError).halt();
        }
    };

    let resp_json = json!({
        "result": {
            "data": data
        }
    });

    debug!(%resp_json);

    conn.with_json(&resp_json)
}

#[cfg(test)]
mod tests {
    use trillium_http::{Conn as HttpConn, Synthetic};
    use trillium_testing::prelude::*;

    use super::*;

    async fn handle(
        method: Method,
        path: &str,
        body: impl Into<Synthetic>,
        handler: &impl trillium::Handler,
    ) -> Conn {
        let http_conn = HttpConn::new_synthetic(method, path, body);
        let conn = handler.run(http_conn.into()).await;
        let mut conn = handler.before_send(conn).await;
        conn.inner_mut().finalize_headers();
        conn
    }

    async fn body_string(conn: &mut Conn) -> String {
        let body_bytes = conn
            .take_response_body()
            .expect("missing body")
            .into_bytes()
            .await
            .expect("error consuming body")
            .to_vec();
        String::from_utf8(body_bytes).expect("error converting body bytes")
    }

    mod health_handler {
        use super::*;

        async fn test_handler(
            centrifugo_request: mpsc::Sender<CentrifugoClientRequest>,
        ) -> impl trillium::Handler {
            let namespace_prefix = Arc::new(String::from(""));
            let (current_data, _) = mpsc::channel(1);
            (
                State::new(AppState {
                    namespace_prefix,
                    centrifugo_request,
                    current_data,
                }),
                health_handler,
            )
        }

        #[tokio::test]
        async fn request_sending_error() {
            let (tx, _) = mpsc::channel(1);
            let handler = test_handler(tx).await;

            let resp = handle(Method::Get, "/", (), &handler).await;

            assert_status!(resp, 500);
        }

        #[tokio::test]
        async fn outcome_channel_receiving_error() {
            let (tx, mut rx) = mpsc::channel(1);
            let handler = test_handler(tx).await;
            tokio::spawn(async move {
                let CentrifugoClientRequest::Health(response) = rx.recv().await.unwrap() else {
                    panic!("wrong request");
                };
                drop(response);
            });

            let resp = handle(Method::Get, "/", (), &handler).await;

            assert_status!(resp, 500);
        }

        #[tokio::test]
        async fn health_error() {
            let (tx, mut rx) = mpsc::channel(1);
            let handler = test_handler(tx).await;
            tokio::spawn(async move {
                let CentrifugoClientRequest::Health(response) = rx.recv().await.unwrap() else {
                    panic!("wrong request");
                };
                response.send(false).unwrap();
            });

            let resp = handle(Method::Get, "/", (), &handler).await;

            assert_status!(resp, 500);
        }

        #[tokio::test]
        async fn success() {
            let (tx, mut rx) = mpsc::channel(1);
            let handler = test_handler(tx).await;
            tokio::spawn(async move {
                let CentrifugoClientRequest::Health(response) = rx.recv().await.unwrap() else {
                    panic!("wrong request");
                };
                response.send(true).unwrap();
            });

            let resp = handle(Method::Get, "/", (), &handler).await;

            assert_status!(resp, 204);
        }
    }

    mod centrifugo_subscribe_handler {
        use mongodb::bson::{Bson, DateTime};

        use crate::model::MongoDBData;

        use super::*;

        async fn test_handler(
            current_data: mpsc::Sender<(String, oneshot::Sender<CurrentDataResponse>)>,
        ) -> impl trillium::Handler {
            let namespace_prefix = Arc::new(String::from("ns:"));
            let (centrifugo_request, _) = mpsc::channel(1);
            (
                State::new(AppState {
                    namespace_prefix,
                    centrifugo_request,
                    current_data,
                }),
                api(centrifugo_subscribe_handler),
            )
        }

        #[test]
        fn unsupported_protocol() {
            let resp = post("/")
                .with_request_body(
                    r#"{"protocol":"unsupported","encoding":"json","channel":"ns:chan"}"#,
                )
                .with_request_header("content-type", "application/json")
                .on(&api(centrifugo_subscribe_handler));

            assert_response!(
                resp,
                200,
                r#"{"error":{"code":1000,"message":"unsupported protocol"}}"#,
                "content-type" => "application/json"
            );
        }

        #[test]
        fn unsupported_encoding() {
            let resp = post("/")
                .with_request_body(
                    r#"{"protocol":"json","encoding":"unsupported","channel":"ns:chan"}"#,
                )
                .with_request_header("content-type", "application/json")
                .on(&api(centrifugo_subscribe_handler));

            assert_response!(
                resp,
                200,
                r#"{"error":{"code":1001,"message":"unsupported encoding"}}"#,
                "content-type" => "application/json"
            );
        }

        #[tokio::test]
        async fn bad_channel_namespace() {
            let (tx, _) = mpsc::channel(1);
            let handler = test_handler(tx).await;
            let body = r#"{"protocol":"json","encoding":"json","channel":"chan"}"#;

            let mut resp = handle(Method::Get, "/", body, &handler).await;

            assert_status!(resp, 200);
            assert_eq!(
                body_string(&mut resp).await,
                r#"{"error":{"code":1002,"message":"bad channel namespace"}}"#
            );
            assert_headers!(resp, "content-type" => "application/json");
        }

        #[tokio::test]
        async fn request_sending_error() {
            let (tx, _) = mpsc::channel(1);
            let handler = test_handler(tx).await;
            let body = r#"{"protocol":"json","encoding":"json","channel":"ns:chan"}"#;

            let resp = handle(Method::Get, "/", body, &handler).await;

            assert_status!(resp, 500);
        }

        #[tokio::test]
        async fn outcome_channel_receiving_error() {
            let (tx, mut rx) = mpsc::channel(1);
            let handler = test_handler(tx).await;
            let body = r#"{"protocol":"json","encoding":"json","channel":"ns:chan"}"#;
            tokio::spawn(async move {
                let (_, response) = rx.recv().await.unwrap();
                drop(response);
            });

            let resp = handle(Method::Get, "/", body, &handler).await;

            assert_status!(resp, 500);
        }

        #[tokio::test]
        async fn current_data_error() {
            let (tx, mut rx) = mpsc::channel(1);
            let handler = test_handler(tx).await;
            let body = r#"{"protocol":"json","encoding":"json","channel":"ns:chan"}"#;
            tokio::spawn(async move {
                let (_, response) = rx.recv().await.unwrap();
                response.send(Err(())).unwrap();
            });

            let mut resp = handle(Method::Get, "/", body, &handler).await;

            assert_status!(resp, 200);
            assert_eq!(
                body_string(&mut resp).await,
                r#"{"error":{"code":1003,"message":"internal error"}}"#
            );
            assert_headers!(resp, "content-type" => "application/json");
        }

        #[tokio::test]
        async fn success_no_data() {
            let (tx, mut rx) = mpsc::channel(1);
            let handler = test_handler(tx).await;
            let body = r#"{"protocol":"json","encoding":"json","channel":"ns:chan"}"#;
            tokio::spawn(async move {
                let (_, response) = rx.recv().await.unwrap();
                response.send(Ok(None)).unwrap();
            });

            let mut resp = handle(Method::Get, "/", body, &handler).await;

            assert_status!(resp, 200);
            assert_eq!(body_string(&mut resp).await, r#"{"result":{"data":{}}}"#);
            assert_headers!(resp, "content-type" => "application/json");
        }

        #[tokio::test]
        async fn success_with_data() {
            let mut tags_update_data = MongoDBData::with_capacity(2);
            tags_update_data.insert_value("first".into(), Bson::Int32(9));
            tags_update_data.insert_value("second".into(), Bson::String("other".into()));
            tags_update_data
                .insert_timestamp("one".to_string(), DateTime::from_millis(1673598600000));
            tags_update_data
                .insert_timestamp("two".to_string(), DateTime::from_millis(471411000000));
            let (tx, mut rx) = mpsc::channel(1);
            let handler = test_handler(tx).await;
            let body = r#"{"protocol":"json","encoding":"json","channel":"ns:chan"}"#;
            tokio::spawn(async move {
                let (_, response) = rx.recv().await.unwrap();
                response.send(Ok(Some(tags_update_data))).unwrap();
            });

            let mut resp = handle(Method::Get, "/", body, &handler).await;

            assert_status!(resp, 200);
            let body = body_string(&mut resp).await;
            assert!(body.starts_with(r#"{"result":{"data":{"#));
            assert!(body.contains(r#""first":9"#));
            assert!(body.contains(r#""second":"other""#));
            assert!(body.contains(r#""ts":{"#));
            assert!(body.contains(r#""one":"2023-01-13T08:30:00Z""#));
            assert!(body.contains(r#""two":"1984-12-09T03:30:00Z""#));
            assert_headers!(resp, "content-type" => "application/json");
        }
    }
}
