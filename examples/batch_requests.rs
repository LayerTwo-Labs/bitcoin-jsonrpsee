//! # Concurrent Requests Example
//!
//! This example demonstrates how to make multiple batched JSON-RPC requests
//! to efficiently fetch data from Bitcoin Core.

use std::{env, time::Duration};

use futures::TryFutureExt;
use jsonrpsee::core::{
    client::{BatchResponse, ClientT as _},
    params::BatchRequestBuilder,
};

use bitcoin_jsonrpsee::{client::MainClient, jsonrpsee::http_client::HttpClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Bitcoin JSON-RPC Batched Requests Example");
    println!("===============================================\n");

    // Get configuration from environment
    let rpc_host = env::var("BITCOIN_RPC_HOST").expect("BITCOIN_RPC_HOST must be set");
    let rpc_port = env::var("BITCOIN_RPC_PORT").expect("BITCOIN_RPC_PORT must be set");

    let user = env::var("BITCOIN_RPC_USER").expect("BITCOIN_RPC_USER must be set");
    let password = env::var("BITCOIN_RPC_PASSWORD").expect("BITCOIN_RPC_PASSWORD must be set");

    let target = format!("http://{}:{}", rpc_host, rpc_port);
    println!("📡 Connecting to Bitcoin RPC at {}", target);

    // Create client
    let client = bitcoin_jsonrpsee::client(target, None, &password, &user)?;

    // Example 1: Concurrent basic blockchain queries
    println!("\n📊 Example 1: Basic blockchain queries");
    basic_info(&client).await?;

    // Example 2: Concurrent block header requests for multiple heights
    println!("\n🏗️ Example 2: Batched block header requests");
    let start = std::time::Instant::now();
    let batched_block_hashes = batched_block_headers(&client).await?;
    let batched_time = start.elapsed();

    // Example 3: Non-batched block header requests
    let start = std::time::Instant::now();
    println!("\n⚡ Example 3: Non-batched block header requests");
    let non_batched_block_hashes = non_batched_block_headers(&client).await?;
    let non_batched_time = start.elapsed();

    if batched_block_hashes.len() != non_batched_block_hashes.len() {
        return Err("Batched and non-batched block hashes have different lengths".into());
    }

    for (i, (batched, non_batched)) in batched_block_hashes
        .iter()
        .zip(non_batched_block_hashes.iter())
        .enumerate()
    {
        if batched != non_batched {
            println!(
                "⚠️ Header at index {} mismatch: batched: {}, non-batched: {}",
                i, batched, non_batched
            );
        }
    }

    performance_comparison(batched_time, non_batched_time).await?;

    println!("\n🎉 Batched requests example completed!");

    Ok(())
}

async fn basic_info(client: &HttpClient) -> Result<(), Box<dyn std::error::Error>> {
    println!("Fetching blockchain info, best block hash, and block count...");

    // Make three concurrent requests using tokio::join!
    let (blockchain_info, best_hash, block_count) = tokio::join!(
        client.get_blockchain_info(),
        client.getbestblockhash(),
        client.getblockcount()
    );

    // Handle results
    match blockchain_info {
        Ok(info) => {
            println!("✅ Chain: {:?}", info.chain);
            println!("✅ Blocks: {}", info.blocks);
            println!("✅ Difficulty: {:.2}", info.difficulty);
        }
        Err(e) => println!("⚠️ Failed to get blockchain info: {}", e),
    }

    match best_hash {
        Ok(hash) => println!("✅ Best hash: {}", hash),
        Err(e) => println!("⚠️ Failed to get best hash: {}", e),
    }

    match block_count {
        Ok(count) => println!("✅ Block count: {}", count),
        Err(e) => println!("⚠️ Failed to get block count: {}", e),
    }

    Ok(())
}

const BATCHED_HEADER_COUNT: usize = 30;

async fn batched_block_headers(
    client: &HttpClient,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // First get the current height
    let current_height = client.getblockcount().await?;

    if current_height < BATCHED_HEADER_COUNT {
        println!(
            "⚠️ Not enough blocks for this example (need at least {})",
            BATCHED_HEADER_COUNT
        );
        return Ok(Vec::new());
    }

    println!(
        "Fetching headers for the last {} blocks batched...",
        BATCHED_HEADER_COUNT
    );

    let mut req = BatchRequestBuilder::new();

    for i in 0..BATCHED_HEADER_COUNT {
        req.insert("getblockhash", vec![current_height - i])?;
    }
    let res: BatchResponse<String> = client.batch_request(req).await?;

    // Collect successful hashes
    let mut block_hashes = Vec::new();
    let mut heights = Vec::new();

    for (i, hash_result) in res.iter().enumerate() {
        let height = current_height - i;
        match hash_result {
            Ok(hash) => {
                block_hashes.push(hash.clone());
                heights.push(height);
            }
            Err(e) => println!("⚠️ Failed to get hash for block {}: {}", height, e),
        }
    }

    if block_hashes.is_empty() {
        return Err("❌ No block hashes could be retrieved".into());
    }

    for (i, hash) in block_hashes.iter().enumerate() {
        let height = current_height - i;
        println!("✅ Block #{}: {}", height, hash);
    }

    Ok(block_hashes)
}

async fn non_batched_block_headers(
    client: &HttpClient,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let current_height = client.getblockcount().await?;
    let mut futures = Vec::new();
    for i in 0..BATCHED_HEADER_COUNT {
        futures.push(
            client
                .getblockhash(current_height - i)
                .map_ok(|hash| hash.to_string()),
        );
    }

    let results = futures::future::join_all(futures).await;

    let mut block_hashes: Vec<String> = Vec::new();

    for res in results.into_iter() {
        match res {
            Ok(hash) => block_hashes.push(hash),
            Err(e) => return Err(e.into()),
        }
    }

    Ok(block_hashes)
}

async fn performance_comparison(
    batched_time: Duration,
    non_batched_time: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("⏱️ Batched time: {:?}", batched_time);
    println!("⏱️ Non-batched time: {:?}", non_batched_time);

    if batched_time < non_batched_time {
        let speedup = non_batched_time.as_micros() as f64 / batched_time.as_micros() as f64;
        println!("🚀 Batched requests were {:.1}x faster!", speedup);
    } else {
        println!("📊 Results may vary based on network latency and server load");
        println!("💡 Concurrent benefits are more apparent with higher latency connections");
    }

    Ok(())
}
