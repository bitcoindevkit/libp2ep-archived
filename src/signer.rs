use bitcoin::blockdata::opcodes::all::*;
use bitcoin::blockdata::script::Builder;
use bitcoin::{Script, Transaction};

use crate::Error;

pub trait Signer {
    type Error;

    fn sign(&self, transaction: &mut Transaction, inputs: &[usize]) -> Result<(), Self::Error>;

    fn p2wpkh_scriptcode(script: &Script) -> Script {
        assert!(script.is_v0_p2wpkh());

        let pubkey = &script.as_bytes()[2..];
        Builder::new()
            .push_opcode(OP_DUP)
            .push_opcode(OP_HASH160)
            .push_slice(pubkey)
            .push_opcode(OP_EQUALVERIFY)
            .push_opcode(OP_CHECKSIG)
            .into_script()
    }
}
