use std::convert::TryFrom;
use std::ops::Deref;

use serde::Serialize;

use bitcoin::blockdata::opcodes::all::*;
use bitcoin::blockdata::script::Builder;
use bitcoin::secp256k1::{All, Message as SecpMessage, Secp256k1, Signature};
use bitcoin::util::bip143::SighashComponents;
use bitcoin::{PublicKey, Script, Transaction, TxOut};

use crate::blockchain::Blockchain;
use crate::signer::Signer;
use crate::Error as CrateError;

const BTC: u64 = 100_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofTransactionError {
    InvalidVersion,
    InvalidLocktime,
    InvalidProofOutput(usize),
    InvalidInputType(usize),
    InvalidInputSignature(usize),
    MissingUTXO(usize),
    InputIsSpent(usize),
}

pub trait ValidationContext {}

pub struct Created {}
impl ValidationContext for Created {}
pub struct Validated {}
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
    CrateError: From<<B as Blockchain>::Error>,
{
    type Error = CrateError;

    fn try_from(data: (Transaction, &B)) -> Result<Self, Self::Error> {
        let (tx, blockchain) = data;

        if tx.version != 2 {
            Err(ProofTransactionError::InvalidVersion.into())
        } else if tx.lock_time != 0 {
            Err(ProofTransactionError::InvalidLocktime.into())
        } else if tx.output.len() != 1
            || tx.output[0].value != 21_000_000 * BTC
            || tx.output[0].script_pubkey != Builder::new().push_opcode(OP_RETURN).into_script()
        {
            Err(ProofTransactionError::InvalidProofOutput(0).into())
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
    CrateError: From<<S as Signer>::Error>,
{
    type Error = CrateError;

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
                script_pubkey: Builder::new().push_opcode(OP_RETURN).into_script(),
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
