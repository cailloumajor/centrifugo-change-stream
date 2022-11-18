use std::collections::HashMap;

use actix::prelude::*;
use futures_util::FutureExt;
use tracing::{error, info_span, Instrument};

pub(crate) type HealthResult = Result<(), String>;

#[derive(Message)]
#[rtype(result = "HealthResult")]
pub(crate) struct HealthPing;

#[derive(Message)]
#[rtype(result = "HealthResult")]
pub(crate) struct HealthQuery;

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct Subscribe {
    name: &'static str,
    recipient: Recipient<HealthPing>,
}

impl Subscribe {
    pub(crate) fn new(name: &'static str, recipient: Recipient<HealthPing>) -> Self {
        Self { name, recipient }
    }
}

#[derive(Default)]
pub(crate) struct HealthService {
    registered: HashMap<&'static str, Recipient<HealthPing>>,
}

impl Actor for HealthService {
    type Context = Context<Self>;
}

impl Handler<Subscribe> for HealthService {
    type Result = ();

    fn handle(&mut self, msg: Subscribe, _ctx: &mut Self::Context) -> Self::Result {
        self.registered.insert(msg.name, msg.recipient);
    }
}

impl Handler<HealthQuery> for HealthService {
    type Result = ResponseFuture<HealthResult>;

    fn handle(&mut self, _msg: HealthQuery, _ctx: &mut Self::Context) -> Self::Result {
        let named_recipients: Vec<_> = self
            .registered
            .iter()
            .map(|(n, r)| (*n, r.clone()))
            .collect();
        async move {
            for (name, recipient) in named_recipients {
                let pong = recipient.send(HealthPing).await;
                if let Err(err) = pong {
                    error!(kind = "fatal", %err);
                    System::current().stop();
                }
                if let Err(err) = pong.unwrap() {
                    return Err(format!("component `{}` is unhealthy: {}", name, err));
                }
            }

            Ok(())
        }
        .instrument(info_span!("health handler"))
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestActor {
        pong: HealthResult,
        must_stop: bool,
    }

    impl TestActor {
        fn new(pong: HealthResult, must_stop: bool) -> Self {
            Self { pong, must_stop }
        }
    }

    impl Actor for TestActor {
        type Context = Context<Self>;

        fn started(&mut self, ctx: &mut Self::Context) {
            if self.must_stop {
                ctx.stop();
            }
        }
    }

    impl Handler<HealthPing> for TestActor {
        type Result = HealthResult;

        fn handle(&mut self, _msg: HealthPing, _ctx: &mut Self::Context) -> Self::Result {
            self.pong.clone()
        }
    }

    mod handle_health_query {
        use super::*;

        #[actix::test]
        async fn all_healthy() {
            let first_recipient = TestActor::new(Ok(()), false).start().recipient();
            let second_recipient = TestActor::new(Ok(()), false).start().recipient();
            let health_service = HealthService::start_default();
            health_service
                .try_send(Subscribe::new("first", first_recipient))
                .unwrap();
            health_service
                .try_send(Subscribe::new("second", second_recipient))
                .unwrap();

            let health_result = health_service
                .send(HealthQuery)
                .await
                .expect("unexpected mailbox error");

            assert!(health_result.is_ok());
        }

        #[actix::test]
        async fn one_unhealthy() {
            let first_recipient = TestActor::new(Err("I'm ill".into()), false)
                .start()
                .recipient();
            let second_recipient = TestActor::new(Ok(()), false).start().recipient();
            let health_service = HealthService::start_default();
            health_service
                .try_send(Subscribe::new("first", first_recipient))
                .unwrap();
            health_service
                .try_send(Subscribe::new("second", second_recipient))
                .unwrap();

            let health_result = health_service
                .send(HealthQuery)
                .await
                .expect("unexpected mailbox error");

            assert_eq!(
                health_result.unwrap_err(),
                "component `first` is unhealthy: I'm ill"
            );
        }

        #[actix::test]
        async fn mailbox_error() {
            let first_recipient = TestActor::new(Ok(()), false).start().recipient();
            let second_recipient = TestActor::new(Ok(()), true).start().recipient();
            let health_service = HealthService::start_default();
            health_service
                .try_send(Subscribe::new("first", first_recipient))
                .unwrap();
            health_service
                .try_send(Subscribe::new("second", second_recipient))
                .unwrap();

            let health_result = health_service.send(HealthQuery).await;

            assert!(health_result.is_err());
        }
    }
}
