use alloy::signers::local::PrivateKeySigner;
use dotenvy::dotenv;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::env;
use tokio_tungstenite::connect_async;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use x402_reqwest::chains::evm::EvmSenderWallet;
use x402_reqwest::X402Payments;
use x402_rs::types::PaymentRequirements;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env.buyer (project root) and also example-local path, then fallback to .env
    let _ = dotenvy::from_filename(".env.buyer");
    let _ = dotenvy::from_filename("examples/x402-ws-example/.env.buyer");
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let seller_ws = env::var("SELLER_WS_URL").unwrap_or_else(|_| "ws://localhost:8081/ws".into());
    let (mut ws, _) = connect_async(seller_ws.as_str()).await?;

    let evm_pk: PrivateKeySigner = env::var("EVM_PRIVATE_KEY")?.parse()?;
    let buyer_addr = evm_pk.address();
    let payments = X402Payments::with_wallet(EvmSenderWallet::new(evm_pk));
    tracing::info!(buyer_address = %buyer_addr, "Buyer ready");

    // Send stream.init
    let init = json!({
        "id": Uuid::new_v4().to_string(),
        "method": "stream.init",
        "params": { "resource": "wss://example/stream", "network": "polygon-amoy" }
    });
    tracing::info!(env = %init, "Sending stream.init");
    ws
        .send(tokio_tungstenite::tungstenite::Message::Text(
            init.to_string().into(),
        ))
        .await?;

    while let Some(msg) = ws.next().await {
        let msg = msg?;
        if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
            tracing::debug!(raw = %text, "WS recv");
            let val: serde_json::Value = serde_json::from_str(&text)?;
            if let Some(err) = val.get("error") {
                tracing::warn!(error = %err, "WS error envelope from seller");
            }
            if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
                match method {
                    "stream.require" => {
                        let params = val.get("params").cloned().unwrap_or_default();
                        let stream_id = params.get("streamId").and_then(|v| v.as_str()).unwrap_or("");
                        let slice_index = params.get("sliceIndex").and_then(|v| v.as_u64()).unwrap_or(0);
                        let requirements_json = params.get("requirements").cloned().unwrap();
                        let requirements: PaymentRequirements = serde_json::from_value(requirements_json.clone())?;

                        // Build PaymentPayload using reqwest's signer logic
                        let payload = payments.make_payment_payload(requirements).await?;
                        tracing::info!(%stream_id, slice_index, "Sending stream.pay");
                        let env = json!({
                            "id": val.get("id").cloned().unwrap_or_else(|| json!(Uuid::new_v4().to_string())),
                            "method": "stream.pay",
                            "params": {
                                "streamId": stream_id,
                                "sliceIndex": slice_index,
                                "paymentPayload": payload,
                                "requirements": requirements_json,
                                "verifyOnly": false,
                            }
                        });
                        ws
                            .send(tokio_tungstenite::tungstenite::Message::Text(
                                env.to_string().into(),
                            ))
                            .await?;
                    }
                    _ => {}
                }
            } else if let Some(result) = val.get("result") {
                // Handle "stream.accept" envelope shape from seller
                if result.get("method").and_then(|m| m.as_str()) == Some("stream.accept") {
                    let prepaid_until = result
                        .get("params")
                        .and_then(|p| p.get("prepaidUntilMs"))
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let verify = result.get("params").and_then(|p| p.get("verify"));
                    let settle = result.get("params").and_then(|p| p.get("settle"));
                    tracing::info!(prepaid_until, verify = %verify.unwrap_or(&serde_json::Value::Null), settle = %settle.unwrap_or(&serde_json::Value::Null), "Accepted slice");
                }
            } else {
                tracing::debug!(env = %val, "Unhandled envelope");
            }
        }
    }

    Ok(())
}


