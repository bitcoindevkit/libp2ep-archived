use std::convert::TryFrom;
use std::time::Duration;

use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};

use tokio::time::timeout;

use tokio_socks::tcp::Socks5Stream;
use tokio_socks::IntoTargetAddr;

use log::{debug, info, trace};

use bitcoin::{OutPoint, Transaction, TxIn, Txid};

use libtor::{Tor, TorFlag};

use crate::blockchain::Blockchain;
use crate::common::*;
use crate::jsonrpc::*;
use crate::signer::Signer;
use crate::{Error, ProtocolError, Request, Response, WitnessWrapper, VERSION};

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
        transaction: Transaction,
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

    fn transition(&mut self, message: Response) -> Result<Option<Request>, Error> {
        match &self.state {
            StateVariant::WaitingVersion => match message {
                Response::Version { version } if version == VERSION => {
                    self.state = StateVariant::ServerVersion { version };

                    let transaction = ProofTransaction::<Created>::try_from((
                        self.base_transaction.clone(),
                        self.signer,
                    ))?;

                    Ok(Some(Request::Proof {
                        transaction: transaction.into_inner(),
                    }))
                }
                Response::Version { version } => Err(ProtocolError::InvalidVersion(version).into()),
                _ => Err(ProtocolError::Expected("VERSION".into()).into()),
            },
            StateVariant::ServerVersion { version } => match message {
                Response::Utxos { utxos } => {
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
                        sequence: 0xFFFF_FFFF,
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
                        final_transaction_meta.receiver_txin.previous_output = *utxo;

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
                        utxos,
                    };

                    Ok(Some(Request::Witnesses {
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
                Response::Txid { txid, transaction } => {
                    self.state = StateVariant::ServerTxid {
                        version: version.to_string(),
                        transaction,
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
    type OutMessage = Request;
    type InMessage = Response;
    type Response = (Txid, Transaction);
    type Error = Error;

    fn setup(&mut self) -> Result<Option<Self::OutMessage>, Self::Error> {
        Ok(Some(Request::Version {
            version: VERSION.to_string(),
        }))
    }

    fn message(
        &mut self,
        message: Self::InMessage,
    ) -> Result<Option<Self::OutMessage>, Self::Error> {
        Ok(self.transition(message)?)
    }

    fn done(&self) -> Result<Self::Response, ()> {
        if let StateVariant::ServerTxid {
            txid, transaction, ..
        } = &self.state
        {
            Ok((*txid, transaction.clone()))
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
    stream: Socks5Stream,
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
    pub async fn new<'a, A: IntoTargetAddr<'a> + std::clone::Clone>(
        server: A,
        blockchain: B,
        signer: S,
        base_transaction: Transaction,
        receiver_output_index: usize,
    ) -> Result<Client<B, S>, Error> {
        let rand_string: String = thread_rng().sample_iter(&Alphanumeric).take(30).collect();

        let mut dir = std::env::temp_dir();
        dir.push(rand_string);

        debug!("Using tempdir: {}", dir.display());

        Tor::new()
            .flag(TorFlag::DataDirectory(dir.to_str().unwrap().into()))
            .flag(TorFlag::SocksPort(9051))
            .start_background();

        let mut attempts = 0;
        let stream = loop {
            if attempts > 10 {
                return Err(Error::Timeout);
            }

            debug!("Attempting to connect...");
            attempts += 1;

            match timeout(
                Duration::from_secs(10),
                Socks5Stream::connect("127.0.0.1:9051", server.clone()),
            )
            .await
            {
                Err(_) => continue,
                Ok(Err(_)) => std::thread::sleep(Duration::from_secs(2)),
                Ok(Ok(stream)) => break stream,
            };
        };

        Ok(Client {
            stream,
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
        let (txid, _transaction) = jsonrpc.mainloop().await?;

        Ok(txid)
    }
}
