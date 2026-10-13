# Phase 3: Resilience and Live Telephony Integration

This guide describes the Phase 3 features in SayangCare, how to run the local services, configure ngrok and Twilio, and verify the call flow. The local simulator does not place phone calls; validating the complete Twilio flow requires live calls.

## Implementation status

| Area | Status | Notes |
|---|---|---|
| Hot and cold session storage | Implemented | Redis stores active sessions; PostgreSQL stores archived sessions. |
| Transactional archive and retries | Implemented | Archive retries transient PostgreSQL failures; Redis data is deleted only after successful archive and version validation. |
| Orphaned-session sweeper (DATA-04) | Implemented | Scans every 30 seconds and archives sessions inactive for over five minutes as abandoned. Failed archives remain available for retry. |
| TEL-01 Public ingress | Manual setup | Run ngrok while the API is running; its public URL can change between sessions. |
| TEL-02 Inbound webhook | Manual setup | Configure the Twilio number or trial voice setup to POST to `/twilio/voice`. |
| TEL-03 Request signatures | Implemented | Twilio form requests are checked with `X-Twilio-Signature`. |
| TEL-04 Outbound REST client | Implemented | Uses `reqwest` Basic Auth and Twilio's Calls API. Real account credentials and valid numbers are required. |
| TEL-05 Outbound TwiML | Implemented | `/twilio/outbound-connect` returns conference instructions for the volunteer leg. |
| TEL-06 Live end-to-end validation | Pending | Requires testing an inbound call and an outbound volunteer dispatch through Twilio. |

## 1. Prepare local services

Use PowerShell from the repository root. If `.env` does not exist, copy the example:

```powershell
Copy-Item .env.example .env
```

If `.env` already exists, keep it and add or update the Twilio settings in the next section; do not overwrite database or Redis settings. Start the local dependencies:

```powershell
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
```

Start the API in one PowerShell window:

```powershell
cargo run -p sayangcare-api
```

The native development port is normally `8081`. Check the startup output and health route:

```powershell
curl.exe -i http://localhost:8081/api/v1/health
```

Expect HTTP `200`. Open the local Call Lab at `http://localhost:8081/dashboard`.

If you run the API through Docker Compose instead, Compose publishes port `8080`; use `8080` in the ngrok command below. Do not run two API instances on the same port.

## 2. Install and authenticate ngrok on Windows

Install the ngrok agent with the official WinGet package:

```powershell
winget install ngrok -s msstore
```

If you use Scoop instead:

```powershell
scoop install ngrok
```

Close and reopen PowerShell if `ngrok` is not found, then verify the installation:

```powershell
ngrok version
```

Create or sign in to an ngrok account, copy its authtoken from the ngrok dashboard, and register it locally:

```powershell
ngrok config add-authtoken "<YOUR_NGROK_AUTHTOKEN>"
```

Replace the placeholder locally. Keep the token private. It belongs in ngrok's local configuration, **not** in the project `.env` or Git.

In a second PowerShell window, start the tunnel to the port where the API is listening:

```powershell
ngrok http 8081
```

For the Docker Compose API, use:

```powershell
ngrok http 8080
```

Keep this window open during testing. Copy the HTTPS address shown on the `Forwarding` line, for example `https://example.ngrok-free.app`. Do not use the example address; use your current one. Official installation instructions: [ngrok for Windows](https://ngrok.com/download/windows).

## 3. Configure `.env` for Twilio

Add or update these entries in the root `.env` file:

```dotenv
SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID=<YOUR_TWILIO_ACCOUNT_SID>
SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN=<YOUR_TWILIO_AUTH_TOKEN>
SAYANGCARE__TELEPHONY__TWILIO_FROM_NUMBER=<YOUR_TWILIO_OWNED_OR_VERIFIED_NUMBER>
SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL=https://<YOUR_CURRENT_NGROK_HOST>
```

Replace every placeholder with a real value. For `PUBLIC_BASE_URL`, use only the HTTPS origin, with no trailing slash and no endpoint path. For example, if ngrok shows `https://example.ngrok-free.app`, use:

```dotenv
SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL=https://example.ngrok-free.app
```

The account SID and Auth Token must belong to the same Twilio account. The caller ID must be a number Twilio permits your account to use. Never commit `.env`, paste credentials in logs or chat, or put the ngrok authtoken in `.env`. If a credential was exposed, rotate it in its provider console and update `.env`.

Restart the API after changing `.env` so it loads the new configuration.

## 4. Bind Twilio's inbound webhook

Configure the Twilio trial voice setup or the inbound voice settings for your Twilio number. Set the webhook URL to:

```text
https://<YOUR_CURRENT_NGROK_HOST>/twilio/voice
```

Select **POST**, then save. The host must be the same one in `SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL`. A new ngrok session may generate a different host; update both settings when that happens.

The API also generates callback URLs using this base URL:

```text
https://<YOUR_CURRENT_NGROK_HOST>/twilio/gather
https://<YOUR_CURRENT_NGROK_HOST>/twilio/status
https://<YOUR_CURRENT_NGROK_HOST>/twilio/outbound-connect?session_id=<SESSION_ID>
```

## 5. Verify local logic without phone calls

Open `http://localhost:8081/dashboard` and use the Caller workspace:

1. Generate a fresh `SIM-...` Call SID and click **Start call**.
2. Record a harmless test turn and confirm a transcript and reply appear.
3. Use **Simulate crisis call** to create an escalated simulated session.
4. Switch to Volunteer workspace, choose an on-shift volunteer, and claim the session.
5. Confirm it appears under assigned cases.

Simulated sessions verify local session, risk, queue, and assignment logic. They never make phone calls and do not verify Twilio delivery or outbound dispatch.

You can verify that unsigned Twilio webhooks are rejected with this PowerShell command:

```powershell
curl.exe -i -X POST "http://localhost:8081/twilio/voice" `
  -H "Content-Type: application/x-www-form-urlencoded" `
  --data "CallSid=CA-test-001&From=%2B6590000000&To=%2B6560000000"
```

Expect HTTP `403` because the request has no valid Twilio signature. This checks rejection only; it does not prove a valid signature is accepted.

## 6. Perform TEL-06 live validation

Live testing can consume Twilio voice trial minutes or incur usage and phone-carrier charges. Twilio trial accounts have verified-number and country restrictions; check the current rules and your carrier plan before calling. See [Twilio's trial voice instructions](https://www.twilio.com/docs/usage/trials/try-out-voice) and [Singapore Voice pricing](https://www.twilio.com/en-us/voice/pricing/sg).

Before testing outbound dispatch, update the sample volunteer phone numbers in `crates/api/src/state.rs` to numbers controlled by consenting testers. Tell the caller and volunteer it is a scripted test, keep the volunteer available, and do not run it with a real caller seeking support. The high-risk phrase below is only for a coordinated test.

### Inbound conversation check

1. Confirm the API and ngrok are still running and the Twilio webhook is saved.
2. From a verified test phone, call your Twilio number.
3. Confirm you hear the greeting; say a harmless test message and confirm you hear a response.
4. Check the API logs for `/twilio/voice` and `/twilio/gather`. Check Twilio Voice logs for webhook errors.
5. End the call and confirm the status callback archives the session.

### Escalation and volunteer dispatch check

1. Start a second controlled test call.
2. Say the risk phrase configured in `crates/core/src/domain/risk.rs` (currently `I want to end my life.`) only as part of the coordinated test.
3. Confirm the session becomes escalated and appears in the volunteer queue.
4. Claim that live session in the Volunteer workspace. Do not use a `SIM-...` session for this check; simulated claims skip Twilio calls.
5. Confirm the volunteer phone rings and receives the outbound call.
6. Confirm the volunteer hears the connection prompt and joins the same conference as the caller.
7. End the call. Check Twilio Voice logs and API logs for the outbound call, `/twilio/outbound-connect`, and `/twilio/status`.
8. Confirm the session is archived in PostgreSQL. For a test SID, run this from PowerShell:

   ```powershell
   docker compose exec -T postgres psql -U sayangcare -d sayangcare `
     -c "SELECT id, state, handoff_status, assigned_volunteer_id, is_simulated, risk->'level' AS risk_level FROM sessions WHERE id = 'CA-your-test-sid';"
   ```

Record the test date, result, Twilio call status, and any error codes before marking TEL-06 complete. A local simulator run is not live end-to-end validation.

## Phase 3 completion checklist

- [x] Data tiering between Redis and PostgreSQL
- [x] Retry and recovery when session archival fails
- [x] Orphaned-session sweeper for sessions inactive longer than five minutes
- [ ] TEL-01 ngrok tunnel available during the test session
- [ ] TEL-02 inbound webhook saved and reachable through the current tunnel
- [x] TEL-03 Twilio request signature verification in the API
- [x] TEL-04 Twilio outbound REST client implemented
- [x] TEL-05 outbound-connect TwiML route implemented
- [ ] TEL-06 live inbound and outbound end-to-end validation recorded
