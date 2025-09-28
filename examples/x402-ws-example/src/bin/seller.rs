use axum::extract::ws::{Message, WebSocket};
use axum::extract::WebSocketUpgrade;
use axum::routing::get;
use axum::{Router, Extension};
use dotenvy::dotenv;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use std::net::SocketAddr;
use tokio_tungstenite::connect_async;
use tracing::instrument;
use tracing_subscriber::EnvFilter;
use url::Url;
use uuid::Uuid;

use x402_rs::network::{Network, USDCDeployment};
use x402_rs::types::{PaymentRequirements, Scheme, VerifyRequest, X402Version};

#[derive(Clone)]
struct AppConfig {
    facilitator_ws: Url,
    network: Network,
    unit_seconds: u64,
    price_usdc: String,
    pay_to: String,
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(4000);

    let facilitator_ws = env::var("FACILITATOR_WS_URL")
        .unwrap_or_else(|_| "ws://localhost:8080/ws".into());
    let facilitator_ws = Url::parse(&facilitator_ws).expect("FACILITATOR_WS_URL invalid");

    let network = env::var("STREAM_NETWORK")
        .ok()
        .and_then(|s| serde_json::from_str::<Network>(&format!("\"{}\"", s)).ok())
        .unwrap_or(Network::BaseSepolia);

    let unit_seconds: u64 = env::var("STREAM_UNIT_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    let price_usdc = env::var("STREAM_PRICE_USDC").unwrap_or_else(|_| "0.05".into());
    let pay_to = env::var("STREAM_PAY_TO")
        .unwrap_or_else(|_| "0xBAc675C310721717Cd4A37F6cbeA1F081b1C2a07".into());

    let config = AppConfig {
        facilitator_ws,
        network,
        unit_seconds,
        price_usdc,
        pay_to,
    };

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .layer(Extension(config));

    let addr: SocketAddr = format!("{}:{}", host, port).parse().unwrap();
    tracing::info!(%addr, "WS Seller listening");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct EnvelopeReq {
    id: serde_json::Value,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct EnvelopeOk<T> {
    id: serde_json::Value,
    result: T,
}

#[instrument(skip_all)]
async fn ws_handler(
    Extension(config): Extension<AppConfig>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| ws_serve(socket, config))
}

async fn ws_serve(mut socket: WebSocket, config: AppConfig) {
    let mut slice_index: u64 = 0;
    // Wait for stream.init from buyer
    while let Some(Ok(msg)) = socket.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(req) = serde_json::from_str::<EnvelopeReq>(&text) {
                    match req.method.as_str() {
                        "stream.init" => {
                            // Choose USDC on configured network
                            let usdc = USDCDeployment::by_network(config.network);
                            let stream_id = Uuid::new_v4().to_string();
                            let accept = json!({
                                "pricePerUnit": config.price_usdc,
                                "unitSeconds": config.unit_seconds,
                                "payTo": config.pay_to,
                                "asset": usdc.address(),
                                "network": config.network,
                                "streamId": stream_id
                            });
                            let response = json!({
                                "id": req.id,
                                "result": { "method": "stream.accept", "params": accept }
                            });
                            let _ = socket.send(Message::Text(response.to_string())).await;

                            // Immediately request first slice
                            let require = build_requirements(&config, &stream_id, slice_index, usdc);
                            let env = json!({
                                "id": Uuid::new_v4().to_string(),
                                "method": "stream.require",
                                "params": require,
                            });
                            let _ = socket.send(Message::Text(env.to_string())).await;
                        }
                        "stream.pay" => {
                            // Forward to facilitator WS for verify (+ optional settle)
                            let verify_only = req
                                .params
                                .get("verifyOnly")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            match facilitator_verify_and_maybe_settle(&config, &req.params, !verify_only).await {
                                Ok((verify, settle)) => {
                                    // Extend prepaid window by one unit
                                    slice_index += 1;
                                    let prepaid_until_ms = chrono::Utc::now().timestamp_millis()
                                        + (config.unit_seconds as i64) * 1000;
                                    let result = json!({
                                        "verify": verify,
                                        "settle": settle,
                                        "prepaidUntilMs": prepaid_until_ms,
                                    });
                                    let env = json!({
                                        "id": req.id,
                                        "result": { "method": "stream.accept", "params": result }
                                    });
                                    let _ = socket.send(Message::Text(env.to_string())).await;

                                    // Issue next require a bit before end
                                    let next_require = build_requirements(&config,
                                        req.params.get("streamId").and_then(|v| v.as_str()).unwrap_or("unknown"),
                                        slice_index,
                                        USDCDeployment::by_network(config.network),
                                    );
                                    let env2 = json!({
                                        "id": Uuid::new_v4().to_string(),
                                        "method": "stream.require",
                                        "params": next_require,
                                    });
                                    let _ = socket.send(Message::Text(env2.to_string())).await;
                                }
                                Err(e) => {
                                    let env = json!({
                                        "id": req.id,
                                        "error": { "code": 1001, "message": format!("{}", e) }
                                    });
                                    let _ = socket.send(Message::Text(env.to_string())).await;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

fn build_requirements(
    config: &AppConfig,
    stream_id: &str,
    slice_index: u64,
    usdc: &USDCDeployment,
) -> serde_json::Value {
    // PaymentRequirements for one slice
    let requirements = PaymentRequirements {
        scheme: Scheme::Exact,
        network: config.network,
        max_amount_required: usdc.amount(config.price_usdc.as_str()).expect("valid amount"),
        resource: Url::parse("wss://example/stream").unwrap(),
        description: format!("Slice {}", slice_index),
        mime_type: "application/octet-stream".into(),
        output_schema: None,
        pay_to: config.pay_to.parse().expect("valid pay_to"),
        max_timeout_seconds: config.unit_seconds + 30,
        asset: usdc.address(),
        extra: usdc.eip712.as_ref().map(|meta| json!({ "name": meta.name, "version": meta.version })),
    };
    json!({
        "streamId": stream_id,
        "sliceIndex": slice_index,
        "expiresAt": chrono::Utc::now().timestamp() + (config.unit_seconds as i64) + 10,
        "requirements": requirements,
    })
}

async fn facilitator_verify_and_maybe_settle(
    config: &AppConfig,
    params: &serde_json::Value,
    do_settle: bool,
) -> anyhow::Result<(serde_json::Value, Option<serde_json::Value>)> {
    // Extract paymentPayload + requirements from Buyer params
    let payment_payload = params.get("paymentPayload").cloned().ok_or_else(|| anyhow::anyhow!("missing paymentPayload"))?;
    let payment_requirements = params
        .get("paymentPayload")
        .and_then(|_| params.get("sliceIndex"))
        .and_then(|_| params.get("requirements"))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing requirements"))?;
    let verify_req = VerifyRequest {
        x402_version: X402Version::V1,
        payment_payload: serde_json::from_value(payment_payload.clone())?,
        payment_requirements: serde_json::from_value(payment_requirements.clone())?,
    };

    let (mut ws, _) = connect_async(config.facilitator_ws.as_str()).await?;

    let id_verify = Uuid::new_v4();
    let env = json!({
        "id": id_verify,
        "method": "x402.verify",
        "params": verify_req,
    });
    ws.send(tokio_tungstenite::tungstenite::Message::Text(env.to_string())).await?;
    let verify = recv_result(&mut ws, &id_verify.to_string()).await?;

    let settle = if do_settle {
        let id_settle = Uuid::new_v4();
        let env2 = json!({
            "id": id_settle,
            "method": "x402.settle",
            "params": verify_req,
        });
        ws.send(tokio_tungstenite::tungstenite::Message::Text(env2.to_string())).await?;
        Some(recv_result(&mut ws, &id_settle.to_string()).await?)
    } else { None };

    Ok((verify, settle))
}

async fn recv_result(ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::ConnectorStream>, id: &str) -> anyhow::Result<serde_json::Value> {
    while let Some(msg) = ws.next().await {
        let msg = msg?;
        if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                if val.get("id").map(|v| v.to_string().trim_matches('"').to_string()) == Some(id.to_string()) {
                    if let Some(err) = val.get("error") {
                        return Err(anyhow::anyhow!("{}", err));
                    }
                    return Ok(val.get("result").cloned().unwrap_or(val));
                }
            }
        }
    }
    Err(anyhow::anyhow!("WS closed before response"))
}


