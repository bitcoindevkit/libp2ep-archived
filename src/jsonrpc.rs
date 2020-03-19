use std::ops::DerefMut;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::time::timeout;

use log::{debug, info, trace};

use serde::Serialize;

use crate::Error;
use crate::Message;

pub trait JsonRpcState: std::fmt::Debug {
    type Response;
    type Error;

    fn setup(&mut self) -> Result<Option<Message>, Self::Error> {
        Ok(None)
    }

    fn message(&mut self, message: Message) -> Result<Option<Message>, Self::Error>;
    fn done(&self) -> Result<Self::Response, ()>;
}

#[derive(Debug)]
pub struct JsonRpc<'a, T>
where
    T: JsonRpcState,
{
    reader: BufReader<ReadHalf<'a>>,
    writer: WriteHalf<'a>,
    timeout: Duration,
    state: T,
}

impl<'a, T> JsonRpc<'a, T>
where
    T: JsonRpcState<Error = Error>,
{
    pub fn new(stream: &'a mut TcpStream, state: T, timeout: Duration) -> JsonRpc<'a, T> {
        let (raw_read, writer) = stream.split();
        let reader = BufReader::new(raw_read);

        JsonRpc {
            reader,
            writer,
            timeout,
            state,
        }
    }

    async fn write<M: Serialize + std::fmt::Debug>(&mut self, message: &M) -> Result<(), Error> {
        debug!("Sending response: {:?}", message);

        let mut raw = serde_json::to_vec(message)?;
        raw.extend_from_slice(b"\n");
        self.writer.write_all(&raw).await?;

        Ok(())
    }

    pub async fn mainloop(&mut self) -> Result<<T as JsonRpcState>::Response, Error> {
        info!("Starting mainloop...");

        // Optional setup message
        if let Some(response) = self.state.setup()? {
            self.write(&response.to_request()?).await?;
        }

        let mut line = String::with_capacity(1024);
        loop {
            line.clear();

            match timeout(self.timeout, self.reader.read_line(&mut line)).await {
                Err(_) => return Err(Error::Timeout),
                Ok(Err(e)) => {
                    let e: Error = e.into();
                    if let Error::Protocol(protocol_err) = &e {
                        debug!("Protocol error: {:?}", protocol_err);

                        self.write(&Message::Error {
                            error: protocol_err.clone(),
                        })
                        .await?;
                    }

                    return Err(e);
                }
                Ok(Ok(0)) => return Err(Error::EOF),
                Ok(Ok(_)) => {}
            }
            trace!("Received line: `{}`", line.trim());

            let message = serde_json::from_str::<Message>(line.trim())?;
            debug!("Received message: {:?}", message);

            // handle errors separately
            if let Message::Error { error } = message {
                return Err(Error::PeerError(error));
            }

            match self.state.message(message) {
                Ok(Some(response)) => self.write(&response.to_request()?).await?,
                Err(Error::Protocol(e)) => {
                    self.write(&e).await?;
                    return Err(e.into());
                }
                _ => {}
            }

            match self.state.done() {
                Ok(txid) => return Ok(txid),
                Err(_) => continue,
            }
        }
    }
}
