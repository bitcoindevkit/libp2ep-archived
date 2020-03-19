use std::any::Any;
use std::convert::TryFrom;
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use tokio::net::{TcpStream, ToSocketAddrs};

use log::{debug, info, trace, warn};

use rand::Rng;

use bitcoin::consensus::deserialize;
use bitcoin::{OutPoint, Script, Transaction, TxIn, TxOut, Txid};

use crate::blockchain::Blockchain;
use crate::common::*;
use crate::jsonrpc::*;
use crate::signer::Signer;
use crate::{Error, Message, ProtocolError, WitnessWrapper, VERSION};

#[derive(Debug)]
enum StateVariant {
    WaitingVersion,
    ServerVersion {
        version: String,
    },
    ServerUtxos {
        version: String,
        utxos: Vec<OutPoint>,
        proof: ProofTransaction<Created>,
    },
    ServerTxid {
        version: String,
        txid: Txid,
    },
}

#[derive(Debug)]
struct ClientState<'a, B, S> {
    base_transaction: Transaction,
    receiver_output_index: usize,

    state: StateVariant,

    blockchain: &'a B,
    signer: &'a S,
}

impl<'a, B, S> ClientState<'a, B, S>
where
    B: Blockchain + std::fmt::Debug,
    Error: From<<B as Blockchain>::Error>,
    S: Signer + std::fmt::Debug,
    Error: From<<S as Signer>::Error>,
{
    fn new(
        base_transaction: Transaction,
        receiver_output_index: usize,
        blockchain: &'a B,
        signer: &'a S,
    ) -> ClientState<'a, B, S> {
        ClientState {
            base_transaction,
            receiver_output_index,
            state: StateVariant::WaitingVersion,
            blockchain,
            signer,
        }
    }

    fn transition(&mut self, message: Message) -> Result<Option<Message>, Error> {
        match &self.state {
            StateVariant::WaitingVersion => match message {
                Message::Version { version } if version == VERSION => {
                    self.state = StateVariant::ServerVersion { version };

                    let transaction = ProofTransaction::<Created>::try_from((
                        self.base_transaction.clone(),
                        self.signer,
                    ))?;

                    Ok(Some(Message::Proof {
                        transaction: transaction.into_inner(),
                    }))
                }
                Message::Version { version } => {
                    Err(ProtocolError::InvalidVersion(version.into()).into())
                }
                _ => Err(ProtocolError::Expected("VERSION".into()).into()),
            },
            StateVariant::ServerVersion { version } => match message {
                Message::Utxos { utxos } => {
                    let tx = &self.base_transaction;

                    let change_script_index = if self.receiver_output_index == 0 {
                        1
                    } else {
                        0
                    };
                    let change_script = tx.output[change_script_index].script_pubkey.clone();

                    let proof_transaction = ProofTransaction::<Created>::try_from((
                        self.base_transaction.clone(),
                        self.signer,
                    ))?;
                    let fees = 5000;
                    let receiver_txin = TxIn {
                        sequence: 0xFFFFFFFF,
                        //previous_output: (),
                        ..Default::default()
                    };
                    let receiver_input_index = tx.input.len(); // TODO: shuffle
                    let receiver_txout = tx.output[self.receiver_output_index].clone();
                    let receiver_output_index = self.receiver_output_index;

                    let final_transaction_meta = FinalTransactionMeta {
                        tx: proof_transaction.clone(),
                        fees,
                        sender_script: change_script.clone(),
                        receiver_txin,
                        receiver_input_index,
                        receiver_txout,
                        receiver_output_index,
                    };

                    let mut witnesses = Vec::new();
                    for utxo in &utxos {
                        if !self.blockchain.is_unspent(&utxo)? {
                            trace!("Invalid prev_out (wrong type or spent)");
                            return Err(ProtocolError::InvalidUtxo.into());
                        }

                        let mut final_transaction_meta = final_transaction_meta.clone();
                        final_transaction_meta.receiver_txin.previous_output = utxo.clone();

                        let final_transaction = FinalTransaction::<Unsigned>::try_from((
                            final_transaction_meta,
                            self.blockchain,
                        ))?;
                        let final_transaction = FinalTransaction::<SenderSigned>::try_from((
                            final_transaction,
                            self.signer,
                        ))?;

                        let inputs_to_sign = (0..final_transaction.input.len())
                            .filter(|i| *i != receiver_input_index)
                            .collect::<Vec<_>>();
                        let this_utxo_witnesses = inputs_to_sign
                            .into_iter()
                            .map(|index| {
                                WitnessWrapper::new(&final_transaction.input[index].witness)
                            })
                            .collect();

                        witnesses.push(this_utxo_witnesses);
                    }

                    self.state = StateVariant::ServerUtxos {
                        version: version.to_string(),
                        proof: proof_transaction,
                        utxos: utxos.clone(),
                    };

                    Ok(Some(Message::Witnesses {
                        fees,
                        change_script,
                        receiver_input_position: receiver_input_index,
                        receiver_output_position: receiver_output_index,
                        witnesses,
                    }))
                }
                _ => Err(ProtocolError::Expected("UTXOS".into()).into()),
            },
            StateVariant::ServerUtxos { version, .. } => match message {
                Message::Txid { txid } => {
                    self.state = StateVariant::ServerTxid {
                        version: version.to_string(),
                        txid,
                    };

                    Ok(None)
                }
                _ => Err(ProtocolError::Expected("TXID".into()).into()),
            },
            _ => Err(ProtocolError::UnexpectedMessage.into()),
        }
    }
}

impl<'a, B, S> JsonRpcState for ClientState<'a, B, S>
where
    B: Blockchain + std::fmt::Debug,
    Error: From<<B as Blockchain>::Error>,
    S: Signer + std::fmt::Debug,
    Error: From<<S as Signer>::Error>,
{
    type Response = Txid;
    type Error = Error;

    fn setup(&mut self) -> Result<Option<Message>, Self::Error> {
        Ok(Some(Message::Version {
            version: VERSION.to_string(),
        }))
    }

    fn message(&mut self, message: Message) -> Result<Option<Message>, Self::Error> {
        Ok(self.transition(message)?)
    }

    fn done(&self) -> Result<Self::Response, ()> {
        if let StateVariant::ServerTxid { txid, .. } = self.state {
            Ok(txid)
        } else {
            Err(())
        }
    }
}

pub struct Client<B, S>
where
    B: Blockchain + std::fmt::Debug,
    S: Signer + std::fmt::Debug,
{
    stream: TcpStream,
    blockchain: B,
    signer: S,

    base_transaction: Transaction,
    receiver_output_index: usize,
}

impl<B, S> Client<B, S>
where
    B: Blockchain + std::fmt::Debug,
    Error: From<<B as Blockchain>::Error>,
    S: Signer + std::fmt::Debug,
    Error: From<<S as Signer>::Error>,
{
    pub async fn new<A: ToSocketAddrs>(
        server: A,
        blockchain: B,
        signer: S,
        base_transaction: Transaction,
        receiver_output_index: usize,
    ) -> Result<Client<B, S>, Error> {
        Ok(Client {
            stream: TcpStream::connect(server).await?,
            blockchain,
            signer,

            base_transaction,
            receiver_output_index,
        })
    }

    pub async fn start(&mut self) -> Result<Txid, Error> {
        info!("Client running!");

        let state = ClientState::new(
            self.base_transaction.clone(),
            self.receiver_output_index,
            &self.blockchain,
            &self.signer,
        );
        let mut jsonrpc = JsonRpc::new(&mut self.stream, state, Duration::from_secs(10));
        let txid = jsonrpc.mainloop().await?;

        Ok(txid)
    }
}
