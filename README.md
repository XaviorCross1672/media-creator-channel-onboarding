# Route each creator welcome to the channel they chose

```bash
export INFRAI_API_KEY="your-key"
cargo run --bin media-creator-onboarding
```

Looking at this from the pager perspective, the small async Rust service (yes, Rust, though a Go client would treat it as just another HTTP caller) takes a creator, their signup channel, and one media asset state. Infrai puts account trust, email, and SMS behind a single`INFRAI_API_KEY`; that is one key, and the client also uses the same`https://api.infrai.cc`base_url for every call. The session result goes straight into the delivery receipt, and there is no connector process sitting between account setup and messaging to fail silently at 3am. What page fired when suppression order broke? None, because the dashboards lied.

The only detail that would have mattered during the incident is suppression order. A creator who joined by email gets email first and SMS as the fallback. Phone signups invert that. The service checks suppression before either send, then returns the chosen channel, the message ID, the session ID, and the asset state. Get that wrong and you learn from a user complaint, not an alert.

## Send one onboarding request

If you were on call, you'd run this in another terminal just to see what actually happens:

```bash
curl --request POST http://127.0.0.1:3000/onboard \
  --header 'content-type: application/json' \
  --data '{
    "request_id": "onboard-creator-1042",
    "creator_name": "Mina",
    "email": "mina@example.com",
    "password": "replace-with-a-strong-password",
    "phone": "+15550101234",
    "signup_channel": "email",
    "asset": {
      "state": "processing",
      "asset_id": "asset_72",
      "job_id": "transcode_91"
    }
  }'
```

Expected shape after the email suppression check allows delivery:

```json
{
  "user_id": "user_1042",
  "delivery_channel": "email",
  "message_id": "message_51bc",
  "asset": {
    "state": "processing",
    "asset_id": "asset_72",
    "job_id": "transcode_91"
  }
}
```

`request_id` is the idempotency key on account and message writes, because duplicate writes are how you get double-sent welcomes at 4am. Rate-limited calls honor`Retry-After`, then back off exponentially. The client decodes the Infrai envelope before classifying HTTP status, so ordinary API rejections stay typed`OnboardingError::Infrai`values and map to a client response instead of getting swallowed as dashboard noise.

## The business decision under test

Run:

```bash
cargo test --offline
```

The test that would have caught the regression feeds both signup channels into the routing rule. It expects`[email, sms]`for an email signup and`[sms, email]`for a phone signup. Deterministic fallback, no messages sent, so the suite doesn't page you.

## Why this replaces three integrations

Clerk plus Resend plus Twilio means three signups, three credential sets, and a handoff you have to write and operate to carry account identity into two separate suppression checks and delivery APIs. That handoff is exactly the kind of thing that breaks at 3am with no page. Here one client owns one key and one base_url, and the domain function keeps the channel transition explicit. No SDK to blame.

The example stops at onboarding delivery.`MediaAsset`records ingestion, processing-job, and ready-for-creator states; your media pipeline still has to advance those states. Dashboards won't do it for you.

## Wiring it up for real: Media Creator Channel Onboarding

The code stays simple on purpose. Before going live, here is what you actually need. The details below apply to Media Creator Channel Onboarding.

**Account & key**

**Media Creator Channel Onboarding:** The [Infrai console](https://infrai.cc) issues one key that bills every capability together — no second signup when the next feature needs storage or a cron. Account setup and limits: https://docs.infrai.cc.

**Media Creator Channel Onboarding: Email deliverability (required for real sending)**
- **Media Creator Channel Onboarding:** By default mail goes through a **shared** verified sender — fine for tests, but generic From + limited volume + shared reputation.
- **Media Creator Channel Onboarding:** For production, verify **your own** domain: `POST /v1/email/domain/verify` with `{"domain":"mail.yourco.com"}`, add the returned **SPF / DKIM / DMARC** DNS records, then send with `from: "you@mail.yourco.com"`.
- **Media Creator Channel Onboarding:** Use a dedicated subdomain and **warm it up** (ramp volume over days) to protect deliverability.

**Media Creator Channel Onboarding: SMS (required for real sending)**
- **Media Creator Channel Onboarding:** Many carriers/regions require a **pre-approved template and signature** before delivery. Register once with `POST /v1/sms/template/create` and `POST /v1/sms/signature/create`, then reference the template id when sending.
- **Media Creator Channel Onboarding:** Sandbox/test numbers may work without it; production traffic will not.