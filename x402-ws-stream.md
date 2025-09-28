## x402-ws-stream: WebSocket Streaming Payments Extension (EVM-only)

### Status
- Draft; EVM chains only.

### Motivation
- Micropay real-time streams (video, API, data) with low latency and predictable UX.
- All transports over WebSockets: content, payments, and blockchain RPC.
- No custom escrow required for MVP; can optionally add escrow or channels later without changing the app protocol.

### Roles
- Buyer: requests a metered resource and prepays in small time slices.
- Seller: gates access, verifies each prepayment, streams content while prepaid balance remains.
- Facilitator: verifies and optionally settles payments (x402 Exact scheme; EIP‑3009 on EVM).

### Transport
- Content stream: single WebSocket between Buyer and Seller.
- Payment control channel: multiplexed on the same WS or a sibling WS.
- Facilitator: WebSocket endpoint mirroring HTTP `verify`, `settle`, `supported`.
- EVM RPC: WebSocket (wss://) endpoints for all chain calls is recommended, fallback to HTTPS.

### Extension Overview
This extension defines a prepay cadence for time-sliced access. Seller requests payment for a unit (e.g., 60 seconds of stream). Buyer responds with an x402 `PaymentPayload` for that exact unit. After successful verify (and optional settle), Seller streams until the slice ends, then prompts for the next slice. If the next prepayment does not arrive within a short TTL, the Seller pauses.

### Normative Requirements
- Payments MUST use x402 `scheme = "exact"` on EVM with EIP‑3009 (USDC style) payloads.
- Each slice MUST have a fresh `nonce` and a `validAfter/validBefore` window that covers that slice plus a small grace (recommend 5–10s) but no more.
- The per-slice `value` MUST be ≥ the required amount for that slice.
- Seller MUST NOT stream beyond the prepaid horizon; on TTL expiry without next prepay, MUST pause.
- If using on-chain-per-slice settlement, Seller SHOULD settle promptly after a successful verify.
- RPC to the chain MUST be over WebSocket (wss). Facilitator transport SHOULD be WebSocket.

### Message Envelope (WS)
Frames use a simple JSON-RPC-like envelope. Methods are namespaced under `x402.*` and `stream.*`.

```json
{ "id": "uuid", "method": "string", "params": { /* method-specific */ } }
```

Errors return:

```json
{ "id": "uuid", "error": { "code": int, "message": "string", "data": { /* optional */ } } }
```

### Core Methods
- x402.supported → Facilitator lists supported kinds
- x402.verify → Facilitator verifies `VerifyRequest`
- x402.settle → Facilitator settles `SettleRequest`
- stream.init → Buyer↔Seller: negotiate stream metadata
- stream.require → Seller requests prepayment for next slice
- stream.pay → Buyer submits `PaymentPayload`
- stream.accept / stream.reject → Seller response (Verify/Settle result)
- stream.pause / stream.resume / stream.end → Seller state changes
- stream.keepalive → Heartbeat with remaining prepaid millis

### Types (reused from x402)
- PaymentPayload: EIP‑3009 signed payload (JSON, not base64 on WS).
- VerifyRequest: `{ x402Version, paymentPayload, paymentRequirements }`
- VerifyResponse: `{ isValid, payer?, invalidReason? }`
- SettleRequest: alias of `VerifyRequest`
- SettleResponse: `{ success, errorReason?, payer, transaction?, network }`
- PaymentRequirements: `{ scheme, network, maxAmountRequired, resource, description, mimeType, payTo, maxTimeoutSeconds, asset, extra }`

### Slice Accounting
- Unit: time duration in seconds (e.g., 60).
- Price: token amount per unit (e.g., 50,000 USDC base units for $0.05).
- TTL: grace before the next slice must be prepaid (recommend 30s into a 60s unit).
- Clock skew buffer: ≥ 5s inside `validBefore` checks.

### Protocol Flow
1) stream.init (Buyer→Seller)
   - Params: `resource`, `accepts` (candidate assets/prices), `network`, optional `facilitatorWs`.
   - Reply: `stream.accept` echoing chosen `pricePerUnit`, `unitSeconds`, `payTo`, `asset`, `network`, and a `streamId`.

2) stream.require (Seller→Buyer)
   - Params: `streamId`, `sliceIndex`, `requirements` (a single `PaymentRequirements` for the slice), `expiresAt`.
   - Requirements MUST set: `scheme=exact`, `payTo`, `asset`, `network`, `maxAmountRequired = pricePerUnit`, `resource = canonical URL for this stream`.

3) stream.pay (Buyer→Seller)
   - Params: `streamId`, `sliceIndex`, `paymentPayload` (JSON form), optional `verifyOnly: boolean`.
   - Seller invokes Facilitator over WS:
     - `x402.verify` with `{ paymentPayload, paymentRequirements }`.
     - If `verifyOnly=false` and on-chain-per-slice mode: call `x402.settle`.

4) stream.accept / stream.reject (Seller→Buyer)
   - On success: include `{ verify: VerifyResponse, settle?: SettleResponse, prepaidUntilMs }`.
   - On failure: include reason; Buyer may retry with a new payload.

5) stream.keepalive (Seller→Buyer)
   - Periodic heartbeat with `remainingMs`, `nextRequireAtMs`.
   - At `nextRequireAtMs`, Seller issues the next `stream.require`.

6) stream.pause / stream.resume / stream.end
   - Pause if `remainingMs` ≤ 0 and no accepted next slice.
   - Resume after a successful next prepay.
   - End on completion or by either party.

### Settlement Modes
1) On-chain per slice (trustless, no custom contracts)
   - After `x402.verify` succeeds, Seller calls `x402.settle` immediately.
   - Pros: strong guarantees; Cons: one tx per slice (tune unit size accordingly).

2) Deferred batches (lighter on-chain, partially trusted)
   - Seller verifies each slice and records usage off-chain; settles periodically.
   - Pros: cheaper; Cons: reduced trust minimization.

### Security Considerations
- Replay: Require unique `nonce` per slice; Seller MUST reject duplicate `nonce` for the same stream.
- Windowing: `validBefore` MUST narrowly bound the slice (e.g., sliceEnd + ≤10s).
- Skew: apply ≥5s grace in verification to accommodate latency.
- Front‑running/DoS: on-chain-per-slice settlement avoids accumulation risk; otherwise cap unpaid exposure to ≤ one unit.
- Pause semantics: Seller MUST pause on TTL expiry without the next accepted prepay.

### Example WS Frames
stream.require
```json
{
  "id": "6f2e...",
  "method": "stream.require",
  "params": {
    "streamId": "abc123",
    "sliceIndex": 7,
    "expiresAt": 1730000123,
    "requirements": {
      "x402Version": 1,
      "scheme": "exact",
      "network": "base-sepolia",
      "maxAmountRequired": "50000",
      "resource": "wss://seller.example/streams/abc123",
      "description": "Video minute 7",
      "mimeType": "video/mp4",
      "payTo": "0xPAYEE...",
      "maxTimeoutSeconds": 120,
      "asset": "0xUSDC...",
      "extra": { "name": "USDC", "version": "2" }
    }
  }
}
```

stream.pay
```json
{
  "id": "6f2e...",
  "method": "stream.pay",
  "params": {
    "streamId": "abc123",
    "sliceIndex": 7,
    "paymentPayload": {
      "x402Version": 1,
      "scheme": "exact",
      "network": "base-sepolia",
      "payload": {
        "signature": "0x...",
        "authorization": {
          "from": "0xPAYER...",
          "to": "0xPAYEE...",
          "value": "50000",
          "validAfter": 1729999500,
          "validBefore": 1730000200,
          "nonce": "0x...32bytes..."
        }
      }
    }
  }
}
```

stream.accept (on-chain per slice)
```json
{
  "id": "6f2e...",
  "result": {
    "verify": { "isValid": true, "payer": "0xPAYER..." },
    "settle": {
      "success": true,
      "payer": "0xPAYER...",
      "transaction": "0xTXHASH...",
      "network": "base-sepolia"
    },
    "prepaidUntilMs": 1730000130000
  }
}
```

### Facilitator over WS
Mirror the HTTP API as WS methods:
- `x402.supported` → `{ kinds: [{ x402Version, scheme, network, extra? }] }`
- `x402.verify` → `VerifyResponse`
- `x402.settle` → `SettleResponse`

### Client/Server Pseudocode
Buyer loop (TypeScript-like)
```ts
ws.on('stream.require', async ({ streamId, sliceIndex, requirements }) => {
  const paymentPayload = await signEip3009(requirements);
  ws.send({ method: 'stream.pay', params: { streamId, sliceIndex, paymentPayload } });
});

ws.on('stream.accept', ({ prepaidUntilMs }) => {
  scheduleNextPay(prepaidUntilMs - Date.now() - 30000); // 30s before expiry
});
```

Seller loop (Rust-like pseudocode)
```rust
loop {
    if prepaid_remaining_ms() < threshold_ms {
        let req = build_payment_requirements_for_next_slice();
        send_ws("stream.require", req);
    }
    match recv_ws() {
        StreamPay { payment_payload } => {
            let verify = facilitator.verify(&VerifyRequest { payment_payload, payment_requirements: req });
            if !verify.is_valid { send_ws("stream.reject", reason); pause_if_needed(); continue; }
            let settle = if onchain_per_slice { Some(facilitator.settle(&req)) } else { None };
            extend_prepaid_window(unit_seconds);
            send_ws("stream.accept", { verify, settle, prepaidUntilMs });
            resume_stream_if_paused();
        }
        _ => {}
    }
}
```

### Parameter Recommendations
- unitSeconds: 10–120 (trade-off: latency vs. on-chain frequency)
- ttlLeadMs (when to require next slice): 30–70% into unit
- valid window: `[sliceStart, sliceEnd + 5..10s]`
- token: USDC (EIP‑3009) with correct `extra.name/version` when needed

### Compatibility
- Backwards-compatible with existing x402 Exact scheme; only transport and cadence are defined here.
- HTTP-gated x402 endpoints can coexist; this WS profile bypasses HTTP headers.

### Future Work
- Streaming escrow contract (Sablier-style) for continuous accrual with on-chain claimability.
- Unidirectional state channels with challenge windows.
- Multi-asset and price discovery via WS.

### Glossary
- Slice: a fixed-duration prepaid time unit.
- TTL: time before the next prepayment must be received.
- Exact: x402 scheme requiring exact token amount per payment.


