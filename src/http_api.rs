use actix::Recipient;
use tracing::instrument;
use trillium::{conn_try, Conn, Handler, State, Status};
use trillium_router::Router;

use crate::health::HealthQuery;

#[derive(Clone)]
struct AppState {
    health_service: Recipient<HealthQuery>,
}

pub(crate) fn handler(health_service: Recipient<HealthQuery>) -> impl Handler {
    (
        State::new(AppState { health_service }),
        Router::new().get("/health", health_handler),
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
