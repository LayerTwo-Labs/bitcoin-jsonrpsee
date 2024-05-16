use crate::client;

use jsonrpsee::{
    core::RpcResult,
    types::{response, Response},
};

#[test]

// Test deserializing a result from `getblockcommitments`
fn test_deserialize_getblockcommitments() {
    let json_str = include_str!("json/getblockcommitments.json");
    let mut json_des = serde_json::Deserializer::from_str(json_str);
    let res: Response<client::BlockCommitments> = serde_path_to_error::deserialize(&mut json_des)
        .expect("Failed to deserialize block header");
    let res: RpcResult<response::Success<_>> = res.try_into();
    assert!(res.is_ok())
}

// Test deserializing a result from `getblockheader`
#[test]
fn test_deserialize_getblockheader() {
    let json_str = include_str!("json/getblockheader.json");
    let mut json_des = serde_json::Deserializer::from_str(json_str);
    let res: Response<client::Header> = serde_path_to_error::deserialize(&mut json_des)
        .expect("Failed to deserialize block header");
    let res: RpcResult<response::Success<_>> = res.try_into();
    assert!(res.is_ok())
}

// Test deserializing a genesis block result from `getblockheader`.
// The genesis block header will have no `previousblockhash`.
#[test]
fn test_deserialize_getblockheader_genesis() {
    let json_str = include_str!("json/getblockheader-genesis.json");
    let mut json_des = serde_json::Deserializer::from_str(json_str);
    let res: Response<client::Header> = serde_path_to_error::deserialize(&mut json_des)
        .expect("Failed to deserialize block header");
    let res: RpcResult<response::Success<_>> = res.try_into();
    assert!(res.is_ok())
}
