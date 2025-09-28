# x402-rs

[![Crates.io](https://img.shields.io/crates/v/x402-rs.svg)](https://crates.io/crates/x402-rs)
[![Docs.rs](https://docs.rs/x402-rs/badge.svg)](https://docs.rs/x402-rs)
[![Docker Pulls](https://img.shields.io/docker/pulls/ukstv/x402-facilitator.svg)](https://hub.docker.com/r/ukstv/x402-facilitator)
[![GHCR](https://img.shields.io/badge/ghcr.io-x402--facilitator-blue)](https://github.com/orgs/x402-rs/packages)

> A Rust-based implementation of the x402 protocol.

This repository provides:

- `x402-rs` (current crate):
  - Core protocol types, facilitator traits, and logic for on-chain payment verification and settlement
  - Facilitator binary - production-grade HTTP server to verify and settle x402 payments
- [`x402-axum`](./crates/x402-axum) - Axum middleware for accepting x402 payments,
- [`x402-reqwest`](./crates/x402-reqwest) - Wrapper for reqwest for transparent x402 payments,
- [`x402-axum-example`](./examples/x402-axum-example) - an example of `x402-axum` usage.
- [`x402-reqwest-example`](./examples/x402-reqwest-example) - an example of `x402-reqwest` usage.
 - [`x402-ws-example`](./examples/x402-ws-example) - a Buyer/Seller demo of the WS streaming draft.

## About x402

The [x402 protocol](https://docs.cdp.coinbase.com/x402/docs/overview) is a proposed standard for making blockchain payments directly through HTTP using native `402 Payment Required` status code.

Servers declare payment requirements for specific routes. Clients send cryptographically signed payment payloads. Facilitators verify and settle payments on-chain.

## Getting Started

### Run facilitator

```shell
docker run --env-file .env -p 8080:8080 ukstv/x402-facilitator
```

Or build locally:
```shell
docker build -t x402-rs .
docker run --env-file .env -p 8080:8080 x402-rs
```

See the [Facilitator](#facilitator) section below for full usage details

### Protect Axum Routes

Use `x402-axum` to gate your routes behind on-chain payments:

```rust
let x402 = X402Middleware::try_from("https://x402.org/facilitator/").unwrap();
let usdc = USDCDeployment::by_network(Network::BaseSepolia);

let app = Router::new().route("/paid-content", get(handler).layer( 
        x402.with_price_tag(usdc.amount("0.025").pay_to("0xYourAddress").unwrap())
    ),
);
```

See [`x402-axum` crate docs](./crates/x402-axum/README.md).

### Send x402 payments

Use `x402-reqwest` to send payments:

```rust
let signer: PrivateKeySigner = "0x...".parse()?; // never hardcode real keys!

let client = reqwest::Client::new()
    .with_payments(signer)
    .prefer(USDCDeployment::by_network(Network::Base))
    .max(USDCDeployment::by_network(Network::Base).amount("1.00")?)
    .build();

let res = client
    .get("https://example.com/protected")
    .send()
    .await?;
```

See [`x402-reqwest` crate docs](./crates/x402-reqwest/README.md).

## Roadmap

| Milestone                           | Description                                                                                              |   Status   |
|:------------------------------------|:---------------------------------------------------------------------------------------------------------|:----------:|
| Facilitator for Base USDC           | Payment verification and settlement service, enabling real-time pay-per-use transactions for Base chain. | ✅ Complete |
| Metrics and Tracing                 | Expose OpenTelemetry metrics and structured tracing for observability, monitoring, and debugging         | ✅ Complete |
| Server Middleware                   | Provide ready-to-use integration for Rust web frameworks such as axum and tower.                         | ✅ Complete |
| Client Library                      | Provide a lightweight Rust library for initiating and managing x402 payment flows from Rust clients.     | ✅ Complete |
| Solana Support                      | Support Solana chain.                                                                                    | ✅ Complete |
| Multiple chains and multiple tokens | Support various tokens and EVM compatible chains.                                                        | ⏳ Planned  |
| Payment Storage                     | Persist verified and settled payments for analytics, access control, and auditability.                   | 🔜 Planned |
| Micropayment Support                | Enable fine-grained offchain usage-based payments, including streaming and per-request billing.          | 🔜 Planned |

The initial focus is on establishing a stable, production-quality Rust SDK and middleware ecosystem for x402 integration.

## WebSocket Streaming (Draft)

This repository ships an experimental WebSocket profile for streaming payments inspired by the proposal in `x402-ws-stream.md`. It enables time-sliced prepay over a WS control channel while content is streamed on the same or sibling WS.

Status: Draft. EVM-only. Breaking changes possible.

What is implemented:

- Facilitator WS endpoint at `GET /ws` mirroring core HTTP methods:
  - `x402.supported` → lists supported kinds
  - `x402.verify` → verify `VerifyRequest`
  - `x402.settle` → settle `SettleRequest`
- Example Seller WS server that:
  - Accepts `stream.init` and responds with `stream.accept` containing `pricePerUnit`, `unitSeconds`, `payTo`, `asset`, `network`, `streamId`
  - Issues `stream.require` per slice with `PaymentRequirements`
  - On `stream.pay`, calls the Facilitator over WS (`x402.verify` then optional `x402.settle`) and replies with `stream.accept { verify, settle?, prepaidUntilMs }`
- Example Buyer that:
  - Sends `stream.init`
  - On `stream.require`, builds `PaymentPayload` via `x402-reqwest` signer and sends `stream.pay`

Spec reference: see [`x402-ws-stream.md`](./x402-ws-stream.md).

### Running the WS Demo

Prerequisites:

- Rust toolchain
- A Facilitator running with WS enabled (the provided binary exposes `GET /ws` by default)
- EVM private key with test funds if you enable on-chain settlement

1) Start the Facilitator (HTTP + WS):

```bash
docker run --env-file .env -p 8080:8080 ukstv/x402-facilitator
```

Ensure your `.env` provides the necessary `RPC_URL_*` and signer variables (see Facilitator section). The WS endpoint will be at `ws://localhost:8080/ws` unless configured otherwise.

2) Run the Seller WS example (streams and requests prepayments):

Environment variables:

- `HOST` (default `0.0.0.0`)
- `PORT` (default `4000`)
- `FACILITATOR_WS_URL` (default `ws://localhost:8080/ws`)
- `STREAM_NETWORK` (default `base-sepolia`)
- `STREAM_UNIT_SECONDS` (default `60`)
- `STREAM_PRICE_USDC` (default `0.05`)
- `STREAM_PAY_TO` (receiver address)

Run:

```bash
cargo run -p x402-ws-example --bin seller
```

This exposes a WS endpoint for buyers at `ws://localhost:4000/ws`.

3) Run the Buyer WS example (pays per-slice):

Environment variables:

- `SELLER_WS_URL` (default `ws://localhost:4000/ws`)
- `EVM_PRIVATE_KEY` (hex string for signing EIP-3009 payloads)

Run:

```bash
cargo run -p x402-ws-example --bin buyer
```

On receiving `stream.require`, the buyer signs an EIP‑3009 payload using `x402-reqwest` utilities and responds with `stream.pay`.

Notes:

- The demo operates in the “on-chain per slice” mode by default when `verifyOnly=false`. Tune `STREAM_UNIT_SECONDS` to balance latency and on-chain frequency.
- Only the WS control-plane is demonstrated here. Streaming the actual content can reuse the same WS connection or a sibling one.
- The envelope format used is `{ id, method, params }` and `{ id, result }` with `result.method = "stream.accept"` in Seller responses.

## Facilitator

The `x402-rs` crate (this repo) provides a runnable x402 facilitator binary. The _Facilitator_ role simplifies adoption of x402 by handling:
- **Payment verification**: Confirming that client-submitted payment payloads match the declared requirements.
- **Payment settlement**: Submitting validated payments to the blockchain and monitoring their confirmation.

By using a Facilitator, servers (sellers) do not need to:
- Connect directly to a blockchain.
- Implement complex cryptographic or blockchain-specific payment logic.

Instead, they can rely on the Facilitator to perform verification and settlement, reducing operational overhead and accelerating x402 adoption.
The Facilitator **never holds user funds**. It acts solely as a stateless verification and execution layer for signed payment payloads.

For a detailed overview of the x402 payment flow and Facilitator role, see the [x402 protocol documentation](https://docs.cdp.coinbase.com/x402/docs/overview).

### Usage

#### 1. Provide environment variables

Create a `.env` file or set environment variables directly. Example `.env`:

```dotenv
HOST=0.0.0.0
PORT=8080
RPC_URL_BASE_SEPOLIA=https://sepolia.base.org
RPC_URL_BASE=https://mainnet.base.org
SIGNER_TYPE=private-key
EVM_PRIVATE_KEY=0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef
SOLANA_PRIVATE_KEY=6ASf5EcmmEHTgDJ4X4ZT5vT6iHVJBXPg5AN5YoTCpGWt
RUST_LOG=info
```

**Important:**
The supported networks are determined by which RPC URLs you provide:
- If you set only `RPC_URL_BASE_SEPOLIA`, then only Base Sepolia network is supported.
- If you set both `RPC_URL_BASE_SEPOLIA` and `RPC_URL_BASE`, then both Base Sepolia and Base Mainnet are supported.
- If an RPC URL for a network is missing, that network will not be available for settlement or verification.

#### 2. Build and Run with Docker

Prebuilt Docker images are available at:
- [GitHub Container Registry](https://ghcr.io/x402-rs/x402-facilitator): `ghcr.io/x402-rs/x402-facilitator`
- [Docker Hub](https://hub.docker.com/r/ukstv/x402-facilitator): `ukstv/x402-facilitator`

Run the container from Docker Hub:
```shell
docker run --env-file .env -p 8080:8080 ukstv/x402-facilitator
```

To run using GitHub Container Registry:
```shell
docker run --env-file .env -p 8080:8080 ghcr.io/x402-rs/x402-facilitator
```

Or build a Docker image locally:
```shell
docker build -t x402-rs .
docker run --env-file .env -p 8080:8080 x402-rs
```

The container:
* Exposes port `8080` (or a port you configure with `PORT` environment variable).
* Starts on http://localhost:8080 by default.
* Requires minimal runtime dependencies (based on `debian:bullseye-slim`).

#### 3. Point your application to your Facilitator

If you are building an x402-powered application, update the Facilitator URL to point to your self-hosted instance.

> ℹ️ **Tip:** For production deployments, ensure your Facilitator is reachable via HTTPS and protect it against public abuse.

<details>
<summary>If you use Hono and x402-hono</summary>
From [x402.org Quickstart for Sellers](https://x402.gitbook.io/x402/getting-started/quickstart-for-sellers):

```typescript
import { Hono } from "hono";
import { serve } from "@hono/node-server";
import { paymentMiddleware } from "x402-hono";

const app = new Hono();

// Configure the payment middleware
app.use(paymentMiddleware(
  "0xYourAddress", // Your receiving wallet address
  {
    "/protected-route": {
      price: "$0.10",
      network: "base-sepolia",
      config: {
        description: "Access to premium content",
      }
    }
  },
  {
    url: "http://your-validator.url/", // 👈 Your self-hosted Facilitator
  }
));

// Implement your protected route
app.get("/protected-route", (c) => {
  return c.json({ message: "This content is behind a paywall" });
});

serve({
  fetch: app.fetch,
  port: 3000
});
```

</details>

<details>
<summary>If you use `x402-axum`</summary>

```rust
let x402 = X402Middleware::try_from("http://your-validator.url/").unwrap();  // 👈 Your self-hosted Facilitator
let usdc = USDCDeployment::by_network(Network::BaseSepolia);

let app = Router::new().route("/paid-content", get(handler).layer( 
        x402.with_price_tag(usdc.amount("0.025").pay_to("0xYourAddress").unwrap())
    ),
);
```

</details>

### Configuration

The service reads configuration via `.env` file or directly through environment variables.

Available variables:

* `RUST_LOG`: Logging level (e.g., `info`, `debug`, `trace`),
* `HOST`: HTTP host to bind to (default: `0.0.0.0`),
* `PORT`: HTTP server port (default: `8080`),
* `SIGNER_TYPE` (required): Type of signer to use. Only `private-key` is supported now,
* `EVM_PRIVATE_KEY` (required): Private key in hex for EVM networks, like `0xdeadbeef...`,
* `SOLANA_PRIVATE_KEY` (required): Private key in hex for Solana networks, like `0xdeadbeef...`,
* `RPC_URL_BASE_SEPOLIA`: Ethereum RPC endpoint for Base Sepolia testnet,
* `RPC_URL_BASE`: Ethereum RPC endpoint for Base mainnet,
* `RPC_URL_AVALANCHE_FUJI`: Ethereum RPC endpoint for Avalanche Fuji testnet,
* `RPC_URL_AVALANCHE`: Ethereum RPC endpoint for Avalanche C-Chain mainnet.
* `RPC_URL_SOLANA`: RPC endpoint for Solana mainnet.
* `RPC_URL_SOLANA_DEVNET`: RPC endpoint for Solana devnet.
* `RPC_URL_POLYGON`: RPC endpoint for Polygon mainnet.
* `RPC_URL_POLYGON_AMOY`: RPC endpoint for Polygon Amoy testnet.
* `RPC_WS_URL_POLYGON_AMOY`: WebSocket RPC endpoint for Polygon Amoy. If set (non-empty), the facilitator will prefer WebSocket transport for Polygon Amoy; otherwise it will fall back to `RPC_URL_POLYGON_AMOY` (HTTP).
* `RPC_URL_SEI`: RPC endpoint for Sei mainnet.
* `RPC_URL_SEI_TESTNET`: RPC endpoint for Sei testnet.


### Observability

The facilitator emits [OpenTelemetry](https://opentelemetry.io)-compatible traces and metrics to standard endpoints,
making it easy to integrate with tools like Honeycomb, Prometheus, Grafana, and others.
Tracing spans are annotated with HTTP method, status code, URI, latency, other request and process metadata.

To enable tracing and metrics export, set the appropriate `OTEL_` environment variables:

```dotenv
# For Honeycomb, for example:
# Endpoint URL for sending OpenTelemetry traces and metrics
OTEL_EXPORTER_OTLP_ENDPOINT=https://api.honeycomb.io:443
# Comma-separated list of key=value pairs to add as headers
OTEL_EXPORTER_OTLP_HEADERS=x-honeycomb-team=your_api_key,x-honeycomb-dataset=x402-rs
# Export protocol to use for telemetry. Supported values: `http/protobuf` (default), `grpc`
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
```

The service automatically detects and initializes exporters if `OTEL_EXPORTER_OTLP_*` variables are provided.

### Supported Networks

The Facilitator supports different networks based on the environment variables you configure:

| Network                   | Environment Variable     | Supported if Set | Notes                            |
|:--------------------------|:-------------------------|:-----------------|:---------------------------------|
| Base Sepolia Testnet      | `RPC_URL_BASE_SEPOLIA`   | ✅                | Testnet, Recommended for testing |
| Base Mainnet              | `RPC_URL_BASE`           | ✅                | Mainnet                          |
| XDC Mainnet               | `RPC_URL_XDC`            | ✅                | Mainnet                          |
| Avalanche Fuji Testnet    | `RPC_URL_AVALANCHE_FUJI` | ✅                | Testnet                          |
| Avalanche C-Chain Mainnet | `RPC_URL_AVALANCHE`      | ✅                | Mainnet                          |
| Polygon Amoy Testnet      | `RPC_URL_POLYGON_AMOY` or `RPC_WS_URL_POLYGON_AMOY` | ✅ | Testnet. WS is preferred if `RPC_WS_URL_POLYGON_AMOY` is set |
| Polygon Mainnet           | `RPC_URL_POLYGON`        | ✅                | Mainnet                          |
| Sei Testnet               | `RPC_URL_SEI_TESTNET`    | ✅                | Testnet                          |
| Sei Mainnet               | `RPC_URL_SEI`            | ✅                | Mainnet                          |
| Solana Mainnet            | `RPC_URL_SOLANA`         | ✅                | Mainnet                          |
| Solana Devnet             | `RPC_URL_SOLANA_DEVNET`  | ✅                | Testnet, Recommended for testing |

- If you provide say only `RPC_URL_BASE_SEPOLIA`, only **Base Sepolia** will be available.
- If you provide `RPC_URL_BASE_SEPOLIA`, `RPC_URL_BASE`, and other env variables on the list, then all the specified networks will be supported.

> ℹ️ **Tip:** For initial development and testing, you can start with Base Sepolia only.

### Development

Prerequisites:
- Rust 1.80+
- `cargo` and a working toolchain

Build locally:
```shell
cargo build
```
Run:
```shell
cargo run
```

## Related Resources

* [x402 Protocol Documentation](https://x402.org)
* [x402 Overview by Coinbase](https://docs.cdp.coinbase.com/x402/docs/overview)
* [Facilitator Documentation by Coinbase](https://docs.cdp.coinbase.com/x402/docs/facilitator)

## Contributions and feedback welcome!
Feel free to open issues or pull requests to improve x402 support in the Rust ecosystem.

## License

[Apache-2.0](LICENSE)
