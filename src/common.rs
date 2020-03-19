use std::convert::TryFrom;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

use bitcoin::blockdata::opcodes::all::*;
use bitcoin::blockdata::script::Builder;
use bitcoin::consensus::deserialize;
use bitcoin::secp256k1::{All, Message as SecpMessage, Secp256k1, Signature};
use bitcoin::util::bip143::SighashComponents;
use bitcoin::{PublicKey, Script, Transaction, TxIn, TxOut};

use crate::blockchain::Blockchain;
use crate::signer::Signer;
use crate::{Error, WitnessWrapper};

const BTC: u64 = 100_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofTransactionError {
    InvalidVersion,
    InvalidLocktime,
    InvalidProofOutput,
    InvalidInputType(usize),
    InvalidInputSignature(usize),
    MissingUTXO(usize),
    InputIsSpent(usize),
}

pub trait ValidationContext {}

#[derive(Debug, Clone)]
pub struct Created;
impl ValidationContext for Created {}
#[derive(Debug, Clone)]
pub struct Validated;
impl ValidationContext for Validated {}

/// "Proof" transaction that has been verified
#[derive(Debug, Clone, Serialize)]
pub struct ProofTransaction<C: ValidationContext>(
    #[serde(serialize_with = "crate::to_hex")] Transaction,
    std::marker::PhantomData<C>,
);

impl<C: ValidationContext> ProofTransaction<C> {
    pub fn into_inner(self) -> Transaction {
        self.0
    }
}

/// Make sure that a transaction is a valid "proof" transaction
impl<B> TryFrom<(Transaction, &B)> for ProofTransaction<Validated>
where
    B: Blockchain,
    Error: From<<B as Blockchain>::Error>,
{
    type Error = Error;

    fn try_from(data: (Transaction, &B)) -> Result<Self, Self::Error> {
        let (tx, blockchain) = data;

        if tx.version != 2 {
            Err(ProofTransactionError::InvalidVersion.into())
        } else if tx.lock_time != 0 {
            Err(ProofTransactionError::InvalidLocktime.into())
        } else if tx.output.len() != 1
            || tx.output[0].value != 21_000_000 * BTC
            || !tx.output[0].script_pubkey.is_empty()
        {
            Err(ProofTransactionError::InvalidProofOutput.into())
        } else {
            let secp: Secp256k1<All> = Secp256k1::gen_new();
            let comp = SighashComponents::new(&tx);

            for (index, input) in tx.input.iter().enumerate() {
                let prev_tx = blockchain.get_tx(&input.previous_output.txid)?;
                let prev_out = prev_tx
                    .output
                    .get(input.previous_output.vout as usize)
                    .ok_or(ProofTransactionError::MissingUTXO(index))?;

                if !prev_out.script_pubkey.is_v0_p2wpkh() {
                    return Err(ProofTransactionError::InvalidInputType(index).into());
                } else if !blockchain.is_unspent(&input.previous_output)? {
                    return Err(ProofTransactionError::InputIsSpent(index).into());
                }

                let pubkey = &prev_out.script_pubkey.as_bytes()[2..];
                let script_code = Builder::new()
                    .push_opcode(OP_DUP)
                    .push_opcode(OP_HASH160)
                    .push_slice(pubkey)
                    .push_opcode(OP_EQUALVERIFY)
                    .push_opcode(OP_CHECKSIG)
                    .into_script();
                let hash = comp.sighash_all(&input, &script_code, prev_out.value);
                let signature = input
                    .witness
                    .get(0)
                    .ok_or(ProofTransactionError::InvalidInputSignature(index))?;
                let pubkey = input
                    .witness
                    .get(1)
                    .ok_or(ProofTransactionError::InvalidInputSignature(index))?;
                let sig_len = signature.len() - 1;

                secp.verify(
                    &SecpMessage::from_slice(&hash).unwrap(),
                    &Signature::from_der(&signature[..sig_len])
                        .map_err(|_| ProofTransactionError::InvalidInputSignature(index))?,
                    &PublicKey::from_slice(&pubkey)
                        .map_err(|_| ProofTransactionError::InvalidInputSignature(index))?
                        .key,
                )
                .map_err(|_| ProofTransactionError::InvalidInputSignature(index))?;
            }

            Ok(ProofTransaction(tx, std::marker::PhantomData))
        }
    }
}

/// Turn a normal transaction into a "proof" transaction
///
/// It will strip all the outputs and add the 21M BTC one
impl<S> TryFrom<(Transaction, &S)> for ProofTransaction<Created>
where
    S: Signer,
    Error: From<<S as Signer>::Error>,
{
    type Error = Error;

    fn try_from(data: (Transaction, &S)) -> Result<Self, Self::Error> {
        let (mut tx, signer) = data;

        if tx.version != 2 {
            Err(ProofTransactionError::InvalidVersion.into())
        } else if tx.lock_time != 0 {
            Err(ProofTransactionError::InvalidLocktime.into())
        } else {
            tx.output.clear();
            tx.output.push(TxOut {
                value: 21_000_000 * BTC,
                script_pubkey: Script::new(),
            });

            for input in &mut tx.input {
                input.script_sig = Script::new();
                input.witness.clear();
            }

            let inputs_to_sign = (0..tx.input.len()).collect::<Vec<_>>();
            signer.sign(&mut tx, &inputs_to_sign)?;

            Ok(ProofTransaction(tx, std::marker::PhantomData))
        }
    }
}

impl<C: ValidationContext> Deref for ProofTransaction<C> {
    type Target = Transaction;

    fn deref(&self) -> &Transaction {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinalTransactionError {
    NegativeSenderAmount,
    InvalidReceiverInputSequence,
    InvalidReceiverInputNonEmptySig,
    InvalidReceiverInputIndex,
    InvalidReceiverOutputIndex,
    InvalidWitness,
}

#[derive(Debug, Clone, Serialize)]
pub struct FinalTransactionMeta<C: ValidationContext> {
    pub tx: ProofTransaction<C>,
    pub fees: u64,
    pub sender_script: Script,
    pub receiver_txin: TxIn,
    pub receiver_input_index: usize,
    pub receiver_txout: TxOut,
    pub receiver_output_index: usize,
}

pub trait SignedContext {}

#[derive(Debug, Clone)]
pub struct Unsigned;
impl SignedContext for Unsigned {}
#[derive(Debug, Clone)]
pub struct SenderSigned;
impl SignedContext for SenderSigned {}
#[derive(Debug, Clone)]
pub struct Signed;
impl SignedContext for Signed {}

#[derive(Debug, Clone, Serialize)]
pub struct FinalTransaction<S: SignedContext> {
    #[serde(serialize_with = "crate::to_hex")]
    transaction: Transaction,
    receiver_input_index: usize,

    phantom: std::marker::PhantomData<S>,
}

impl<S: SignedContext> FinalTransaction<S> {
    pub fn into_inner(self) -> Transaction {
        self.transaction
    }
}

impl<B, C> TryFrom<(FinalTransactionMeta<C>, &B)> for FinalTransaction<Unsigned>
where
    C: ValidationContext,
    B: Blockchain,
    Error: From<<B as Blockchain>::Error>,
{
    type Error = Error;

    fn try_from(data: (FinalTransactionMeta<C>, &B)) -> Result<Self, Self::Error> {
        let (meta, blockchain) = data;
        let FinalTransactionMeta {
            tx,
            fees,
            sender_script,
            receiver_txin,
            receiver_input_index,
            mut receiver_txout,
            receiver_output_index,
        } = meta;
        let mut tx = tx.into_inner();
        tx.output.clear();

        // Sum the value of all the inputs added by the sender
        let mut sender_input_value = 0;
        for input in &tx.input {
            let prev_tx = blockchain.get_tx(&input.previous_output.txid)?;
            sender_input_value += prev_tx.output[input.previous_output.vout as usize].value;
        }
        // Add the change output for the sender. Fees are subtracted from this one
        tx.output.push(TxOut {
            script_pubkey: sender_script,
            value: sender_input_value
                .checked_sub(fees)
                .ok_or(FinalTransactionError::NegativeSenderAmount)?
                .checked_sub(receiver_txout.value)
                .ok_or(FinalTransactionError::NegativeSenderAmount)?,
        });

        // Check and add the receiver's output
        let receiver_prev_tx = blockchain.get_tx(&receiver_txin.previous_output.txid)?;
        let receiver_input_value =
            receiver_prev_tx.output[receiver_txin.previous_output.vout as usize].value;
        receiver_txout.value += receiver_input_value;
        if receiver_output_index > tx.output.len() {
            return Err(FinalTransactionError::InvalidReceiverOutputIndex.into());
        } else {
            tx.output.insert(receiver_output_index, receiver_txout);
        }
        // Check and add the receiver's input
        if receiver_txin.sequence != 0xFFFF_FFFF {
            return Err(FinalTransactionError::InvalidReceiverInputSequence.into());
        } else if !receiver_txin.script_sig.is_empty() || !receiver_txin.witness.is_empty() {
            return Err(FinalTransactionError::InvalidReceiverInputNonEmptySig.into());
        } else if receiver_input_index > tx.input.len() {
            return Err(FinalTransactionError::InvalidReceiverInputIndex.into());
        } else {
            tx.input.insert(receiver_input_index, receiver_txin);
        }

        Ok(FinalTransaction {
            transaction: tx,
            receiver_input_index,
            phantom: std::marker::PhantomData,
        })
    }
}

impl<S> TryFrom<(FinalTransaction<Unsigned>, &S)> for FinalTransaction<SenderSigned>
where
    S: Signer,
    Error: From<<S as Signer>::Error>,
{
    type Error = Error;

    fn try_from(data: (FinalTransaction<Unsigned>, &S)) -> Result<Self, Self::Error> {
        let (final_transaction, signer) = data;
        let FinalTransaction {
            mut transaction,
            receiver_input_index,
            ..
        } = final_transaction;

        for input in &mut transaction.input {
            input.script_sig = Script::new();
            input.witness.clear();
        }

        let inputs_to_sign = (0..transaction.input.len())
            .filter(|index| *index != receiver_input_index)
            .collect::<Vec<_>>();
        signer.sign(&mut transaction, &inputs_to_sign)?;

        Ok(FinalTransaction {
            transaction,
            receiver_input_index,
            phantom: std::marker::PhantomData,
        })
    }
}

impl TryFrom<(FinalTransaction<Unsigned>, &Vec<WitnessWrapper>)>
    for FinalTransaction<SenderSigned>
{
    type Error = Error;

    fn try_from(
        data: (FinalTransaction<Unsigned>, &Vec<WitnessWrapper>),
    ) -> Result<Self, Self::Error> {
        let (final_transaction, witnesses) = data;
        let FinalTransaction {
            mut transaction,
            receiver_input_index,
            ..
        } = final_transaction;

        for ((_, input), witness) in transaction
            .input
            .iter_mut()
            .enumerate()
            .filter(|(index, _)| *index != receiver_input_index)
            .zip(witnesses)
        {
            input.witness =
                deserialize(witness.as_ref()).map_err(|_| FinalTransactionError::InvalidWitness)?;
        }

        Ok(FinalTransaction {
            transaction,
            receiver_input_index,
            phantom: std::marker::PhantomData,
        })
    }
}

impl<S> TryFrom<(FinalTransaction<SenderSigned>, &S)> for FinalTransaction<Signed>
where
    S: Signer,
    Error: From<<S as Signer>::Error>,
{
    type Error = Error;

    fn try_from(data: (FinalTransaction<SenderSigned>, &S)) -> Result<Self, Self::Error> {
        let (final_transaction, signer) = data;
        let FinalTransaction {
            mut transaction,
            receiver_input_index,
            ..
        } = final_transaction;

        signer.sign(&mut transaction, &[receiver_input_index])?;

        Ok(FinalTransaction {
            transaction,
            receiver_input_index,
            phantom: std::marker::PhantomData,
        })
    }
}

impl<S: SignedContext> Deref for FinalTransaction<S> {
    type Target = Transaction;

    fn deref(&self) -> &Transaction {
        &self.transaction
    }
}
