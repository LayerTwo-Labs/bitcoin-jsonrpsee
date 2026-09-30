use std::{
    collections::{HashMap, HashSet},
    fmt::{self, Debug},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use bitcoin::{block, hashes::Hash as _, BlockHash, Txid, Weight, Wtxid};
use educe::Educe;
use hashlink::LinkedHashMap;
use jsonrpsee::proc_macros::rpc;
use serde::{
    de::{DeserializeOwned, Error as _, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value as JsonValue;

/// Wrapper for consensus (de)serializing from hex
#[derive(Debug, Deserialize, Serialize)]
#[repr(transparent)]
#[serde(
    bound(
        deserialize = "T: bitcoin::consensus::Decodable, Case: bitcoin::consensus::serde::hex::Case",
        serialize = "T: bitcoin::consensus::Encodable, Case: bitcoin::consensus::serde::hex::Case",
    ),
    transparent
)]
pub struct ConsensusEncoded<T, Case = bitcoin::consensus::serde::hex::Lower>(
    #[serde(with = "bitcoin::consensus::serde::With::<bitcoin::consensus::serde::Hex<Case>>")] pub T,
    pub PhantomData<Case>,
);

/// (De)serializes a [`bitcoin::CompactTarget`] as unprefixed big-endian hex,
/// e.g. `"207fffff"`.
mod compact_target_hex {
    use serde::{de::Error as _, Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S>(target: &bitcoin::CompactTarget, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        hex::serde::serialize(target.to_consensus().to_be_bytes(), serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<bitcoin::CompactTarget, D::Error>
    where
        D: Deserializer<'de>,
    {
        let hex = String::deserialize(deserializer)?;
        bitcoin::CompactTarget::from_unprefixed_hex(&hex).map_err(D::Error::custom)
    }
}

/// Like [`hex::serde`], for an optional value.
mod option_hex {
    use serde::{de::Error as _, Deserialize as _, Deserializer, Serialize as _, Serializer};

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.as_ref().map(hex::encode).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<String>::deserialize(deserializer)?
            .map(hex::decode)
            .transpose()
            .map_err(D::Error::custom)
    }
}

/// (De)serializes a map with hex-encoded values, keeping the order of entries.
mod hex_values {
    use hashlink::LinkedHashMap;
    use serde::{de::Error as _, Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S>(
        map: &LinkedHashMap<String, Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_map(map.iter().map(|(key, value)| (key, hex::encode(value))))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<LinkedHashMap<String, Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        LinkedHashMap::<String, String>::deserialize(deserializer)?
            .into_iter()
            .map(|(key, value)| hex::decode(value).map(|value| (key, value)))
            .collect::<Result<_, _>>()
            .map_err(D::Error::custom)
    }
}

/// Deserializes a map into its entries, in order.
fn deserialize_entries<'de, D, K, V>(deserializer: D) -> Result<Vec<(K, V)>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de>,
    V: Deserialize<'de>,
{
    struct EntriesVisitor<K, V>(PhantomData<(K, V)>);

    impl<'de, K, V> Visitor<'de> for EntriesVisitor<K, V>
    where
        K: Deserialize<'de>,
        V: Deserialize<'de>,
    {
        type Value = Vec<(K, V)>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut entries = Vec::new();
            while let Some(entry) = map.next_entry()? {
                entries.push(entry);
            }
            Ok(entries)
        }
    }

    deserializer.deserialize_map(EntriesVisitor(PhantomData))
}

/// Deserializes a value that must equal `expected`.
fn deserialize_exact<'de, D, T>(deserializer: D, expected: T) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + PartialEq + fmt::Display,
{
    let value = T::deserialize(deserializer)?;
    if value == expected {
        Ok(())
    } else {
        Err(D::Error::custom(format!(
            "invalid value `{value}`, expected `{expected}`"
        )))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Header {
    pub hash: BlockHash,
    pub height: u32,
    pub version: bitcoin::block::Version,
    #[serde(rename = "previousblockhash", default = "BlockHash::all_zeros")]
    pub prev_blockhash: BlockHash,
    #[serde(rename = "merkleroot")]
    pub merkle_root: bitcoin::TxMerkleNode,
    pub time: u32,
    #[serde(with = "compact_target_hex")]
    pub bits: bitcoin::CompactTarget,
    pub nonce: u32,
}

impl Header {
    /// Computes the target (range [0, T] inclusive) that a blockhash must land in to be valid.
    pub fn target(&self) -> bitcoin::Target {
        self.bits.into()
    }

    /// Returns the total work of the block.
    pub fn work(&self) -> bitcoin::Work {
        self.target().to_work()
    }
}

impl From<Header> for bitcoin::block::Header {
    fn from(header: Header) -> Self {
        Self {
            version: header.version,
            prev_blockhash: header.prev_blockhash,
            merkle_root: header.merkle_root,
            time: header.time,
            bits: header.bits,
            nonce: header.nonce,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct MiningInfoNext {
    pub height: u32,
    #[serde(with = "compact_target_hex")]
    pub bits: bitcoin::CompactTarget,
    pub difficulty: f64,
    pub target: bitcoin::Target,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MiningInfo {
    #[serde(with = "bitcoin::network::as_core_arg")]
    pub chain: bitcoin::Network,
    pub signet_challenge: Option<bitcoin::ScriptBuf>,
    pub next: MiningInfoNext,
}

/// Core reports these in BTC, not satoshi.
///
/// All fields except `base` include `prioritisetransaction` deltas, which can
/// be negative.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct RawMempoolTxFees {
    #[serde(with = "bitcoin::amount::serde::as_btc")]
    pub base: bitcoin::Amount,
    #[serde(with = "bitcoin::amount::serde::as_btc")]
    pub modified: bitcoin::SignedAmount,
    #[serde(with = "bitcoin::amount::serde::as_btc")]
    pub ancestor: bitcoin::SignedAmount,
    #[serde(with = "bitcoin::amount::serde::as_btc")]
    pub descendant: bitcoin::SignedAmount,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawMempoolTxInfo {
    pub vsize: u64,
    pub weight: u64,
    #[serde(rename = "descendantcount")]
    pub descendant_count: u64,
    #[serde(rename = "descendantsize")]
    pub descendant_size: u64,
    #[serde(rename = "ancestorcount")]
    pub ancestor_count: u64,
    #[serde(rename = "ancestorsize")]
    pub ancestor_size: u64,
    pub wtxid: Wtxid,
    pub fees: RawMempoolTxFees,
    pub depends: Vec<Txid>,
    #[serde(rename = "spentby")]
    pub spent_by: Vec<Txid>,
    #[serde(rename = "bip125-replaceable")]
    pub bip125_replaceable: bool,
    pub unbroadcast: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawMempoolWithSequence {
    pub txids: Vec<Txid>,
    pub mempool_sequence: u64,
}

/// `getrawmempool verbose=true`.
//
// Core returns the entries as a bare JSON object keyed by txid with no
// wrapper.
#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
pub struct RawMempoolVerbose {
    #[serde(deserialize_with = "deserialize_entries")]
    pub entries: Vec<(Txid, RawMempoolTxInfo)>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TxOutSetInfo {
    pub height: u32,
    #[serde(rename = "bestblock")]
    pub best_block: BlockHash,
    #[serde(rename = "transactions")]
    pub n_txs: u64,
    #[serde(rename = "txouts")]
    pub n_txouts: u64,
    #[serde(with = "hex::serde")]
    pub hash_serialized_3: [u8; 32],
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Vote {
    Upvote,
    Abstain,
    Downvote,
}

#[derive(Clone, Debug, Deserialize)]
pub struct NetworkInfo {
    // Time offset in seconds
    #[serde(rename = "timeoffset")]
    pub time_offset_s: i64,
}

/// Output from `getrawtransaction` where `verbosity = 1`
#[derive(Clone, Debug, Deserialize)]
pub struct TxInfo {
    #[serde(deserialize_with = "hex::serde::deserialize")]
    pub hex: Vec<u8>,
    pub txid: Txid,
    // TODO: add more fields
}

mod private {
    pub trait Sealed {}
}

impl<const BOOL: bool> private::Sealed for BoolWitness<BOOL> {}

pub trait ShowTxDetails: private::Sealed {
    type Output;
}

impl ShowTxDetails for BoolWitness<false> {
    type Output = Txid;
}

impl ShowTxDetails for BoolWitness<true> {
    type Output = TxInfo;
}

#[derive(Educe)]
#[educe(
    Clone(bound(<BoolWitness<SHOW_TX_DETAILS> as ShowTxDetails>::Output: Clone)),
    Debug(bound(<BoolWitness<SHOW_TX_DETAILS> as ShowTxDetails>::Output: Debug)),
)]
#[derive(Deserialize, Serialize)]
#[serde(
    bound(
        deserialize = "for<'des> <BoolWitness<SHOW_TX_DETAILS> as ShowTxDetails>::Output: Deserialize<'des>",
        serialize = "<BoolWitness<SHOW_TX_DETAILS> as ShowTxDetails>::Output: Serialize"
    ),
    rename_all = "camelCase"
)]
pub struct Block<const SHOW_TX_DETAILS: bool>
where
    BoolWitness<SHOW_TX_DETAILS>: ShowTxDetails,
{
    pub hash: bitcoin::BlockHash,
    pub confirmations: isize, // Confirmations can be negative if block are reorged/invalidated
    pub strippedsize: usize,
    pub size: usize,
    pub weight: usize,
    pub height: u32,
    pub version: bitcoin::block::Version,
    pub version_hex: String,
    pub merkleroot: bitcoin::hash_types::TxMerkleNode,
    pub tx: Vec<<BoolWitness<SHOW_TX_DETAILS> as ShowTxDetails>::Output>,
    pub time: u32,
    pub mediantime: u32,
    pub nonce: u32,
    #[serde(rename = "bits")]
    #[serde(with = "compact_target_hex")]
    pub compact_target: bitcoin::CompactTarget,
    pub difficulty: f64,
    pub chainwork: String,
    pub previousblockhash: Option<bitcoin::BlockHash>,
    pub nextblockhash: Option<bitcoin::BlockHash>,
}

impl TryFrom<&Block<true>> for bitcoin::Block {
    type Error = bitcoin::consensus::encode::Error;

    fn try_from(block: &Block<true>) -> Result<Self, Self::Error> {
        let header = bitcoin::block::Header {
            version: block.version,
            prev_blockhash: block.previousblockhash.unwrap_or_else(BlockHash::all_zeros),
            merkle_root: block.merkleroot,
            time: block.time,
            bits: block.compact_target,
            nonce: block.nonce,
        };
        let txdata = block
            .tx
            .iter()
            .map(|tx_info| bitcoin::consensus::deserialize(&tx_info.hex))
            .collect::<Result<_, _>>()?;
        Ok(Self { header, txdata })
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BlockTemplateRequest {
    /// BIP22/BIP23 `mode`. Absent, or JSON null, is equivalent to
    /// `"template"`. Read it through [`BlockTemplateRequest::mode`].
    ///
    /// Deliberately untyped. An unrecognised mode, and a `mode` that is not a
    /// string at all, are both `Invalid mode` errors in Bitcoin Core. Typing
    /// this as an enum or a `String` would instead fail deserialization and
    /// report the whole params object as malformed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<JsonValue>,
    /// BIP23 block proposal: the hex-encoded block to validate. Required when
    /// `mode` is `"proposal"`, ignored otherwise. Read it through
    /// [`BlockTemplateRequest::data`].
    ///
    /// Left as hex rather than a decoded block so that undecodable data can be
    /// reported as its own error, the way Bitcoin Core does. Untyped for the
    /// same reason as `mode`: Core tests it with `isStr()`, so a `data` that
    /// is present but not a string is the same error as an absent one, not
    /// malformed params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub capabilities: HashSet<String>,
    /// BIP22 long polling: the `longpollid` from a previous template response.
    /// A server that supports long polling holds the request open until that
    /// template is stale (e.g. the chain tip changed), then responds with a
    /// fresh template.
    #[serde(
        default,
        rename = "longpollid",
        skip_serializing_if = "Option::is_none"
    )]
    pub long_poll_id: Option<String>,
}

/// `mode` was present but was not a JSON string.
///
/// Bitcoin Core reports this exactly as it reports an unrecognised mode, so
/// callers should map it to the same error.
///
/// <https://github.com/bitcoin/bitcoin/blob/6c4fe401e908cff1b67d80035b117aae15fe7db6/src/rpc/mining.cpp#L726>
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("Invalid mode")]
pub struct InvalidMode;

impl BlockTemplateRequest {
    /// BIP22/BIP23 mode, defaulting to `"template"` when absent or null.
    ///
    /// Mirrors Core's `isStr()` / `isNull()` / else split, and an unrecognised
    /// string is the caller's to reject, the same way Core does further down.
    ///
    /// <https://github.com/bitcoin/bitcoin/blob/6c4fe401e908cff1b67d80035b117aae15fe7db6/src/rpc/mining.cpp#L718-L726>
    /// <https://github.com/bitcoin/bitcoin/blob/6c4fe401e908cff1b67d80035b117aae15fe7db6/src/rpc/mining.cpp#L763>
    pub fn mode(&self) -> Result<&str, InvalidMode> {
        match &self.mode {
            None | Some(JsonValue::Null) => Ok(MODE_TEMPLATE),
            Some(JsonValue::String(mode)) => Ok(mode),
            Some(_) => Err(InvalidMode),
        }
    }

    /// BIP23 proposal `data`, as a hex string.
    ///
    /// `None` when absent *or* when present as some other JSON type: Bitcoin
    /// Core tests it with `isStr()` and reports both identically, so there is
    /// nothing for a caller to tell apart.
    ///
    /// <https://github.com/bitcoin/bitcoin/blob/6c4fe401e908cff1b67d80035b117aae15fe7db6/src/rpc/mining.cpp#L731-L733>
    pub fn data(&self) -> Option<&str> {
        match &self.data {
            Some(JsonValue::String(data)) => Some(data),
            _ => None,
        }
    }
}

impl Default for BlockTemplateRequest {
    fn default() -> Self {
        Self {
            mode: None,
            data: None,
            rules: vec!["segwit".into()],
            capabilities: HashSet::new(),
            long_poll_id: None,
        }
    }
}

/// The default `getblocktemplate` mode: build and return a new template.
pub const MODE_TEMPLATE: &str = "template";

/// BIP23 block proposal mode: validate the submitted block instead of building
/// a template.
pub const MODE_PROPOSAL: &str = "proposal";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BlockTemplateTransaction {
    #[serde(with = "hex::serde")]
    pub data: Vec<u8>,
    pub txid: Txid,
    // TODO: check that this is the wtxid
    pub hash: Wtxid,
    pub depends: Vec<u32>,
    #[serde(with = "bitcoin::amount::serde::as_sat")]
    pub fee: bitcoin::SignedAmount,
    pub sigops: Option<u64>,
    pub weight: u64,
}

/// `coinbasetxn` or `coinbasevalue` field
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CoinbaseTxnOrValue {
    #[serde(rename = "coinbasetxn")]
    Txn(BlockTemplateTransaction),
    #[serde(rename = "coinbasevalue")]
    ValueSats(u64),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BlockTemplate {
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub version: block::Version,
    pub rules: Vec<String>,
    #[serde(rename = "vbavailable")]
    pub version_bits_available: LinkedHashMap<String, JsonValue>,
    #[serde(rename = "vbrequired")]
    pub version_bits_required: block::Version,
    #[serde(rename = "previousblockhash")]
    pub prev_blockhash: bitcoin::BlockHash,
    pub transactions: Vec<BlockTemplateTransaction>,
    #[serde(rename = "coinbaseaux")]
    #[serde(with = "hex_values")]
    pub coinbase_aux: LinkedHashMap<String, Vec<u8>>,
    #[serde(flatten)]
    pub coinbase_txn_or_value: CoinbaseTxnOrValue,
    /// MUST be omitted if the server does not support long polling
    #[serde(rename = "longpollid")]
    pub long_poll_id: Option<String>,
    #[serde(with = "hex::serde")]
    pub target: [u8; 32],
    pub mintime: u64,
    pub mutable: Vec<String>,
    #[serde(rename = "noncerange")]
    #[serde(with = "hex::serde")]
    pub nonce_range: [u8; 8],
    #[serde(rename = "sigoplimit")]
    pub sigop_limit: u64,
    #[serde(rename = "sizelimit")]
    pub size_limit: u64,
    #[serde(rename = "weightlimit")]
    pub weight_limit: Weight,
    #[serde(rename = "curtime")]
    pub current_time: u64,
    #[serde(rename = "bits")]
    #[serde(with = "compact_target_hex")]
    pub compact_target: bitcoin::CompactTarget,
    pub height: u32,
    pub signet_challenge: Option<bitcoin::ScriptBuf>,
    #[serde(default, with = "option_hex")]
    pub default_witness_commitment: Option<Vec<u8>>,
}

#[derive(Debug, Deserialize)]
pub struct AddressInfo {
    pub address: bitcoin::Address<bitcoin::address::NetworkUnchecked>,
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: String,
    #[serde(rename = "ismine")]
    pub is_mine: bool,
    #[serde(rename = "iswatchonly")]
    pub is_watch_only: bool,
    #[serde(rename = "isscript")]
    pub is_script: bool,
    #[serde(rename = "iswitness")]
    pub is_witness: bool,
    #[serde(rename = "hdkeypath")]
    pub hd_key_path: Option<String>,
    #[serde(rename = "hdseedid")]
    pub hd_seed_id: Option<String>,
}

/// Additional blockchain info, present after v29
#[derive(Debug, Deserialize)]
pub struct BlockchainInfoV29 {
    #[serde(rename = "bits")]
    #[serde(with = "compact_target_hex")]
    pub compact_target: bitcoin::CompactTarget,
    #[serde(with = "hex::serde")]
    pub target: [u8; 32],
}

#[derive(Debug, Deserialize)]
pub struct BlockchainInfo {
    #[serde(with = "bitcoin::network::as_core_arg")]
    pub chain: bitcoin::Network,
    pub blocks: u32,
    #[serde(rename = "bestblockhash")]
    pub best_blockhash: bitcoin::BlockHash,
    pub difficulty: f64,
    #[serde(flatten)]
    pub v29_info: Option<BlockchainInfoV29>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IndexInfo {
    pub synced: bool,
    pub best_block_height: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ZMQNotification {
    #[serde(rename = "type")]
    pub notification_type: String,
    pub address: String,
    #[serde(rename = "hwm")]
    pub high_water_mark: u32,
}

#[rpc(client)]
pub trait Main {
    #[method(name = "generate")]
    async fn generate(&self, num: u32) -> Result<serde_json::Value, jsonrpsee::core::Error>;

    #[method(name = "generatetoaddress")]
    async fn generate_to_address(
        &self,
        n_blocks: u32,
        address: &bitcoin::Address<bitcoin::address::NetworkUnchecked>,
    ) -> Result<Vec<BlockHash>, jsonrpsee::core::Error>;

    #[method(name = "getblocktemplate")]
    async fn get_block_template(
        &self,
        block_template_request: BlockTemplateRequest,
    ) -> Result<BlockTemplate, jsonrpsee::core::Error>;

    #[method(name = "getblockchaininfo")]
    async fn get_blockchain_info(&self) -> Result<BlockchainInfo, jsonrpsee::core::Error>;

    #[method(name = "getmininginfo")]
    async fn get_mining_info(&self) -> Result<MiningInfo, jsonrpsee::core::Error>;

    #[method(name = "getmempoolentry")]
    async fn get_mempool_entry(
        &self,
        txid: Txid,
    ) -> Result<RawMempoolTxInfo, jsonrpsee::core::Error>;

    #[method(name = "getnetworkinfo")]
    async fn get_network_info(&self) -> jsonrpsee::core::RpcResult<NetworkInfo>;

    #[method(name = "getbestblockhash")]
    async fn getbestblockhash(&self) -> Result<bitcoin::BlockHash, jsonrpsee::core::Error>;

    #[method(name = "getblockhash")]
    async fn getblockhash(
        &self,
        height: usize,
    ) -> Result<bitcoin::BlockHash, jsonrpsee::core::Error>;

    #[method(name = "getblockcount")]
    async fn getblockcount(&self) -> Result<usize, jsonrpsee::core::Error>;

    #[method(name = "getblockheader")]
    async fn getblockheader(
        &self,
        block_hash: bitcoin::BlockHash,
    ) -> Result<Header, jsonrpsee::core::Error>;

    #[method(name = "getaddressinfo")]
    async fn get_address_info(
        &self,
        address: &bitcoin::Address<bitcoin::address::NetworkUnchecked>,
    ) -> Result<AddressInfo, jsonrpsee::core::Error>;

    #[method(name = "getnewaddress")]
    async fn getnewaddress(
        &self,
        account: &str,
        address_type: &str,
    ) -> Result<bitcoin::Address<bitcoin::address::NetworkUnchecked>, jsonrpsee::core::Error>;

    #[method(name = "getindexinfo")]
    async fn get_index_info(&self) -> Result<HashMap<String, IndexInfo>, jsonrpsee::core::Error>;

    #[method(name = "gettxoutsetinfo")]
    async fn gettxoutsetinfo(&self) -> Result<TxOutSetInfo, jsonrpsee::core::Error>;

    #[method(name = "invalidateblock")]
    async fn invalidate_block(
        &self,
        block_hash: bitcoin::BlockHash,
    ) -> Result<(), jsonrpsee::core::Error>;

    #[method(name = "prioritisetransaction", param_kind = map)]
    async fn prioritize_transaction(
        &self,
        txid: Txid,
        fee_delta: i64,
    ) -> Result<bool, jsonrpsee::core::Error>;

    // Max fee rate: BTC/kvB value
    // Max burn amount: BTC value
    #[method(name = "sendrawtransaction")]
    async fn send_raw_transaction(
        &self,
        tx_hex: String,
        max_fee_rate: Option<f64>,
        max_burn_amount: Option<f64>,
    ) -> Result<bitcoin::Txid, jsonrpsee::core::Error>;

    #[method(name = "stop")]
    async fn stop(&self) -> Result<String, jsonrpsee::core::Error>;

    /// Returns None if the block is invalid, otherwise the error code describing why the
    /// block was rejected.
    #[method(name = "submitblock")]
    async fn submit_block(
        &self,
        block_hex: String,
    ) -> Result<Option<String>, jsonrpsee::core::Error>;

    #[method(name = "getzmqnotifications")]
    async fn get_zmq_notifications(&self) -> Result<Vec<ZMQNotification>, jsonrpsee::core::error>;
}

pub struct U8Witness<const U8: u8>;

impl<const U8: u8> Serialize for U8Witness<{ U8 }> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        U8.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for U8Witness<0> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_exact(deserializer, 0u8).map(|()| Self)
    }
}

impl<'de> Deserialize<'de> for U8Witness<1> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_exact(deserializer, 1u8).map(|()| Self)
    }
}

impl<'de> Deserialize<'de> for U8Witness<2> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_exact(deserializer, 2u8).map(|()| Self)
    }
}

pub trait GetBlockVerbosity {
    type Response: DeserializeOwned;
}

impl GetBlockVerbosity for U8Witness<0> {
    type Response = ConsensusEncoded<bitcoin::Block>;
}

impl GetBlockVerbosity for U8Witness<1> {
    type Response = Block<false>;
}

impl GetBlockVerbosity for U8Witness<2> {
    type Response = Block<true>;
}

#[rpc(
    client,
    client_bounds(Verbosity: Serialize + Send + Sync + 'static)
)]
pub trait GetBlock<Verbosity>
where
    Verbosity: GetBlockVerbosity,
{
    #[method(name = "getblock")]
    async fn get_block(
        &self,
        block_hash: BlockHash,
        verbosity: Verbosity,
    ) -> Result<<Verbosity as GetBlockVerbosity>::Response, jsonrpsee::core::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoolWitness<const BOOL: bool>;

impl<const BOOL: bool> Serialize for BoolWitness<{ BOOL }> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        BOOL.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BoolWitness<false> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_exact(deserializer, false).map(|()| Self)
    }
}

impl<'de> Deserialize<'de> for BoolWitness<true> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_exact(deserializer, true).map(|()| Self)
    }
}

pub struct GetRawMempoolParams<Verbose, MempoolSequence>(PhantomData<(Verbose, MempoolSequence)>);

pub trait GetRawMempoolResponse {
    type Response: DeserializeOwned;
}

// There is deliberately no impl for `<Verbose = true, MempoolSequence = true>`.
// Core rejects that combination.
impl GetRawMempoolResponse for GetRawMempoolParams<BoolWitness<false>, BoolWitness<false>> {
    type Response = Vec<Txid>;
}

impl GetRawMempoolResponse for GetRawMempoolParams<BoolWitness<false>, BoolWitness<true>> {
    type Response = RawMempoolWithSequence;
}

impl GetRawMempoolResponse for GetRawMempoolParams<BoolWitness<true>, BoolWitness<false>> {
    type Response = RawMempoolVerbose;
}

#[rpc(
    client,
    client_bounds(
        Verbose: Serialize + Send + Sync + 'static,
        MempoolSequence: Serialize + Send + Sync + 'static,
        GetRawMempoolParams<Verbose, MempoolSequence>: GetRawMempoolResponse
    )
)]
pub trait GetRawMempool<Verbose, MempoolSequence>
where
    GetRawMempoolParams<Verbose, MempoolSequence>: GetRawMempoolResponse,
{
    #[method(name = "getrawmempool")]
    async fn get_raw_mempool(
        &self,
        verbose: Verbose,
        mempool_sequence: MempoolSequence,
    ) -> Result<
        <GetRawMempoolParams<Verbose, MempoolSequence> as GetRawMempoolResponse>::Response,
        jsonrpsee::core::Error,
    >;
}

pub trait GetRawTransactionVerbosity {
    type Response: DeserializeOwned;
}

#[derive(Debug)]
pub struct GetRawTransactionVerbose<const VERBOSE: bool>;

impl<const VERBOSE: bool> Serialize for GetRawTransactionVerbose<{ VERBOSE }> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        VERBOSE.serialize(serializer)
    }
}

impl GetRawTransactionVerbosity for GetRawTransactionVerbose<false> {
    type Response = String;
}

impl<'de> Deserialize<'de> for GetRawTransactionVerbose<false> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_exact(deserializer, false).map(|()| Self)
    }
}

impl GetRawTransactionVerbosity for GetRawTransactionVerbose<true> {
    type Response = serde_json::Value;
}

impl<'de> Deserialize<'de> for GetRawTransactionVerbose<true> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_exact(deserializer, true).map(|()| Self)
    }
}

#[rpc(client)]
pub trait GetRawTransaction<T>
where
    T: GetRawTransactionVerbosity,
{
    #[method(name = "getrawtransaction")]
    async fn get_raw_transaction(
        &self,
        txid: Txid,
        verbose: T,
        block_hash: Option<bitcoin::BlockHash>,
    ) -> Result<<T as GetRawTransactionVerbosity>::Response, jsonrpsee::core::Error>;
}

// FIXME: Make mainchain API machine friendly. Parsing human readable amounts
// here is stupid -- just take and return values in satoshi.
#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct AmountBtc(#[serde(with = "bitcoin::amount::serde::as_btc")] pub bitcoin::Amount);

impl From<bitcoin::Amount> for AmountBtc {
    fn from(other: bitcoin::Amount) -> AmountBtc {
        AmountBtc(other)
    }
}

impl From<AmountBtc> for bitcoin::Amount {
    fn from(other: AmountBtc) -> bitcoin::Amount {
        other.0
    }
}

impl Deref for AmountBtc {
    type Target = bitcoin::Amount;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AmountBtc {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
