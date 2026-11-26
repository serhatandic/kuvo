//! Message transport shared by local WebSockets and paired link sockets.

use std::io;

use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
};
use tokio_tungstenite::WebSocketStream;
pub use tokio_tungstenite::tungstenite::Message;
pub use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
use tokio_util::sync::CancellationToken;

pub struct Socket {
    pub send: mpsc::Sender<Message>,
    pub receive: mpsc::Receiver<Message>,
}

impl Socket {
    pub fn pair() -> (Self, Self) {
        let (out, incoming) = mpsc::channel(16);
        let (back, returned) = mpsc::channel(16);
        (
            Self {
                send: out,
                receive: returned,
            },
            Self {
                send: back,
                receive: incoming,
            },
        )
    }

    /// Pump application messages and close codes. Each WebSocket answers its own
    /// ping and pong frames. Separate pumps allow traffic in both directions.
    pub async fn bridge(
        mut self,
        websocket: WebSocketStream<impl AsyncRead + AsyncWrite + Unpin>,
        cancel: CancellationToken,
    ) -> io::Result<()> {
        let (mut writer, mut reader) = websocket.split();
        let (flush, mut flushing) = mpsc::channel(1);
        let incoming = async {
            while let Some(message) = reader.next().await {
                let message = message.map_err(io::Error::other)?;
                match message {
                    Message::Ping(_) => {
                        let _ = flush.try_send(());
                    }
                    Message::Text(_) | Message::Binary(_) | Message::Close(_) => {
                        let close = message.is_close();
                        if self.send.send(message).await.is_err() {
                            break;
                        }
                        if close {
                            return Ok(true);
                        }
                    }
                    _ => {}
                }
            }
            Ok(false)
        };
        let outgoing = async {
            loop {
                tokio::select! {
                    Some(()) = flushing.recv() => writer.flush().await.map_err(io::Error::other)?,
                    message = self.receive.recv() => {
                        let Some(message) = message else { break; };
                        let close = message.is_close();
                        writer.send(message).await.map_err(io::Error::other)?;
                        if close {
                            // The reader can still receive the close acknowledgement.
                            std::future::pending::<()>().await;
                        }
                    }
                }
            }
            Ok(false)
        };
        let result: io::Result<bool> = tokio::select! {
            result = incoming => result,
            result = outgoing => result,
            () = cancel.cancelled() => Ok(false),
        };
        let received_close = result?;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            if received_close {
                // Tungstenite has queued our acknowledgement. Keep the peer
                // alive until it has passed the close on to its WebSocket.
                let _ = writer.flush().await;
                while let Some(message) = self.receive.recv().await {
                    if message.is_close() {
                        break;
                    }
                }
            }
            writer.close().await
        })
        .await;
        Ok(())
    }
}
