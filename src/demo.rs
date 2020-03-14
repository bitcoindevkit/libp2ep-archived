use std::str::FromStr;
use std::collections::HashMap;

use log::debug;

use bitcoin::*;
use bitcoin::secp256k1::{Secp256k1, Message, All};
use bitcoin::util::bip143::SighashComponents;
use bitcoin::consensus::encode::{serialize, deserialize};
use bitcoin::hashes::hex::{ToHex, FromHex};
use crate::blockchain::*;
use crate::signer::*;

#[derive(Debug)]
pub struct ElectrumBlockchain {
}

impl ElectrumBlockchain {
    pub fn new() -> Self {
        ElectrumBlockchain {}
    }
}

impl Blockchain for ElectrumBlockchain {
    type Error = ();

    fn get_tx(&self, txid: &Txid) -> Result<Transaction, ()> {
        let string = match txid.to_string().as_str() {
            "c790622f0b33ff5b99ee10f8cb4bfb9271390ed7cfeb596209be75fb6d86e088" => Some("02000000000103d24c899e496427a0f0584be482a81b9ee092d9c46a82b882433cdb7c5071038a010000001716001486bf33a57a3ab02a94b611a9a9683ec990258555feffffff61a256f73e8c139a0400781b8fe376c81c5d0e28efb3eb1f9a2e9967b78c0c1f000000001716001456462c2dcc24f3932134d16ce899cb7828ba4530feffffffe697b21f4968622551c57a5669326eaf4ff311edd65c5679de2fb3de49de5ba7000000001716001465ee068ca37d73848fdf3dc4e2dc8ab74147d069feffffff0200e1f50500000000160014726589f17c655b20a803f4599931907a050d078598e52d00000000001600140e446db06bab1c70c0f138f3891733256c37ba1e02473044022070b997244b2ce7b002c0af99bf3f65d60198cb530b97edffe10fc6802a984b0f022015b507a934ec4076fc1bcf910dbd370406b703aacb4c30acfbe5d1d6acfae197012102ea95b1ff16a2ae46a124d5b988de0ff8d3ea7ef409aed7fa4d46365c96c301210247304402203c6e5f2c14cb0b76d192d71a69a5556dec165b0f36d8f990ef62456b607271330220106824fdde36c0b28809924ce4d888450ab6e7e84fa618490f824f8e709ba0b9012103d95651b6e05f30f536ee06f5d65eeab51865e0492b55fb713a406587333cf5e20247304402201663d5762d7845918903a4d78655078961822a0623c91db609dcb0b1437a599f022009e81dce87333a910bf499a17d8edf0fa9c1d575c9ee3744cd2887757d82f30e012102e66733ae47ddfaa9da5d08eaee7872bbd634c3a02b9fd062758db97ea4bf426d00000000"), 
            "dae03e5b58137ccf6432d80dcaf183814d36301b580d083d6b263129bb57f4c2" => Some("02000000000103d632454680d588d2f66dc9c3fea2f92161086858faf20abfb36ed6ac0c44a3540100000000feffffff1139a91ab521d05052a7bd8594d51c4fb3aa914ed3b6e7e36d1f4df0cc3cad9e0000000017160014ed7db2c701c878c810b5fcd93fc6afde065a70c6feffffff65ac7ff29efacfc119265f0430c61a5c0f366fd8a414cd50ef4f698c9cad0be40000000017160014920a839fd7e701c98266a9485daced1a8e307905feffffff0200c2eb0b00000000160014ac2e7daf42d2c97418fd9f78af2de552bb9c6a7a1dfb3d00000000001600149faa1141da59f41cd283f044d3abf6201dc0317b02473044022002f28612401914eb0075f376e734c7212f13eec94f6ff40eea4351e700e6de85022061d96d4e8fcf5839069ba9d6adf92b6d0f6b4e7ff681e56f459c0917135ab6900121030d830b41d493a447338b5a47bc9de297f799664d6f25bab9f09df1210087bd73024730440220368fee3c8fc0189acfb9b840d7d2504c25a87c2dd3e2bf31ec4fc281bb02988202202b07eef1f367d60c4fdd3bcfaf1673bc7f3621bd13eba0b086c0502fa569b454012102173053c1b70ef6e5d412da2c15f83ddd1cfe601dd42c72475d5c028a0be2cb840247304402203ccfc287e9551f0c73dea6a98a9cf21b99fd1a9dee7f707ed0192ed53d62bf6e02207a0d3c8c67382a1c2a6c0e83af3bd56606132a68b98a46542e9121215590ef6d01210360d6565cdda6184fd2466a74167236b0249cec441675c113951a37992fcc321200000000"),
            _ => None,
        }.unwrap();

        let bytes: Vec<u8> = FromHex::from_hex(string).map_err(|_| ())?;
        Ok(deserialize(&bytes).map_err(|_| ())?)
    }

    fn is_unspent(&self, txout: &OutPoint) -> Result<bool, Self::Error> {
        Ok(true)
    }

    fn get_random_utxo(&self) -> Result<OutPoint, Self::Error> {
        Ok(OutPoint {
            txid: Default::default(),
            vout: 42,
        })
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Self::Error> {
        let bytes = serialize(tx);
        debug!("Broadcasting: {}", bytes.to_hex());
        Ok(())
    }
}

#[derive(Debug)]
pub struct SoftwareSigner {
    key: PrivateKey,
    metadata: HashMap<OutPoint, (u64, Script)>,
}

impl SoftwareSigner {
    pub fn new(key: PrivateKey, metadata: HashMap<OutPoint, (u64, Script)>) -> Self {
        SoftwareSigner {
            key,
            metadata,
        }
    }
}

impl Signer for SoftwareSigner {
    type Error = ();

    fn sign(&self, transaction: &mut Transaction, inputs: &[usize]) -> Result<(), Self::Error> {
        debug!("signing tx: {:?}", transaction);

        let secp: Secp256k1<All> = Secp256k1::gen_new();
        let comp = SighashComponents::new(transaction);

        for (index, input) in transaction.input.iter_mut().enumerate() {
            if !inputs.contains(&index) {
                continue;
            }

            let (amount, prev_script) = self.metadata.get(&input.previous_output).unwrap();
            let hash = comp.sighash_all(&input, &prev_script, *amount);
            let sig = secp.sign(&Message::from_slice(&hash).unwrap(), &self.key.key);

            let mut pubkey = self.key.public_key(&secp);
            pubkey.compressed = true;
            let mut sig_with_sighash = sig.serialize_der().to_vec();
            sig_with_sighash.push(0x01);

            input.witness = vec![
                pubkey.to_bytes().to_vec(),
                sig_with_sighash,
            ];

            debug!("signature: {:?}", sig);
        }

        Ok(())
    }
}

