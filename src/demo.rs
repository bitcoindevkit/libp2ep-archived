use std::collections::HashMap;

use log::debug;

use crate::blockchain::*;
use crate::signer::*;

use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{All, Message, Secp256k1};
use bitcoin::util::bip143::SighashComponents;
use bitcoin::*;

#[derive(Debug, Default)]
pub struct ElectrumBlockchain {}

impl ElectrumBlockchain {
    pub fn new() -> Self {
        Default::default()
    }
}

impl Blockchain for ElectrumBlockchain {
    type Error = ();

    fn get_tx(&self, txid: &Txid) -> Result<Transaction, ()> {
        let string = match txid.to_string().as_str() {
            "c790622f0b33ff5b99ee10f8cb4bfb9271390ed7cfeb596209be75fb6d86e088" => Some("02000000000103d24c899e496427a0f0584be482a81b9ee092d9c46a82b882433cdb7c5071038a010000001716001486bf33a57a3ab02a94b611a9a9683ec990258555feffffff61a256f73e8c139a0400781b8fe376c81c5d0e28efb3eb1f9a2e9967b78c0c1f000000001716001456462c2dcc24f3932134d16ce899cb7828ba4530feffffffe697b21f4968622551c57a5669326eaf4ff311edd65c5679de2fb3de49de5ba7000000001716001465ee068ca37d73848fdf3dc4e2dc8ab74147d069feffffff0200e1f50500000000160014726589f17c655b20a803f4599931907a050d078598e52d00000000001600140e446db06bab1c70c0f138f3891733256c37ba1e02473044022070b997244b2ce7b002c0af99bf3f65d60198cb530b97edffe10fc6802a984b0f022015b507a934ec4076fc1bcf910dbd370406b703aacb4c30acfbe5d1d6acfae197012102ea95b1ff16a2ae46a124d5b988de0ff8d3ea7ef409aed7fa4d46365c96c301210247304402203c6e5f2c14cb0b76d192d71a69a5556dec165b0f36d8f990ef62456b607271330220106824fdde36c0b28809924ce4d888450ab6e7e84fa618490f824f8e709ba0b9012103d95651b6e05f30f536ee06f5d65eeab51865e0492b55fb713a406587333cf5e20247304402201663d5762d7845918903a4d78655078961822a0623c91db609dcb0b1437a599f022009e81dce87333a910bf499a17d8edf0fa9c1d575c9ee3744cd2887757d82f30e012102e66733ae47ddfaa9da5d08eaee7872bbd634c3a02b9fd062758db97ea4bf426d00000000"), 
            "17eb46f996ebfbc404080872e29352cc55dc3906458ceb279bc9eb768727c5e0" => Some("0200000000010488e0866dfb75be096259ebcfd70e397192fb4bcbf810ee995bff330b2f6290c70100000000feffffff5d1c72b49f53ba1b4a2648337fde24e3007fcdd01a63c61d3f96306e11b13f0f0000000017160014fc15e593cd786832a32988badf6ca4bb66de3877feffffff55853f9c9c92a8dac14101d7a997527e57c86c4208cde9e89992e1b3d220572f0000000017160014dd579a5685f69c1804391214045c8433c127b29efeffffffc2f457bb2931266b3d080d581b30364d8183f1ca0dd83264cf7c13585b3ee0da0100000000feffffff0200c2eb0b00000000160014751e76e8199196d454941c45d1b3a323f1433bd6c2c424000000000016001485379afd3d8810c77992f50fd3bce240168d09f00247304402205cb64da5841f49a265ac357dd5a73a0d54f9780a39feda5767f385343237b99802203e1057052eab0e638b1f9ac69aea686cae06292ce7ba86547da9bc3203b11002012102d3adcc8e90f0d9cbfd5ce854ed3fc9c7add9a28d99fa46dd0285f397f0f9ecd702473044022069c21c3347bc07cbf9aa6065baf7c05195f7bdbce4db1f241788ffb3625d820b02204d91f38478f23adfbb1463fe319c8bdb16a3120e6d3855ea0bd2aa89b6b12b930121039f995c732f385304ba583e0e641d79cd8f1db05edfd39dc508645111b5218ddd024730440220533cf0a9c0020b2c059435bddcff997fc59a5b50d7aba5c2bf57fe528dc29bd80220519c2eb67712a05b79f29cf3c8fb24f1e76a87c90608443798e5fe0cf87df2f4012103f4e76893a5e4e87e716d2f3a35a1b5ccf2c0228988bf9e610d674103fa0a9bae0247304402207646c6c3ed62c6d1d64c0ab84eedcd70d7c9141f0da98e408cbc3c318f49715f022021a03c86da41100d90ee0ece983f33bd7ee28eff1e9d1352522f3bc7e2dc3a5e0121036bcd9ab18a62382f65e0a4acc587a91ef8c86477a326b593bac7234fa02db170d8050000"),
            "0f3fb1116e30963f1dc6631ad0cd7f00e324de7f3348264a1bba539fb4721c5d" => Some("020000000001010000000000000000000000000000000000000000000000000000000000000000ffffffff05023a030101ffffffff023e7e50090000000017a914b6dfeb91d03f56c1575d1b80c755db1ba029f2dd870000000000000000266a24aa21a9ed72b438543ba67fbbcfea524294609f96e2923969c4ef384e8660e8aa37544a240120000000000000000000000000000000000000000000000000000000000000000000000000"),
            _ => None,
        }.unwrap();

        let bytes: Vec<u8> = FromHex::from_hex(string).map_err(|_| ())?;
        Ok(deserialize(&bytes).map_err(|_| ())?)
    }

    fn is_unspent(&self, _txout: &OutPoint) -> Result<bool, Self::Error> {
        Ok(true)
    }

    fn get_random_utxo(&self) -> Result<OutPoint, Self::Error> {
        Ok(OutPoint {
            txid: Txid::from_hex(
                "0f3fb1116e30963f1dc6631ad0cd7f00e324de7f3348264a1bba539fb4721c5d",
            )
            .unwrap(),
            vout: 0,
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
        SoftwareSigner { key, metadata }
    }
}

impl Signer for SoftwareSigner {
    type Error = ();

    fn sign(&self, transaction: &mut Transaction, inputs: &[usize]) -> Result<(), Self::Error> {
        debug!("signing tx: {:?}", transaction);

        let secp: Secp256k1<All> = Secp256k1::gen_new();
        let comp = SighashComponents::new(&transaction);

        for (index, input) in transaction.input.iter_mut().enumerate() {
            if !inputs.contains(&index) {
                continue;
            }

            let (amount, prev_script) = self.metadata.get(&input.previous_output).unwrap();
            let script_code = Self::p2wpkh_scriptcode(&prev_script);
            println!(
                "input: {} scriptcode: {} value: {}",
                index,
                script_code.to_hex(),
                *amount
            );

            let hash = comp.sighash_all(input, &script_code, *amount);
            let sig = secp.sign(
                &Message::from_slice(&hash.into_inner()[..]).unwrap(),
                &self.key.key,
            );

            let mut pubkey = self.key.public_key(&secp);
            pubkey.compressed = true;
            let mut sig_with_sighash = sig.serialize_der().to_vec();
            sig_with_sighash.push(0x01);

            input.witness = vec![sig_with_sighash, pubkey.to_bytes().to_vec()];

            debug!("signature: {:?}", sig);
        }

        Ok(())
    }
}
