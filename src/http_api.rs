use actix::Recipient;
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, instrument};
use trillium::{conn_try, Conn, Handler, State, Status};
use trillium_api::{api, ApiConnExt};
use trillium_router::Router;

use crate::db::{CurrentDataError, CurrentDataRequest};
use crate::health::HealthQuery;

#[derive(Clone)]
struct AppState {
    current_data_service: Recipient<CurrentDataRequest>,
    health_service: Recipient<HealthQuery>,
}

#[derive(Debug, Deserialize)]
struct SubscribeRequest {
    protocol: String,
    encoding: String,
    channel: String,
}

#[repr(u32)]
#[derive(Clone, Copy)]
enum CentrifugoProxyError {
    UnsupportedProtocol = 1000,
    UnsupportedEncoding,
    BadChannelName,
    BadNamespace,
    InternalError,
}

impl CentrifugoProxyError {
    fn message(&self) -> &'static str {
        match self {
            Self::UnsupportedProtocol => "unsupported protocol",
            Self::UnsupportedEncoding => "unsupported encoding",
            Self::BadChannelName => "bad channel name",
            Self::BadNamespace => "bad namespace",
            Self::InternalError => "internal error",
        }
    }
}

impl From<CurrentDataError> for CentrifugoProxyError {
    fn from(c: CurrentDataError) -> Self {
        use CentrifugoProxyError::*;

        match c {
            CurrentDataError::BadNamespace => BadNamespace,
            CurrentDataError::MongoDB => InternalError,
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
                "code": proxy_error as u32,
                "message": proxy_error.message()
            }
        });
        self.with_json(&error_object)
    }
}

pub(crate) fn handler(
    health_service: Recipient<HealthQuery>,
    current_data_service: Recipient<CurrentDataRequest>,
) -> impl Handler {
    (
        State::new(AppState {
            health_service,
            current_data_service,
        }),
        Router::new()
            .get("/health", health_handler)
            .post("/centrifugo/subscribe", api(centrifugo_subscribe_handler)),
    )
}

#[instrument(name = "health_api_handler", skip_all)]
async fn health_handler(conn: Conn) -> Conn {
    let health_result = conn
        .state::<AppState>()
        .unwrap()
        .health_service
        .send(HealthQuery)
        .await;
    let health_result = conn_try!(health_result, conn);

    match health_result {
        Ok(_) => conn.with_status(Status::NoContent).halt(),
        Err(err) => conn
            .with_status(Status::InternalServerError)
            .with_body(err)
            .halt(),
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

    let channel_parts: Vec<_> = req.channel.splitn(2, ':').collect();
    let [namespace, id] = channel_parts[..] else {
        return conn.with_centrifugo_proxy_error(BadChannelName)
    };

    let dotted_namespace = namespace.replacen('-', ".", 1);

    let msg = CurrentDataRequest::new(dotted_namespace, id.into());
    let current_data_reply = conn
        .state::<AppState>()
        .unwrap()
        .current_data_service
        .send(msg)
        .await;
    let current_data_result = conn_try!(current_data_reply, conn);

    let data = match current_data_result {
        Ok(data) => data.unwrap_or_default(),
        Err(err) => {
            return conn.with_centrifugo_proxy_error(err.into());
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
    use actix::{Actor, ActorContext, Context, Handler};
    use trillium_http::{Conn as HttpConn, Synthetic};
    use trillium_testing::prelude::*;

    use crate::db::CurrentDataResponse;
    use crate::health::HealthResult;

    use super::*;

    struct TestHealthActor {
        outcome: HealthResult,
        must_stop: bool,
    }

    impl Actor for TestHealthActor {
        type Context = Context<Self>;

        fn started(&mut self, ctx: &mut Self::Context) {
            if self.must_stop {
                ctx.stop();
            }
        }
    }

    impl Handler<HealthQuery> for TestHealthActor {
        type Result = HealthResult;

        fn handle(&mut self, _msg: HealthQuery, _ctx: &mut Self::Context) -> Self::Result {
            self.outcome.clone()
        }
    }

    struct TestCurrentDataActor {
        outcome: CurrentDataResponse,
        must_stop: bool,
    }

    impl Actor for TestCurrentDataActor {
        type Context = Context<Self>;

        fn started(&mut self, ctx: &mut Self::Context) {
            if self.must_stop {
                ctx.stop();
            }
        }
    }

    impl Handler<CurrentDataRequest> for TestCurrentDataActor {
        type Result = CurrentDataResponse;

        fn handle(&mut self, _msg: CurrentDataRequest, _ctx: &mut Self::Context) -> Self::Result {
            self.outcome.clone()
        }
    }

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

        async fn test_handler(health_service: Recipient<HealthQuery>) -> impl trillium::Handler {
            let current_data_service = Actor::start(TestCurrentDataActor {
                outcome: Ok(None),
                must_stop: false,
            })
            .recipient();
            (
                State::new(AppState {
                    health_service,
                    current_data_service,
                }),
                health_handler,
            )
        }

        #[actix::test]
        async fn mailbox_error() {
            let addr = Actor::start(TestHealthActor {
                outcome: Ok(()),
                must_stop: true,
            });
            let handler = test_handler(addr.recipient()).await;

            let resp = handle(Method::Get, "/", (), &handler).await;

            assert_status!(resp, 500);
        }

        #[actix::test]
        async fn health_error() {
            let addr = Actor::start(TestHealthActor {
                outcome: Err("health error".into()),
                must_stop: false,
            });
            let handler = test_handler(addr.recipient()).await;

            let mut resp = handle(Method::Get, "/", (), &handler).await;

            assert_status!(resp, 500);
            assert_eq!(body_string(&mut resp).await, "health error");
        }

        #[actix::test]
        async fn success() {
            let addr = Actor::start(TestHealthActor {
                outcome: Ok(()),
                must_stop: false,
            });
            let handler = test_handler(addr.recipient()).await;

            let resp = handle(Method::Get, "/", (), &handler).await;

            assert_status!(resp, 204);
        }
    }

    mod centrifugo_subscribe_handler {
        use super::*;

        async fn test_handler(
            current_data_service: Recipient<CurrentDataRequest>,
        ) -> impl trillium::Handler {
            let health_service = Actor::start(TestHealthActor {
                outcome: Ok(()),
                must_stop: false,
            })
            .recipient();
            (
                State::new(AppState {
                    health_service,
                    current_data_service,
                }),
                api(centrifugo_subscribe_handler),
            )
        }

        #[test]
        fn unsupported_protocol() {
            let resp = post("/")
                .with_request_body(
                    r#"{"protocol":"unsupported","encoding":"json","channel":"db-coll:chan"}"#,
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
                    r#"{"protocol":"json","encoding":"unsupported","channel":"db-coll:chan"}"#,
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

        #[test]
        fn bad_channel_name() {
            let resp = post("/")
                .with_request_body(r#"{"protocol":"json","encoding":"json","channel":"bad"}"#)
                .with_request_header("content-type", "application/json")
                .on(&api(centrifugo_subscribe_handler));

            assert_response!(
                resp,
                200,
                r#"{"error":{"code":1002,"message":"bad channel name"}}"#,
                "content-type" => "application/json"
            );
        }

        #[actix::test]
        async fn mailbox_error() {
            let addr = Actor::start(TestCurrentDataActor {
                outcome: Ok(None),
                must_stop: true,
            });
            let handler = test_handler(addr.recipient()).await;
            let body = r#"{"protocol":"json","encoding":"json","channel":"db-coll:chan"}"#;

            let resp = handle(Method::Get, "/", body, &handler).await;

            assert_status!(resp, 500);
        }

        #[actix::test]
        async fn current_data_error() {
            let addr = Actor::start(TestCurrentDataActor {
                outcome: Err(CurrentDataError::BadNamespace),
                must_stop: false,
            });
            let handler = test_handler(addr.recipient()).await;
            let body = r#"{"protocol":"json","encoding":"json","channel":"db-coll:chan"}"#;

            let mut resp = handle(Method::Get, "/", body, &handler).await;

            assert_status!(resp, 200);
            assert_eq!(
                body_string(&mut resp).await,
                r#"{"error":{"code":1003,"message":"bad namespace"}}"#
            );
            assert_headers!(resp, "content-type" => "application/json");
        }

        #[actix::test]
        async fn success_no_data() {
            let addr = Actor::start(TestCurrentDataActor {
                outcome: Ok(None),
                must_stop: false,
            });
            let handler = test_handler(addr.recipient()).await;
            let body = r#"{"protocol":"json","encoding":"json","channel":"db-coll:chan"}"#;

            let mut resp = handle(Method::Get, "/", body, &handler).await;

            assert_status!(resp, 200);
            assert_eq!(body_string(&mut resp).await, r#"{"result":{"data":{}}}"#);
            assert_headers!(resp, "content-type" => "application/json");
        }

        #[actix::test]
        async fn success_with_data() {
            let data_collection = serde_json::from_str(r#"{"first":9,"second":"other"}"#)
                .expect("deserialization error");
            let addr = Actor::start(TestCurrentDataActor {
                outcome: Ok(Some(data_collection)),
                must_stop: false,
            });
            let handler = test_handler(addr.recipient()).await;
            let body = r#"{"protocol":"json","encoding":"json","channel":"db-coll:chan"}"#;

            let mut resp = handle(Method::Get, "/", body, &handler).await;

            assert_status!(resp, 200);
            assert_eq!(
                body_string(&mut resp).await,
                r#"{"result":{"data":{"first":9,"second":"other"}}}"#
            );
            assert_headers!(resp, "content-type" => "application/json");
        }
    }
}
