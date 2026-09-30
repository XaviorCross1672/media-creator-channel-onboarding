use media_creator_onboarding::creator_onboarding::{
    onboard_creator, InfraiClient, OnboardCreator, OnboardingError,
};
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(InfraiClient::from_env()?);
    let listener = TcpListener::bind("127.0.0.1:3000").await?;
    println!("creator onboarding listening on http://127.0.0.1:3000");
    loop {
        let (stream, _) = listener.accept().await?;
        let infrai = Arc::clone(&client);
        tokio::spawn(async move {
            if let Err(error) = serve_request(stream, infrai).await {
                eprintln!("request error: {error}");
            }
        });
    }
}

async fn serve_request(
    mut stream: TcpStream,
    infrai: Arc<InfraiClient>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut request = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 1024];
    let header_end = loop {
        let count = stream.read(&mut chunk).await?;
        if count == 0 || request.len() + count > 65_536 {
            write_response(&mut stream, 400, &json!({ "error": "invalid request" })).await?;
            return Ok(());
        }
        request.extend_from_slice(&chunk[..count]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };

    let headers = std::str::from_utf8(&request[..header_end])?;
    let request_line = headers.lines().next().unwrap_or_default();
    if request_line != "POST /onboard HTTP/1.1" {
        write_response(&mut stream, 404, &json!({ "error": "route not found" })).await?;
        return Ok(());
    }
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    if content_length == 0 || content_length > 61_440 {
        write_response(
            &mut stream,
            400,
            &json!({ "error": "invalid content length" }),
        )
        .await?;
        return Ok(());
    }
    while request.len() < header_end + content_length {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
    }

    let body = &request[header_end..request.len().min(header_end + content_length)];
    let input: OnboardCreator = match serde_json::from_slice(body) {
        Ok(input) => input,
        Err(error) => {
            write_response(&mut stream, 400, &json!({ "error": error.to_string() })).await?;
            return Ok(());
        }
    };
    match onboard_creator(&infrai, input).await {
        Ok(receipt) => write_response(&mut stream, 200, &serde_json::to_value(receipt)?).await?,
        Err(error) => {
            let status = error.http_status().as_u16();
            write_response(&mut stream, status, &error_body(error)).await?;
        }
    }
    Ok(())
}

fn error_body(error: OnboardingError) -> serde_json::Value {
    json!({ "error": error.to_string() })
}

async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    body: &serde_json::Value,
) -> Result<(), std::io::Error> {
    let payload = body.to_string();
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        _ => "Bad Gateway",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(response.as_bytes()).await
}
