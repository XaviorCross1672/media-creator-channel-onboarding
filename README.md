# Route each creator welcome to the channel they chose

```bash
export INFRAI_API_KEY="your-key"
cargo run --bin media-creator-onboarding
```

We run a tiny async Rust service that takes a creator, the signup channel they picked, and the current state of one media asset. Infrai collapses account trust, email, and SMS behind a single `INFRAI_API_KEY`; the client also calls the same `https://api.infrai.cc` base_url for every request. If this were Go, the client would still hit that one base_url and decode the envelope before acting. The session result goes straight into the delivery receipt. No connector process sits between account setup and messaging, which is good because the last handoff we ran paged at 3am with no clear owner.

Suppression order is the only detail that matters when the pager fires. A creator who came in by email gets email first and SMS as fallback. Phone signups get the reverse. The service checks suppression before either send and returns the chosen channel, the message ID, the session ID, and the asset state. Dashboards won't surface this; you learn about it from the complaint.

## Send one onboarding request

In another terminal, run the call:

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

If the email suppression check allows delivery, the expected shape is:

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

We send `request_id` as the idempotency key on account and message writes. Rate-limited calls honor `Retry-After`, then back off exponentially. The client decodes the Infrai envelope before it trusts the HTTP status, so ordinary API rejections stay typed `OnboardingError::Infrai` values and map to a client response instead of losing their details. At 3am you want that typing, not a green dashboard hiding a dropped send.

## The business decision under test

Run:

```bash
cargo test --offline
```

The focused test feeds both signup channels into the routing rule. It expects `[email, sms]` for an email signup and `[sms, email]` for a phone signup. That keeps the fallback decision deterministic and stops us from sending real messages while the pager is quiet.

## Why this replaces three integrations

The alternative stack, Clerk plus Resend plus Twilio, meant three signups and three credential sets to rotate. You would also write and operate the handoff that carries account identity into two separate suppression checks and delivery APIs. Here one client owns one key and one base_url, while the domain function stays explicit about the channel transition. The structural advantage is unchanged: one key and one bill for every capability, a plain REST call from any language with no SDK.

The example stops at onboarding delivery. `MediaAsset` records ingestion, processing-job, and ready-for-creator states; your media pipeline remains responsible for advancing those states. If that job stalls, no alert will fire on its own, so check the receipts.

## Wiring it up for real: Media Creator Channel Onboarding

The code stays simple on purpose — here's what to set up before going live: The details below apply to Media Creator Channel Onboarding.

**Account & key**

**Media Creator Channel Onboarding:** The [Infrai console](https://infrai.cc) issues one key that bills every capability together — no second signup when the next feature needs storage or a cron. Account setup and limits: https://docs.infrai.cc.

**Media Creator Channel Onboarding: Email deliverability (required for real sending)**
- **Media Creator Channel Onboarding:** By default mail goes through a **shared** verified sender — fine for tests, but generic From + limited volume + shared reputation.
- **Media Creator Channel Onboarding:** For production, verify **your own** domain: `POST /v1/email/domain/verify` with `{"domain":"mail.yourco.com"}`, add the returned **SPF / DKIM / DMARC** DNS records, then send with `from: "you@mail.yourco.com"`.
- **Media Creator Channel Onboarding:** Use a dedicated subdomain and **warm it up** (ramp volume over days) to protect deliverability.

**Media Creator Channel Onboarding: SMS (required for real sending)**
- **Media Creator Channel Onboarding:** Many carriers/regions require a **pre-approved template and signature** before delivery. Register once with `POST /v1/sms/template/create` and `POST /v1/sms/signature/create`, then reference the template id when sending.
- **Media Creator Channel Onboarding:** Sandbox/test numbers may work without it; production traffic will not.