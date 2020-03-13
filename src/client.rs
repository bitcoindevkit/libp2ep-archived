use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::io::{Write, BufRead, BufReader};

use log::{debug, info, trace};

use bitcoin::{Transaction, TxOut, Txid, Script};
use bitcoin::blockdata::script::Builder;
use bitcoin::blockdata::opcodes::all::OP_RETURN;

use crate::{VERSION, Message, ProtocolError, Error};
use crate::blockchain::Blockchain;
use crate::signer::Signer;

#[derive(Debug, Default)]
struct ClientStateMachine {
    server_version: Option<String>,
    server_utxos: Option<Vec<TxOut>>,
    server_txid: Option<Txid>,

    pub client_change: Option<Script>,
    pub client_proof: Option<Transaction>,
}

impl ClientStateMachine {
    fn new() -> Self {
        Default::default()
    }

    fn apply_message(mut self, message: Message) -> Result<(Self, Message), ProtocolError> {
        let VERSION_STRING: String = VERSION.into();

        match (&self.server_version, message.clone()) {
            (None, Message::Version{version: VERSION_STRING}) => {
                self.server_version = Some(VERSION_STRING.clone());
                let response = Message::Proof{transaction: self.client_proof.as_ref().cloned().ok_or(ProtocolError::MissingData)?};
                return Ok((self, response));
            }
            (None, Message::Version{version}) => return Err(ProtocolError::InvalidVersion(version.into())),
            (None, _) => return Err(ProtocolError::Expected("VERSION".into())),
            _ => {},
        }

        match (&self.server_utxos, message.clone()) {
            (None, Message::Utxos{utxos}) => {
                let mut change = self.client_change.as_ref().cloned().ok_or(ProtocolError::MissingData)?;
                let mut base_tx = self.client_proof.as_ref().cloned().ok_or(ProtocolError::MissingData)?;
                base_tx.output.clear();

                // add the two outputs for the final tx
                base_tx.output.push(TxOut{
                    script_pubkey = change,
                    value: 5_000_000,
                });
                base_tx.output.push(TxOut{
                    script_pubkey = Script::new(),
                    value: 0,
                });
                let receiver_pos = 1;

                for utxo in utxos {
                    // TODO
                    // check if utxo is unspent
                    // get its value
                    // update output[receiver_pos] accordingly
                    // sign
                }
            },
            (None, _) => return Err(ProtocolError::Expected("UTXOS".into())),
            _ => {},
        }

        unreachable!();
    }
}

pub struct Client<B, S>
where
    B: Blockchain + std::fmt::Debug,
    S: Signer + std::fmt::Debug,
    <S as Signer>::Error: Into<Error> + std::fmt::Debug,
{
    connection: TcpStream,
    blockchain: B,
    signer: S,
}

impl<B, S> Client<B, S>
where
    B: Blockchain + std::fmt::Debug,
    S: Signer + std::fmt::Debug,
    <S as Signer>::Error: Into<Error> + std::fmt::Debug,
{
    pub fn new<A: ToSocketAddrs>(server: A, blockchain: B, signer: S) -> Result<Client<B, S>, Error> {
        let mut connection = TcpStream::connect(server)?;

        Ok(Client {
            connection,
            blockchain,
            signer,
        })
    }

    pub fn transaction_to_proof(&self, transaction: &Transaction) -> Result<Transaction, Error> {
        let mut transaction = transaction.clone();

        transaction.version = 2;
        transaction.lock_time = 0;

        transaction.output.clear();
        transaction.output.push(TxOut {
            value: 21_000_000__00_000_000,
            script_pubkey: Builder::new().push_opcode(OP_RETURN).push_slice(&[0; 32]).into_script(),
        });

        for input in &mut transaction.input {
            input.script_sig = Script::new();
            input.witness.clear();
        }

        self.signer.sign(&mut transaction).map_err(|e| e.into())?;

        Ok(transaction)
    }

    pub fn start(&mut self, transaction: &Transaction, change: &Script) -> Result<Txid, Error> {
        let mut bufreader = BufReader::new(self.connection.try_clone()?);
        let mut raw_line = String::new();
        let mut state_machine = ClientStateMachine::new();

        state_machine.client_change = Some(change.clone());
        state_machine.client_proof = Some(self.transaction_to_proof(transaction)?);

        let version_msg = Message::Version{version: VERSION.into()};
        let mut raw = serde_json::to_vec(&version_msg.to_request()?)?;
        raw.extend_from_slice(b"\n");
        self.connection.write_all(&raw)?;
        self.connection.flush()?;

        while let Ok(size) = bufreader.read_line(&mut raw_line) {
            if size == 0 {
                break;
            }
            let line = raw_line.trim_end_matches(char::is_whitespace);
            if line.is_empty() {
                continue;
            }

            trace!("==> {:?}", line);

            let message = serde_json::from_str::<Message>(line)?;
            debug!("==> {:?}", message);

            let (new_sm, response) = state_machine.apply_message(message)?;
            debug!("<== {:?}", response);

            let mut raw = serde_json::to_vec(&response.to_request()?)?;
            raw.extend_from_slice(b"\n");
            self.connection.write_all(&raw)?;
            self.connection.flush()?;

            raw_line.clear();
            state_machine = new_sm;
        }

        Ok(Default::default())
    }
}
