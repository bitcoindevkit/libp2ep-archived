use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::io::{Write, BufRead, BufReader};
use std::convert::TryInto;

use log::{debug, info, trace};

use bitcoin::{Transaction, TxOut, Txid, Script, TxIn, OutPoint};
use bitcoin::blockdata::script::Builder;
use bitcoin::blockdata::opcodes::all::OP_RETURN;

use crate::{VERSION, Message, ProtocolError, Error, WitnessWrapper};
use crate::blockchain::Blockchain;
use crate::signer::Signer;

#[derive(Debug, Default)]
struct ClientState {
    server_version: Option<String>,
    server_utxos: Option<Vec<OutPoint>>,
    server_txid: Option<Txid>,
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

    base_tx: Transaction,
    to_remote_pos: usize,
}

impl<B, S> Client<B, S>
where
    B: Blockchain + std::fmt::Debug,
    <B as Blockchain>::Error: Into<Error> + std::fmt::Debug,
    S: Signer + std::fmt::Debug,
    <S as Signer>::Error: Into<Error> + std::fmt::Debug,
{
    pub fn new<A: ToSocketAddrs>(server: A, blockchain: B, signer: S, base_tx: Transaction, to_remote_pos: usize) -> Result<Client<B, S>, Error> {
        let mut connection = TcpStream::connect(server)?;

        // TODO: some checks on `base_tx`
        Ok(Client {
            connection,
            blockchain,
            signer,

            base_tx,
            to_remote_pos,
        })
    }

    pub fn transaction_to_proof(&self) -> Result<Transaction, Error> {
        let mut transaction = self.base_tx.clone();

        transaction.output.clear();
        transaction.output.push(TxOut {
            value: 21_000_000__00_000_000,
            script_pubkey: Builder::new().push_opcode(OP_RETURN).into_script(),
        });

        for input in &mut transaction.input {
            input.script_sig = Script::new();
            input.witness.clear();
        }

        let inputs_to_sign = (0..transaction.input.len()).collect::<Vec<_>>();
        self.signer.sign(&mut transaction, &inputs_to_sign).map_err(|e| e.into())?;

        Ok(transaction)
    }

    // base_tx contains all our inputs + the two outputs
    pub fn start(&mut self) -> Result<Txid, Error> {
        let mut bufreader = BufReader::new(self.connection.try_clone()?);
        let mut raw_line = String::new();
        let mut state = ClientState::default();

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

            // TODO: handle ProtocolError in a different way
            let (new_state, response) = self.apply_message(state, message)?;
            debug!("<== {:?}", response);

            let mut raw = serde_json::to_vec(&response.to_request()?)?;
            raw.extend_from_slice(b"\n");
            self.connection.write_all(&raw)?;
            self.connection.flush()?;

            raw_line.clear();
            state = new_state;
        }

        Ok(Default::default())
    }

    fn apply_message(&self, mut state: ClientState, message: Message) -> Result<(ClientState, Message), Error> {
        let VERSION_STRING: String = VERSION.into();

        match (&state.server_version, message.clone()) {
            (None, Message::Version{version: VERSION_STRING}) => {
                state.server_version = Some(VERSION_STRING.clone());
                let response = Message::Proof{transaction: self.transaction_to_proof()?};
                return Ok((state, response));
            }
            (None, Message::Version{version}) => return Err(ProtocolError::InvalidVersion(version.into()).into()),
            (None, _) => return Err(ProtocolError::Expected("VERSION".into()).into()),
            _ => {},
        }

        match (&state.server_utxos, message.clone()) {
            (None, Message::Utxos{utxos}) => {
                state.server_utxos = Some(utxos.clone());

                let tx = &self.base_tx;
                let original_value = tx.output[self.to_remote_pos].value;
                let receiver_input_position = tx.input.len(); // TODO: shuffle
                let change_script_index = if self.to_remote_pos == 0 { 1 } else { 0 };
                let change_script = tx.output[change_script_index].script_pubkey.clone();

                let mut witnesses = Vec::new();
                for utxo in utxos {
                    if !self.blockchain.is_unspent(&utxo).map_err(|e| e.into())? {
                        trace!("Invalid prev_out (wrong type or spent)");
                        return Err(ProtocolError::InvalidUtxo.into());
                    }

                    let prev_tx = self.blockchain.get_tx(&utxo.txid).map_err(|e| e.into())?;
                    let prev_value = prev_tx.output[utxo.vout as usize].value;

                    let mut new_tx = tx.clone();
                    new_tx.output[self.to_remote_pos].value = prev_value + original_value;

                    // TODO: shuffle
                    new_tx.input.push(TxIn {
                        previous_output: utxo,
                        sequence: 0xFFFFFFFF,
                        ..Default::default()
                    });

                    let inputs_to_sign = (0..new_tx.input.len()).filter(|i| *i != receiver_input_position).collect::<Vec<_>>();
                    self.signer.sign(&mut new_tx, &inputs_to_sign).map_err(|e| e.into())?;

                    let this_utxo_witnesses = inputs_to_sign
                        .into_iter()
                        .map(|index| WitnessWrapper::new(&tx.input[index]))
                        .collect();

                    witnesses.push(this_utxo_witnesses);
                }

                let response = Message::Witnesses{witnesses, change_script, fees: 5000, receiver_input_position: receiver_input_position.try_into().unwrap(), receiver_output_position: self.to_remote_pos};
                return Ok((state, response));
            },
            (None, _) => return Err(ProtocolError::Expected("UTXOS".into()).into()),
            _ => {},
        }

        // TODO: txid

        Err(ProtocolError::UnexpectedMessage.into())
    }

}
