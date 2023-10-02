use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

type RequestPayload<S, R> = (S, oneshot::Sender<R>);

const SEND_TIMEOUT: Duration = Duration::from_millis(100);
const RECEIVE_TIMEOUT: Duration = Duration::from_millis(500);

pub(crate) struct RoundtripSender<S, R> {
    inner: mpsc::Sender<RequestPayload<S, R>>,
}

impl<S, R> Clone for RoundtripSender<S, R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<S, R> RoundtripSender<S, R> {
    pub(crate) async fn roundtrip(&self, request: S) -> Result<R, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let cancellation_token = CancellationToken::new();
        let _drop_guard = cancellation_token.clone().drop_guard();
        self.inner
            .send_timeout((request, reply_tx), SEND_TIMEOUT)
            .await
            .map_err(|err| format!("request sending: {err}"))?;
        let reply = tokio::time::timeout(RECEIVE_TIMEOUT, reply_rx)
            .await
            .map_err(|err| format!("reply receiving: {err}"))?
            .map_err(|err| format!("reply receiving: {err}"))?;
        Ok(reply)
    }
}

pub(crate) fn roundtrip_channel<S, R>(
    buffer: usize,
) -> (RoundtripSender<S, R>, mpsc::Receiver<RequestPayload<S, R>>) {
    let (inner, rx) = mpsc::channel(buffer);
    let sender = RoundtripSender { inner };
    (sender, rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod roundtrip {
        use super::*;

        #[tokio::test]
        async fn request_receiver_dropped() {
            let (tx, _) = roundtrip_channel::<(), ()>(1);
            let result = tx.roundtrip(()).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn request_send_timeout() {
            let (tx, _rx) = roundtrip_channel::<(), ()>(1);
            let (reply_tx, _) = oneshot::channel::<()>();
            tx.inner.send(((), reply_tx)).await.unwrap();
            let result = tx.roundtrip(()).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn reply_sender_dropped() {
            let (tx, mut rx) = roundtrip_channel::<(), ()>(1);
            tokio::spawn(async move {
                let (_, _) = rx.recv().await.unwrap();
            });
            let result = tx.roundtrip(()).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn reply_timeout() {
            let (tx, mut rx) = roundtrip_channel::<(), ()>(1);
            tokio::spawn(async move {
                let (_, _reply_tx) = rx.recv().await.unwrap();
                tokio::time::sleep(RECEIVE_TIMEOUT * 2).await;
            });
            let result = tx.roundtrip(()).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn success() {
            let (tx, mut rx) = roundtrip_channel::<u8, u8>(1);
            tokio::spawn(async move {
                let (request, reply_tx) = rx.recv().await.unwrap();
                assert_eq!(request, 54);
                reply_tx.send(63).unwrap();
            });
            let response = tx.roundtrip(54).await.unwrap();
            assert_eq!(response, 63);
        }
    }
}
