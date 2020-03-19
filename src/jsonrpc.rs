use std::convert::{TryFrom, TryInto};

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{ReadHalf, WriteHalf};
use tokio::net::TcpStream;

use tokio::time::timeout;

use log::{debug, info, trace};

use crate::Error;
use crate::Message;

pub trait JsonRpcState: std::fmt::Debug {
    type OutMessage: Into<Message> + TryFrom<Message>;
    type InMessage: Into<Message> + TryFrom<Message>;
    type Error;
    type Response;

    fn setup(&mut self) -> Result<Option<Self::OutMessage>, Self::Error> {
        Ok(None)
    }

    fn message(
        &mut self,
        message: Self::InMessage,
    ) -> Result<Option<Self::OutMessage>, Self::Error>;
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
    <<T as JsonRpcState>::InMessage as std::convert::TryFrom<Message>>::Error: std::fmt::Debug,
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

    async fn write(&mut self, message: &serde_json::Value) -> Result<(), Error> {
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
            let cast: Message = response.into();
            self.write(&cast.as_json("1")?).await?; // TODO: id
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

                        let cast: Message = protocol_err.clone().into();
                        self.write(&cast.as_json("1")?).await?;
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
            if let Message::Error { error, .. } = message {
                return Err(Error::PeerError(error));
            }
            let parsed: <T as JsonRpcState>::InMessage = message.try_into().unwrap(); // TODO: unwrap

            match self.state.message(parsed) {
                Ok(Some(response)) => self.write(&response.into().as_json("1")?).await?,
                Err(Error::Protocol(e)) => {
                    let cast: Message = e.clone().into();
                    self.write(&cast.as_json("1")?).await?;
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
