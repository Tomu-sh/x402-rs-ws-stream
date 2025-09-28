//! HTTP endpoints implemented by the x402 **facilitator**.
//!
//! These are the server-side handlers for processing client-submitted x402 payments.
//! They include both protocol-critical endpoints (`/verify`, `/settle`) and discovery endpoints (`/supported`, etc).
//!
//! All payloads follow the types defined in the `x402-rs` crate, and are compatible
//! with the TypeScript and Go client SDKs.
//!
//! Each endpoint consumes or produces structured JSON payloads defined in `x402-rs`,
//! and is compatible with official x402 client SDKs.

use axum::http::StatusCode;
use axum::response::Response;
use axum::extract::WebSocketUpgrade;
use axum::{Extension, Json, response::IntoResponse};
use axum::extract::ws::{Message, WebSocket};
use futures_util::StreamExt;
use serde_json::json;
use tracing::instrument;

use crate::chain::FacilitatorLocalError;
use crate::facilitator::Facilitator;
use crate::facilitator_local::FacilitatorLocal;
use crate::types::{
    ErrorResponse, FacilitatorErrorReason, MixedAddress, SettleRequest, VerifyRequest,
    VerifyResponse,
};

/// `GET /verify`: Returns a machine-readable description of the `/verify` endpoint.
///
/// This is served by the facilitator to help clients understand how to construct
/// a valid [`VerifyRequest`] for payment verification.
///
/// This is optional metadata and primarily useful for discoverability and debugging tools.
#[instrument(skip_all)]
pub async fn get_verify_info() -> impl IntoResponse {
    Json(json!({
        "endpoint": "/verify",
        "description": "POST to verify x402 payments",
        "body": {
            "paymentPayload": "PaymentPayload",
            "paymentRequirements": "PaymentRequirements",
        }
    }))
}

/// `GET /settle`: Returns a machine-readable description of the `/settle` endpoint.
///
/// This is served by the facilitator to describe the structure of a valid
/// [`SettleRequest`] used to initiate on-chain payment settlement.
#[instrument(skip_all)]
pub async fn get_settle_info() -> impl IntoResponse {
    Json(json!({
        "endpoint": "/settle",
        "description": "POST to settle x402 payments",
        "body": {
            "paymentPayload": "PaymentPayload",
            "paymentRequirements": "PaymentRequirements",
        }
    }))
}

/// `GET /supported`: Lists the x402 payment schemes and networks supported by this facilitator.
///
/// Facilitators may expose this to help clients dynamically configure their payment requests
/// based on available network and scheme support.
#[instrument(skip_all)]
pub async fn get_supported(
    Extension(facilitator): Extension<FacilitatorLocal>,
) -> impl IntoResponse {
    let kinds = facilitator.kinds();
    (
        StatusCode::OK,
        Json(json!({
            "kinds": kinds,
        })),
    )
}

/// `POST /verify`: Facilitator-side verification of a proposed x402 payment.
///
/// This endpoint checks whether a given payment payload satisfies the declared
/// [`PaymentRequirements`], including signature validity, scheme match, and fund sufficiency.
///
/// Responds with a [`VerifyResponse`] indicating whether the payment can be accepted.
#[instrument(skip_all)]
pub async fn post_verify(
    Extension(facilitator): Extension<FacilitatorLocal>,
    Json(body): Json<VerifyRequest>,
) -> impl IntoResponse {
    match facilitator.verify(&body).await {
        Ok(valid_response) => (StatusCode::OK, Json(valid_response)).into_response(),
        Err(error) => {
            tracing::warn!(
                error = ?error,
                body = %serde_json::to_string(&body).unwrap_or_else(|_| "<can-not-serialize>".to_string()),
                "Verification failed"
            );
            error.into_response()
        }
    }
}

/// `POST /settle`: Facilitator-side execution of a valid x402 payment on-chain.
///
/// Given a valid [`SettleRequest`], this endpoint attempts to execute the payment
/// via ERC-3009 `transferWithAuthorization`, and returns a [`SettleResponse`] with transaction details.
///
/// This endpoint is typically called after a successful `/verify` step.
#[instrument(skip_all)]
pub async fn post_settle(
    Extension(facilitator): Extension<FacilitatorLocal>,
    Json(body): Json<SettleRequest>,
) -> impl IntoResponse {
    match facilitator.settle(&body).await {
        Ok(valid_response) => (StatusCode::OK, Json(valid_response)).into_response(),
        Err(error) => {
            tracing::warn!(
                error = ?error,
                body = %serde_json::to_string(&body).unwrap_or_else(|_| "<can-not-serialize>".to_string()),
                "Settlement failed"
            );
            error.into_response()
        }
    }
}

/// `GET /ws`: WebSocket endpoint that mirrors facilitator methods per x402-ws-stream.
#[instrument(skip_all)]
pub async fn ws_handler(
    Extension(facilitator): Extension<FacilitatorLocal>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_serve(socket, facilitator))
}

#[derive(Debug, serde::Deserialize)]
struct WsEnvelopeReq {
    id: serde_json::Value,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(serde::Serialize)]
struct WsEnvelopeOk<'a, T: serde::Serialize> {
    id: &'a serde_json::Value,
    result: T,
}

#[derive(serde::Serialize)]
struct WsEnvelopeErr<'a> {
    id: &'a serde_json::Value,
    error: WsErrorBody,
}

#[derive(serde::Serialize)]
struct WsErrorBody {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

async fn ws_serve(mut socket: WebSocket, facilitator: FacilitatorLocal) {
    while let Some(Ok(msg)) = socket.next().await {
        match msg {
            Message::Text(text) => {
                let response = handle_ws_text(&text, &facilitator).await;
                if let Some(resp_text) = response {
                    // Best-effort send; if it fails, break the loop
                    if socket.send(Message::Text(resp_text.into())).await.is_err() {
                        break;
                    }
                }
            }
            Message::Binary(bin) => {
                let text = String::from_utf8_lossy(&bin);
                let response = handle_ws_text(&text, &facilitator).await;
                if let Some(resp_text) = response {
                    if socket.send(Message::Text(resp_text.into())).await.is_err() {
                        break;
                    }
                }
            }
            Message::Ping(p) => {
                let _ = socket.send(Message::Pong(p)).await;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

async fn handle_ws_text(text: &str, facilitator: &FacilitatorLocal) -> Option<String> {
    let req: WsEnvelopeReq = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            // Cannot parse envelope; no id to respond to
            tracing::warn!(error = %e, "Invalid WS JSON envelope");
            return None;
        }
    };

    let method = req.method.as_str();
    match method {
        "x402.supported" => {
            let kinds = facilitator.kinds();
            let result = serde_json::json!({ "kinds": kinds });
            Some(serde_json::to_string(&WsEnvelopeOk { id: &req.id, result }).unwrap())
        }
        "x402.verify" => {
            let parsed: Result<VerifyRequest, _> = serde_json::from_value(req.params.clone());
            match parsed {
                Ok(body) => match facilitator.verify(&body).await {
                    Ok(valid_response) => Some(
                        serde_json::to_string(&WsEnvelopeOk { id: &req.id, result: valid_response }).unwrap(),
                    ),
                    Err(error) => Some(serde_json::to_string(&WsEnvelopeOk {
                        id: &req.id,
                        result: map_error_to_verify_response(error),
                    })
                    .unwrap()),
                },
                Err(e) => Some(serde_json::to_string(&WsEnvelopeErr {
                    id: &req.id,
                    error: WsErrorBody { code: -32602, message: format!("Invalid params: {}", e), data: None },
                }).unwrap()),
            }
        }
        "x402.settle" => {
            let parsed: Result<SettleRequest, _> = serde_json::from_value(req.params.clone());
            match parsed {
                Ok(body) => match facilitator.settle(&body).await {
                    Ok(settle_response) => Some(
                        serde_json::to_string(&WsEnvelopeOk { id: &req.id, result: settle_response }).unwrap(),
                    ),
                    Err(error) => {
                        // Map to VerifyResponse InvalidScheme if settle failed due to protocol reasons
                        let mapped = map_error_to_verify_response(error);
                        let data = serde_json::to_value(&mapped).ok();
                        Some(serde_json::to_string(&WsEnvelopeErr {
                            id: &req.id,
                            error: WsErrorBody { code: 1001, message: "Settlement failed".to_string(), data },
                        }).unwrap())
                    }
                },
                Err(e) => Some(serde_json::to_string(&WsEnvelopeErr {
                    id: &req.id,
                    error: WsErrorBody { code: -32602, message: format!("Invalid params: {}", e), data: None },
                }).unwrap()),
            }
        }
        _ => Some(serde_json::to_string(&WsEnvelopeErr {
            id: &req.id,
            error: WsErrorBody { code: -32601, message: "Method not found".to_string(), data: None },
        }).unwrap()),
    }
}

fn map_error_to_verify_response(error: FacilitatorLocalError) -> VerifyResponse {
    match error {
        FacilitatorLocalError::SchemeMismatch(payer, ..) => VerifyResponse::invalid(payer, FacilitatorErrorReason::InvalidScheme),
        FacilitatorLocalError::ReceiverMismatch(payer, ..)
        | FacilitatorLocalError::InvalidSignature(payer, ..)
        | FacilitatorLocalError::InvalidTiming(payer, ..)
        | FacilitatorLocalError::InsufficientValue(payer) => VerifyResponse::invalid(Some(payer), FacilitatorErrorReason::InvalidScheme),
        FacilitatorLocalError::NetworkMismatch(payer, ..)
        | FacilitatorLocalError::UnsupportedNetwork(payer) => VerifyResponse::invalid(payer, FacilitatorErrorReason::InvalidNetwork),
        FacilitatorLocalError::ContractCall(..)
        | FacilitatorLocalError::InvalidAddress(..)
        | FacilitatorLocalError::DecodingError(..)
        | FacilitatorLocalError::ClockError(_) => VerifyResponse::invalid(None, FacilitatorErrorReason::UnexpectedSettleError),
        FacilitatorLocalError::InsufficientFunds(payer) => VerifyResponse::invalid(Some(payer), FacilitatorErrorReason::InsufficientFunds),
    }
}

fn invalid_schema(payer: Option<MixedAddress>) -> VerifyResponse {
    VerifyResponse::invalid(payer, FacilitatorErrorReason::InvalidScheme)
}

impl IntoResponse for FacilitatorLocalError {
    fn into_response(self) -> Response {
        let error = self;

        let bad_request = (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid request".to_string(),
            }),
        )
            .into_response();

        match error {
            FacilitatorLocalError::SchemeMismatch(payer, ..) => {
                (StatusCode::OK, Json(invalid_schema(payer))).into_response()
            }
            FacilitatorLocalError::ReceiverMismatch(payer, ..)
            | FacilitatorLocalError::InvalidSignature(payer, ..)
            | FacilitatorLocalError::InvalidTiming(payer, ..)
            | FacilitatorLocalError::InsufficientValue(payer) => {
                (StatusCode::OK, Json(invalid_schema(Some(payer)))).into_response()
            }
            FacilitatorLocalError::NetworkMismatch(payer, ..)
            | FacilitatorLocalError::UnsupportedNetwork(payer) => (
                StatusCode::OK,
                Json(VerifyResponse::invalid(
                    payer,
                    FacilitatorErrorReason::InvalidNetwork,
                )),
            )
                .into_response(),
            FacilitatorLocalError::ContractCall(..)
            | FacilitatorLocalError::InvalidAddress(..)
            | FacilitatorLocalError::DecodingError(..)
            | FacilitatorLocalError::ClockError(_) => bad_request,
            FacilitatorLocalError::InsufficientFunds(payer) => (
                StatusCode::OK,
                Json(VerifyResponse::invalid(
                    Some(payer),
                    FacilitatorErrorReason::InsufficientFunds,
                )),
            )
                .into_response(),
        }
    }
}
