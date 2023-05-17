use arcstr::ArcStr;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{routing, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tracing::{debug, error, instrument};

use crate::model::{CurrentDataChannel, EnsureObject, HealthChannel};

type StatusWithText = (StatusCode, &'static str);

const INTERNAL_ERROR: StatusWithText = (StatusCode::INTERNAL_SERVER_ERROR, "internal server error");

#[derive(Clone)]
struct AppState {
    namespace_prefix: ArcStr,
    health_channel: HealthChannel,
    current_data: CurrentDataChannel,
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

impl From<CentrifugoProxyError> for Json<Value> {
    fn from(value: CentrifugoProxyError) -> Self {
        let error_object = json!({
            "error": {
                "code": value as u16,
                "message": value.message()
            }
        });
        Json(error_object)
    }
}

pub(crate) fn app(
    mongodb_namespace: &str,
    health_channel: HealthChannel,
    current_data: CurrentDataChannel,
) -> Router {
    let namespace_prefix = ArcStr::from(mongodb_namespace.to_owned() + ":");
    Router::new()
        .route("/health", routing::get(health_handler))
        .route(
            "/centrifugo/subscribe",
            routing::post(centrifugo_subscribe_handler),
        )
        .with_state(AppState {
            namespace_prefix,
            health_channel,
            current_data,
        })
}

#[instrument(name = "health_api_handler", skip_all)]
async fn health_handler(State(state): State<AppState>) -> Result<StatusCode, StatusWithText> {
    let (tx, rx) = oneshot::channel();
    state.health_channel.try_send(tx).map_err(|err| {
        error!(kind = "request channel sending", %err);
        INTERNAL_ERROR
    })?;
    rx.await
        .map_err(|err| {
            error!(kind = "outcome channel receiving", %err);
            INTERNAL_ERROR
        })?
        .then_some(StatusCode::NO_CONTENT)
        .ok_or(INTERNAL_ERROR)
}

#[instrument(name = "centrifugo_subscribe_api_handler", skip_all)]
async fn centrifugo_subscribe_handler(
    State(state): State<AppState>,
    Json(req): Json<SubscribeRequest>,
) -> Result<Json<Value>, StatusWithText> {
    debug!(?req);

    if req.protocol != "json" {
        return Ok(CentrifugoProxyError::UnsupportedProtocol.into());
    }
    if req.encoding != "json" {
        return Ok(CentrifugoProxyError::UnsupportedEncoding.into());
    }

    let Some(channel_name) = req.channel.strip_prefix(state.namespace_prefix.as_str()) else {
        return Ok(CentrifugoProxyError::BadChannelNamespace.into());
    };

    let (response_tx, response_rx) = oneshot::channel();
    state
        .current_data
        .try_send((channel_name.to_owned(), response_tx))
        .map_err(|err| {
            error!(kind = "request channel sending", %err);
            INTERNAL_ERROR
        })?;
    let Ok(data) = response_rx
        .await
        .map_err(|err| {
            error!(kind = "outcome channel receiving", %err);
            INTERNAL_ERROR
        })?
        .map(EnsureObject) else {
            return Ok(CentrifugoProxyError::InternalError.into())
        };

    let resp_json = json!({
        "result": {
            "data": data
        }
    });
    debug!(%resp_json);

    Ok(Json(resp_json))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tokio::sync::mpsc;
    use tower::ServiceExt;

    use super::*;

    mod health_handler {
        use super::*;

        fn testing_fixture(health_channel: HealthChannel) -> (Router, Request<Body>) {
            let (current_data, _) = mpsc::channel(1);
            let app = app(Default::default(), health_channel, current_data);
            let req = Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap();
            (app, req)
        }

        #[tokio::test]
        async fn request_sending_error() {
            let (tx, _) = mpsc::channel(1);
            let (app, req) = testing_fixture(tx);
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }

        #[tokio::test]
        async fn outcome_channel_receiving_error() {
            let (tx, mut rx) = mpsc::channel(1);
            let (app, req) = testing_fixture(tx);
            tokio::spawn(async move {
                // Consume and drop the response channel
                let _ = rx.recv().await.expect("channel has been closed");
            });
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }

        #[tokio::test]
        async fn health_error() {
            let (tx, mut rx) = mpsc::channel(1);
            let (app, req) = testing_fixture(tx);
            tokio::spawn(async move {
                let response_tx = rx.recv().await.expect("channel has been closed");
                response_tx.send(false).expect("error sending response");
            });
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }

        #[tokio::test]
        async fn success() {
            let (tx, mut rx) = mpsc::channel(1);
            let (app, req) = testing_fixture(tx);
            tokio::spawn(async move {
                let response_tx = rx.recv().await.expect("channel has been closed");
                response_tx.send(true).expect("error sending response");
            });
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::NO_CONTENT);
        }
    }

    mod centrifugo_subscribe_handler {
        use mongodb::bson::{Bson, DateTime};

        use crate::model::MongoDBData;

        use super::*;

        fn testing_app(current_data: CurrentDataChannel) -> Router {
            let (centrifugo_request, _) = mpsc::channel(1);
            app("ns", centrifugo_request, current_data)
        }

        #[tokio::test]
        async fn unsupported_protocol() {
            let (tx, _) = mpsc::channel(1);
            let app = testing_app(tx);
            let req = Request::post("/centrifugo/subscribe")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"protocol":"unsupported","encoding":"json","channel":"ns:chan"}"#,
                ))
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(res.headers()["Content-Type"], "application/json");
            let body = hyper::body::to_bytes(res).await.unwrap();
            assert_eq!(
                body,
                r#"{"error":{"code":1000,"message":"unsupported protocol"}}"#
            );
        }

        #[tokio::test]
        async fn unsupported_encoding() {
            let (tx, _) = mpsc::channel(1);
            let app = testing_app(tx);
            let req = Request::post("/centrifugo/subscribe")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"protocol":"json","encoding":"unsupported","channel":"ns:chan"}"#,
                ))
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(res.headers()["Content-Type"], "application/json");
            let body = hyper::body::to_bytes(res).await.unwrap();
            assert_eq!(
                body,
                r#"{"error":{"code":1001,"message":"unsupported encoding"}}"#
            );
        }

        #[tokio::test]
        async fn bad_channel_namespace() {
            let (tx, _) = mpsc::channel(1);
            let app = testing_app(tx);
            let req = Request::post("/centrifugo/subscribe")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"protocol":"json","encoding":"json","channel":"chan"}"#,
                ))
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(res.headers()["Content-Type"], "application/json");
            let body = hyper::body::to_bytes(res).await.unwrap();
            assert_eq!(
                body,
                r#"{"error":{"code":1002,"message":"bad channel namespace"}}"#
            );
        }

        #[tokio::test]
        async fn request_sending_error() {
            let (tx, _) = mpsc::channel(1);
            let app = testing_app(tx);
            let req = Request::post("/centrifugo/subscribe")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"protocol":"json","encoding":"json","channel":"ns:chan"}"#,
                ))
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }

        #[tokio::test]
        async fn outcome_channel_receiving_error() {
            let (tx, mut rx) = mpsc::channel(1);
            let app = testing_app(tx);
            tokio::spawn(async move {
                // Consume and drop the response channel
                let _ = rx.recv().await.expect("channel has been closed");
            });
            let req = Request::post("/centrifugo/subscribe")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"protocol":"json","encoding":"json","channel":"ns:chan"}"#,
                ))
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }

        #[tokio::test]
        async fn current_data_error() {
            let (tx, mut rx) = mpsc::channel(1);
            let app = testing_app(tx);
            tokio::spawn(async move {
                let (_, response_tx) = rx.recv().await.unwrap();
                response_tx.send(Err(())).unwrap();
            });
            let req = Request::post("/centrifugo/subscribe")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"protocol":"json","encoding":"json","channel":"ns:chan"}"#,
                ))
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(res.headers()["Content-Type"], "application/json");
            let body = hyper::body::to_bytes(res).await.unwrap();
            assert_eq!(
                body,
                r#"{"error":{"code":1003,"message":"internal error"}}"#
            );
        }

        #[tokio::test]
        async fn success_no_data() {
            let (tx, mut rx) = mpsc::channel(1);
            let app = testing_app(tx);
            tokio::spawn(async move {
                let (_, response_tx) = rx.recv().await.unwrap();
                response_tx.send(Ok(None)).unwrap();
            });
            let req = Request::post("/centrifugo/subscribe")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"protocol":"json","encoding":"json","channel":"ns:chan"}"#,
                ))
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(res.headers()["Content-Type"], "application/json");
            let body = hyper::body::to_bytes(res).await.unwrap();
            assert_eq!(body, r#"{"result":{"data":{}}}"#);
        }

        #[tokio::test]
        async fn success_with_data() {
            let (tx, mut rx) = mpsc::channel(1);
            let app = testing_app(tx);
            let mut tags_update_data = MongoDBData::with_capacity(2);
            tags_update_data.insert_value("first".into(), Bson::Int32(9));
            tags_update_data.insert_value("second".into(), Bson::String("other".into()));
            tags_update_data
                .insert_timestamp("one".to_string(), DateTime::from_millis(1673598600000));
            tags_update_data
                .insert_timestamp("two".to_string(), DateTime::from_millis(471411000000));
            tokio::spawn(async move {
                let (_, response_tx) = rx.recv().await.unwrap();
                response_tx.send(Ok(Some(tags_update_data))).unwrap();
            });
            let req = Request::post("/centrifugo/subscribe")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"protocol":"json","encoding":"json","channel":"ns:chan"}"#,
                ))
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(res.headers()["Content-Type"], "application/json");
            let body = hyper::body::to_bytes(res).await.unwrap();
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert!(body.starts_with(r#"{"result":{"data":{"#));
            assert!(body.contains(r#""first":9"#));
            assert!(body.contains(r#""second":"other""#));
            assert!(body.contains(r#""ts":{"#));
            assert!(body.contains(r#""one":"2023-01-13T08:30:00Z""#));
            assert!(body.contains(r#""two":"1984-12-09T03:30:00Z""#));
        }
    }
}
