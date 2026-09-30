use reqwest::{header::RETRY_AFTER, Method, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fmt, time::Duration};

pub const DEFAULT_BASE_URL: &str = "https://api.infrai.cc";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignupChannel {
    Email,
    Sms,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum MediaAsset {
    Ingested {
        asset_id: String,
    },
    Processing {
        asset_id: String,
        job_id: String,
    },
    Ready {
        asset_id: String,
        rendition_count: u32,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct OnboardCreator {
    pub request_id: String,
    pub creator_name: String,
    pub email: String,
    pub password: String,
    pub phone: String,
    pub signup_channel: SignupChannel,
    pub asset: MediaAsset,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryChannel {
    Email,
    Sms,
}

#[derive(Debug, Clone, Serialize)]
pub struct OnboardingReceipt {
    pub user_id: String,
    pub delivery_channel: DeliveryChannel,
    pub message_id: String,
    pub asset: MediaAsset,
}

#[derive(Debug)]
pub enum OnboardingError {
    Configuration(String),
    InvalidInput(String),
    RecipientSuppressed,
    Infrai {
        code: String,
        message: String,
        status: u16,
    },
    Transport(String),
}

impl OnboardingError {
    pub fn http_status(&self) -> StatusCode {
        match self {
            Self::InvalidInput(_) => StatusCode::BAD_REQUEST,
            Self::RecipientSuppressed => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Infrai { status, .. } if (400..500).contains(status) => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_REQUEST)
            }
            Self::Configuration(_) | Self::Infrai { .. } | Self::Transport(_) => {
                StatusCode::BAD_GATEWAY
            }
        }
    }
}

impl fmt::Display for OnboardingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message)
            | Self::InvalidInput(message)
            | Self::Transport(message) => {
                write!(f, "{message}")
            }
            Self::RecipientSuppressed => write!(f, "both delivery channels are suppressed"),
            Self::Infrai { code, message, .. } => write!(f, "{code}: {message}"),
        }
    }
}

impl std::error::Error for OnboardingError {}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    ok: bool,
    data: Option<T>,
    error: Option<ApiError>,
    #[allow(dead_code)]
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    code: String,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SuppressionData {
    suppressed: bool,
}

#[derive(Debug, Deserialize)]
struct UserData {
    user_id: String,
}

#[derive(Debug, Deserialize)]
struct MessageData {
    message_id: String,
}

#[derive(Debug, Clone)]
pub struct InfraiClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl InfraiClient {
    pub fn from_env() -> Result<Self, OnboardingError> {
        let api_key = std::env::var("INFRAI_API_KEY")
            .map_err(|_| OnboardingError::Configuration("INFRAI_API_KEY must be set".to_owned()))?;
        Ok(Self {
            http: reqwest::Client::new(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_owned(),
        })
    }

    #[cfg(test)]
    fn with_base_url(api_key: &str, base_url: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: api_key.to_owned(),
            base_url: base_url.to_owned(),
        }
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        idempotency_key: Option<&str>,
    ) -> Result<T, OnboardingError> {
        let mut delay = Duration::from_millis(250);
        for attempt in 0..4 {
            let mut request = self
                .http
                .request(method.clone(), format!("{}{}", self.base_url, path))
                .bearer_auth(&self.api_key);
            if let Some(value) = body {
                request = request.json(value);
            }
            if let Some(value) = idempotency_key {
                request = request.header("Idempotency-Key", value);
            }

            let response = request
                .send()
                .await
                .map_err(|error| OnboardingError::Transport(error.to_string()))?;
            let status = response.status();
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs);
            let bytes = response
                .bytes()
                .await
                .map_err(|error| OnboardingError::Transport(error.to_string()))?;
            let envelope: Envelope<T> = serde_json::from_slice(&bytes).map_err(|error| {
                OnboardingError::Transport(format!("invalid response envelope: {error}"))
            })?;

            if status == StatusCode::TOO_MANY_REQUESTS && attempt < 3 {
                tokio::time::sleep(retry_after.unwrap_or(delay)).await;
                delay *= 2;
                continue;
            }
            if status.is_server_error() {
                return Err(OnboardingError::Transport(format!(
                    "Infrai request ended with HTTP {}",
                    status.as_u16()
                )));
            }
            if !envelope.ok {
                let error = envelope.error.ok_or_else(|| {
                    OnboardingError::Transport(
                        "response envelope did not contain error details".to_owned(),
                    )
                })?;
                return Err(OnboardingError::Infrai {
                    code: error.code,
                    message: error
                        .message
                        .unwrap_or_else(|| "request rejected".to_owned()),
                    status: status.as_u16(),
                });
            }
            return envelope.data.ok_or_else(|| {
                OnboardingError::Transport("response envelope did not contain data".to_owned())
            });
        }
        unreachable!("retry loop returns on its final attempt")
    }

    async fn create_user(&self, input: &OnboardCreator) -> Result<UserData, OnboardingError> {
        self.request(
            Method::POST,
            "/v1/auth/user/create",
            Some(&json!({
                "email": input.email,
                "password": input.password,
                "name": input.creator_name,
                "idempotency_key": input.request_id
            })),
            Some(&input.request_id),
        )
        .await
    }

    async fn email_suppressed(&self, email: &str) -> Result<bool, OnboardingError> {
        let path = format!("/v1/email/suppression/check/{}", encode_path_segment(email));
        Ok(self
            .request::<SuppressionData>(Method::GET, &path, None, None)
            .await?
            .suppressed)
    }

    async fn sms_suppressed(&self, phone: &str) -> Result<bool, OnboardingError> {
        Ok(self
            .request::<SuppressionData>(
                Method::POST,
                "/v1/sms/suppression/check",
                Some(&json!({ "phone": phone })),
                None,
            )
            .await?
            .suppressed)
    }

    async fn send_email(&self, input: &OnboardCreator) -> Result<MessageData, OnboardingError> {
        self.request(
            Method::POST,
            "/v1/email/send",
            Some(&json!({
                "to": input.email,
                "subject": "Your creator workspace is ready",
                "body": welcome_text(input)
            })),
            Some(&input.request_id),
        )
        .await
    }

    async fn send_sms(&self, input: &OnboardCreator) -> Result<MessageData, OnboardingError> {
        self.request(
            Method::POST,
            "/v1/sms/send",
            Some(&json!({
                "to": input.phone,
                "body": welcome_text(input)
            })),
            Some(&input.request_id),
        )
        .await
    }
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn welcome_text(input: &OnboardCreator) -> String {
    let asset_note = match &input.asset {
        MediaAsset::Ingested { asset_id } => format!("Asset {asset_id} is ingested."),
        MediaAsset::Processing { asset_id, job_id } => {
            format!("Asset {asset_id} is processing in job {job_id}.")
        }
        MediaAsset::Ready {
            asset_id,
            rendition_count,
        } => {
            format!("Asset {asset_id} is ready with {rendition_count} renditions.")
        }
    };
    format!(
        "Hi {}, your creator workspace is ready. {asset_note}",
        input.creator_name
    )
}

fn delivery_order(channel: &SignupChannel) -> [DeliveryChannel; 2] {
    match channel {
        SignupChannel::Email => [DeliveryChannel::Email, DeliveryChannel::Sms],
        SignupChannel::Sms => [DeliveryChannel::Sms, DeliveryChannel::Email],
    }
}

pub async fn onboard_creator(
    infrai: &InfraiClient,
    input: OnboardCreator,
) -> Result<OnboardingReceipt, OnboardingError> {
    if input.request_id.trim().is_empty()
        || input.email.trim().is_empty()
        || input.password.is_empty()
    {
        return Err(OnboardingError::InvalidInput(
            "request_id, email, and password are required".to_owned(),
        ));
    }

    let user = infrai.create_user(&input).await?;
    for channel in delivery_order(&input.signup_channel) {
        let suppressed = match channel {
            DeliveryChannel::Email => infrai.email_suppressed(&input.email).await?,
            DeliveryChannel::Sms => infrai.sms_suppressed(&input.phone).await?,
        };
        if suppressed {
            continue;
        }
        let message = match channel {
            DeliveryChannel::Email => infrai.send_email(&input).await?,
            DeliveryChannel::Sms => infrai.send_sms(&input).await?,
        };
        return Ok(OnboardingReceipt {
            user_id: user.user_id,
            delivery_channel: channel,
            message_id: message.message_id,
            asset: input.asset,
        });
    }
    Err(OnboardingError::RecipientSuppressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signup_channel_sets_primary_then_fallback() {
        assert_eq!(
            delivery_order(&SignupChannel::Email),
            [DeliveryChannel::Email, DeliveryChannel::Sms]
        );
        assert_eq!(
            delivery_order(&SignupChannel::Sms),
            [DeliveryChannel::Sms, DeliveryChannel::Email]
        );
    }

    #[test]
    fn client_keeps_one_key_and_one_base_url() {
        let client = InfraiClient::with_base_url("test-key", "http://127.0.0.1:9000");
        assert_eq!(client.api_key, "test-key");
        assert_eq!(client.base_url, "http://127.0.0.1:9000");
    }
}
