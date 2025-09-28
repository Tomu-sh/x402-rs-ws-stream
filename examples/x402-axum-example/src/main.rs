use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use dotenvy::dotenv;
use opentelemetry::trace::Status;
use std::env;
use tower_http::trace::TraceLayer;
use tracing::instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use x402_axum::{IntoPriceTag, X402Middleware};
use x402_rs::network::{Network, USDCDeployment};
use x402_rs::telemetry::Telemetry;
use x402_rs::types::EvmAddress;

#[tokio::main]
async fn main() {
    dotenv().ok();

    let _telemetry = Telemetry::new()
        .with_name(env!("CARGO_PKG_NAME"))
        .with_version(env!("CARGO_PKG_VERSION"))
        .register();

    let facilitator_url =
        env::var("FACILITATOR_URL").unwrap_or_else(|_| "https://facilitator.x402.rs".to_string());
    let base_url =
        env::var("BASE_URL").unwrap_or_else(|_| "https://localhost:3000/".to_string());

    let x402 = X402Middleware::try_from(facilitator_url)
        .unwrap()
        .with_base_url(url::Url::parse(&base_url).unwrap());

    // Read EVM recipient address from environment (Polygon Amoy)
    let pay_to_polygon_amoy: EvmAddress = env::var("PAY_TO_EVM_POLYGON_AMOY")
        .unwrap_or_else(|_| "0xBAc675C310721717Cd4A37F6cbeA1F081b1C2a07".to_string())
        .parse()
        .expect("Invalid EVM address for PAY_TO_EVM_POLYGON_AMOY");

    // Read human-readable USDC amount (e.g., "0.0025") for Polygon Amoy
    let amount_polygon_amoy =
        env::var("PRICE_USDC_POLYGON_AMOY").unwrap_or_else(|_| "0.0025".to_string());

    // Build price tag for Polygon Amoy
    let price_polygon_amoy = USDCDeployment::by_network(Network::PolygonAmoy)
        .pay_to(pay_to_polygon_amoy)
        .amount(amount_polygon_amoy.as_str())
        .build()
        .unwrap();

    let app = Router::new()
        .route(
            "/protected-route",
            get(my_handler).layer(
                x402.with_description("Premium API")
                    .with_mime_type("application/json")
                    .with_price_tag(price_polygon_amoy),
            ),
        )
        .layer(
            // Usual HTTP tracing
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    tracing::info_span!(
                        "http_request",
                        otel.kind = "server",
                        otel.name = %format!("{} {}", request.method(), request.uri()),
                        method = %request.method(),
                        uri = %request.uri(),
                        version = ?request.version(),
                    )
                })
                .on_response(
                    |response: &axum::http::Response<_>,
                     latency: std::time::Duration,
                     span: &tracing::Span| {
                        span.record("status", tracing::field::display(response.status()));
                        span.record("latency", tracing::field::display(latency.as_millis()));
                        span.record(
                            "http.status_code",
                            tracing::field::display(response.status().as_u16()),
                        );

                        // OpenTelemetry span status
                        if response.status().is_success()
                            || response.status() == StatusCode::PAYMENT_REQUIRED
                        {
                            span.set_status(Status::Ok);
                        } else {
                            span.set_status(Status::error(
                                response
                                    .status()
                                    .canonical_reason()
                                    .unwrap_or("unknown")
                                    .to_string(),
                            ));
                        }

                        tracing::info!(
                            "status={} elapsed={}ms",
                            response.status().as_u16(),
                            latency.as_millis()
                        );
                    },
                ),
        );

    tracing::info!("Using facilitator on {}", x402.facilitator_url());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("Can not start server");
    tracing::info!("Listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

#[instrument(skip_all)]
async fn my_handler() -> impl IntoResponse {
    (StatusCode::OK, "This is a VIP content!")
}
