use crate::Error as CrateError;
use bitcoin::{OutPoint, Transaction, Txid};

pub trait Blockchain {
    type Error: Into<CrateError> + std::fmt::Debug;

    fn get_tx(&self, txid: &Txid) -> Result<Transaction, Self::Error>;
    fn is_unspent(&self, txout: &OutPoint) -> Result<bool, Self::Error>;
    fn get_random_utxo(&self) -> Result<OutPoint, Self::Error>;
    fn broadcast(&self, tx: &Transaction) -> Result<(), Self::Error>;
}
