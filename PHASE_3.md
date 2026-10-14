# Phase 3: Resilience and Live Telephony

Run all commands in PowerShell from the repository root.

## Status

| Area | Status |
|---|---|
| Hot/cold session storage (Redis + PostgreSQL) | Done |
| Archive with retries | Done |
| Orphaned-session sweeper (every 30s, >5 min inactive) | Done |
| TEL-01 ngrok tunnel | Done |
| TEL-02 Inbound webhook | Done |
| TEL-03 Twilio signature verification | Done |
| TEL-04 Outbound REST client | Done |
| TEL-05 Outbound-connect TwiML | Done |
| TEL-06 Live end-to-end test | Pending (US trial number; team is in Singapore) |

## 1. Start services

```powershell
Copy-Item .env.example .env   # only if .env does not exist
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
cargo run -p sayangcare-api
```

Check health (expect `200`):

```powershell
curl.exe -i http://localhost:8081/api/v1/health
```

Dashboard: `http://localhost:8081/dashboard`

## 2. ngrok

One-time setup:

```powershell
winget install ngrok -s msstore
ngrok config add-authtoken "<YOUR_NGROK_AUTHTOKEN>"
ngrok version
```

Start the tunnel (use `8080` instead if running the API via Docker Compose):

```powershell
ngrok http 8081 --url tackling-mud-chip.ngrok-free.dev
```

Only one agent can use the domain at a time (`ERR_NGROK_334` means it is already online). Inspect traffic at `http://127.0.0.1:4040`.

## 3. Configure `.env`

Copy these from your Twilio console and `.env` file:

```dotenv
SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID=<your-account-sid-from-.env>
SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN=<your-auth-token-from-.env>
SAYANGCARE__TELEPHONY__TWILIO_FROM_NUMBER=<your-twilio-number-from-.env>
SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL=https://<your-ngrok-url>.ngrok-free.dev
```

**⚠️ NEVER commit `.env` or actual credentials to git. Use `git update-index --skip-worktree .env` to exclude it from tracking.**

`PUBLIC_BASE_URL` is the HTTPS origin only (no trailing slash or path). Never commit `.env`. Restart the API after changes.

## 4. Twilio webhook

In the Twilio console, set the number's voice webhook to **POST**:
(ALREADY DONE SO SKIP.)
```text
https://tackling-mud-chip.ngrok-free.dev/twilio/voice
```

The API derives `/twilio/gather`, `/twilio/status` and `/twilio/outbound-connect` from `PUBLIC_BASE_URL`.

## 5. Verify without phone calls

Simulator (no Twilio calls): on the dashboard, start a `SIM-...` call, run **Simulate crisis call**, then claim it in the Volunteer workspace.

Unsigned webhook must be rejected (expect `403`):

```powershell
curl.exe -i -X POST "https://tackling-mud-chip.ngrok-free.dev/twilio/voice" `
  -H "Content-Type: application/x-www-form-urlencoded" `
  --data "CallSid=CA-test-001&From=%2B6590000000&To=%2B6560000000"
```

## 6. TEL-06 live validation (pending)

Requires verified test phones and consenting testers. Set volunteer phone numbers in `crates/api/src/state.rs` first.

Inbound:

1. Call the Twilio number and confirm the greeting and a reply.
2. Check API logs for `/twilio/voice` and `/twilio/gather`.
3. End the call and confirm `/twilio/status` archives the session.

Escalation and dispatch:

1. Place a second call and say the risk phrase from `crates/core/src/domain/risk.rs` (coordinated test only).
2. Claim the live (non-`SIM-`) session in the Volunteer workspace.
3. Confirm the volunteer phone rings and joins the caller's conference.
4. Confirm the session is archived:

```powershell (This command should show 0 rows for now. It is to verify the call is archieved, confirm volunteer, risk assessment and debug issues.)
docker compose exec -T postgres psql -U sayangcare -d sayangcare `
  -c "SELECT id, state, handoff_status, assigned_volunteer_id, is_simulated, risk->'level' AS risk_level FROM sessions WHERE id = 'CA-your-test-sid';"
```

Record the date, result, Twilio call status and any error codes.
