use bitcoin::{Txid, Transaction};

use crate::Error;

pub trait Signer {
    type Error: Into<Error> + std::fmt::Debug;

    fn sign(&self, transaction: &mut Transaction, inputs: &[usize]) -> Result<(), Self::Error>;
}
