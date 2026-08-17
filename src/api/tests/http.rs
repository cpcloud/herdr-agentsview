// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::sync::{Arc, Once};
use std::time::Duration;

use rcgen::{generate_simple_self_signed, CertifiedKey};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use secrecy::{ExposeSecret, SecretString};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use url::Url;

const MAX_REQUEST_HEAD: usize = 64 * 1024;
static INSTALL_TLS_PROVIDER: Once = Once::new();

pub(super) struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub had_bearer_header: bool,
    pub bearer_matched: Option<bool>,
}

pub(super) struct ResponsePlan {
    delay: Option<Duration>,
    response: PlannedResponse,
}

enum PlannedResponse {
    Complete {
        status: u16,
        body: Vec<u8>,
        headers: Vec<(String, String)>,
    },
    WithoutContentLength {
        status: u16,
        body: Vec<u8>,
        headers: Vec<(String, String)>,
    },
    Incomplete {
        body: Vec<u8>,
    },
    HeadersThenWait {
        status: u16,
        content_length: usize,
    },
    WaitForDisconnect,
}

impl ResponsePlan {
    pub fn json(body: impl AsRef<[u8]>) -> Self {
        Self {
            delay: None,
            response: PlannedResponse::Complete {
                status: 200,
                body: body.as_ref().to_vec(),
                headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
            },
        }
    }

    pub fn status(status: u16, body: impl AsRef<[u8]>) -> Self {
        Self {
            delay: None,
            response: PlannedResponse::Complete {
                status,
                body: body.as_ref().to_vec(),
                headers: vec![("Content-Type".to_owned(), "text/plain".to_owned())],
            },
        }
    }

    pub fn without_content_length(status: u16, body: impl AsRef<[u8]>) -> Self {
        Self {
            delay: None,
            response: PlannedResponse::WithoutContentLength {
                status,
                body: body.as_ref().to_vec(),
                headers: vec![("Content-Type".to_owned(), "text/plain".to_owned())],
            },
        }
    }

    pub fn redirect(location: &str) -> Self {
        Self {
            delay: None,
            response: PlannedResponse::Complete {
                status: 302,
                body: Vec::new(),
                headers: vec![("Location".to_owned(), location.to_owned())],
            },
        }
    }

    pub fn incomplete(body: impl AsRef<[u8]>) -> Self {
        Self {
            delay: None,
            response: PlannedResponse::Incomplete {
                body: body.as_ref().to_vec(),
            },
        }
    }

    pub fn status_then_wait(status: u16) -> Self {
        Self {
            delay: None,
            response: PlannedResponse::HeadersThenWait {
                status,
                content_length: 64,
            },
        }
    }

    pub fn status_then_wait_with_length(status: u16, content_length: usize) -> Self {
        Self {
            delay: None,
            response: PlannedResponse::HeadersThenWait {
                status,
                content_length,
            },
        }
    }

    pub fn wait_for_disconnect() -> Self {
        Self {
            delay: None,
            response: PlannedResponse::WaitForDisconnect,
        }
    }

    pub fn delayed(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }
}

pub(super) struct RecordingServer {
    base_url: Url,
    requests: mpsc::UnboundedReceiver<RecordedRequest>,
    task: Option<JoinHandle<io::Result<()>>>,
}

impl RecordingServer {
    pub async fn start(plan: ResponsePlan) -> Self {
        Self::start_sequence(vec![plan]).await
    }

    pub async fn start_sequence(plans: Vec<ResponsePlan>) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, requests) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            for plan in plans {
                let (stream, _) = listener.accept().await?;
                serve_connection(stream, plan, None, request_tx.clone()).await?;
            }
            Ok(())
        });
        Self {
            base_url: Url::parse(&format!("http://{address}/")).unwrap(),
            requests,
            task: Some(task),
        }
    }

    pub async fn start_tls(
        plan: ResponsePlan,
        expected_bearer: SecretString,
    ) -> (Self, reqwest::Certificate) {
        INSTALL_TLS_PROVIDER.call_once(|| {
            if rustls::crypto::aws_lc_rs::default_provider()
                .install_default()
                .is_err()
            {
                assert!(
                    rustls::crypto::CryptoProvider::get_default().is_some(),
                    "a Rustls crypto provider must be installed for the TLS recorder"
                );
            }
        });
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
        let certificate_der = cert.der().clone();
        let root = reqwest::Certificate::from_der(certificate_der.as_ref()).unwrap();
        let private_key =
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate_der], private_key)
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, requests) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            let stream = acceptor.accept(stream).await.map_err(io::Error::other)?;
            serve_connection(stream, plan, Some(expected_bearer), request_tx).await
        });
        (
            Self {
                base_url: Url::parse(&format!("https://{address}/")).unwrap(),
                requests,
                task: Some(task),
            },
            root,
        )
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub async fn take_request(&mut self) -> RecordedRequest {
        self.requests
            .recv()
            .await
            .expect("recording server stopped before receiving a request")
    }

    pub async fn finish(&mut self) -> io::Result<()> {
        match self.task.take() {
            Some(task) => task.await.map_err(io::Error::other)?,
            None => Ok(()),
        }
    }
}

impl Drop for RecordingServer {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn serve_connection<S>(
    mut stream: S,
    plan: ResponsePlan,
    expected_bearer: Option<SecretString>,
    request_tx: mpsc::UnboundedSender<RecordedRequest>,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(offset) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break offset + 4;
        }
        if bytes.len() >= MAX_REQUEST_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head exceeds test limit",
            ));
        }
        let read = stream.read_buf(&mut bytes).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before request head",
            ));
        }
    };
    let request = parse_request(&bytes[..header_end], expected_bearer.as_ref())?;
    drop(bytes);
    let _ = request_tx.send(request);

    if let Some(delay) = plan.delay {
        tokio::time::sleep(delay).await;
    }
    match plan.response {
        PlannedResponse::Complete {
            status,
            body,
            headers,
        } => write_response(&mut stream, status, &headers, &body, body.len()).await,
        PlannedResponse::WithoutContentLength {
            status,
            body,
            headers,
        } => write_response_without_content_length(&mut stream, status, &headers, &body).await,
        PlannedResponse::Incomplete { body } => {
            write_response(&mut stream, 200, &[], &body, body.len() + 32).await
        }
        PlannedResponse::HeadersThenWait {
            status,
            content_length,
        } => {
            write_head(&mut stream, status, &[], Some(content_length)).await?;
            wait_for_disconnect(&mut stream).await
        }
        PlannedResponse::WaitForDisconnect => wait_for_disconnect(&mut stream).await,
    }
}

fn parse_request(
    bytes: &[u8],
    expected_bearer: Option<&SecretString>,
) -> io::Result<RecordedRequest> {
    let request = std::str::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request head is not UTF-8"))?;
    let mut lines = request.split("\r\n");
    let mut request_parts = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))?
        .split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request method"))?
        .to_owned();
    let target = request_parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request target"))?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let mut had_bearer_header = false;
    let mut bearer_matched = expected_bearer.map(|_| false);
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("authorization") {
            had_bearer_header = true;
            if let Some(expected) = expected_bearer {
                bearer_matched =
                    Some(value.trim().strip_prefix("Bearer ") == Some(expected.expose_secret()));
            }
        }
    }
    Ok(RecordedRequest {
        method,
        path: path.to_owned(),
        query,
        had_bearer_header,
        bearer_matched,
    })
}

async fn write_response<S>(
    stream: &mut S,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
    content_length: usize,
) -> io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    write_head(stream, status, headers, Some(content_length)).await?;
    stream.write_all(body).await?;
    stream.shutdown().await
}

async fn write_response_without_content_length<S>(
    stream: &mut S,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    write_head(stream, status, headers, None).await?;
    stream.write_all(body).await?;
    stream.shutdown().await
}

async fn write_head<S>(
    stream: &mut S,
    status: u16,
    headers: &[(String, String)],
    content_length: Option<usize>,
) -> io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason_phrase(status));
    if let Some(content_length) = content_length {
        head.push_str(&format!("Content-Length: {content_length}\r\n"));
    }
    head.push_str("Connection: close\r\n");
    for (name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await
}

async fn wait_for_disconnect<S>(stream: &mut S) -> io::Result<()>
where
    S: AsyncRead + Unpin,
{
    let mut byte = [0_u8; 1];
    loop {
        if stream.read(&mut byte).await? == 0 {
            return Ok(());
        }
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        302 => "Found",
        401 => "Unauthorized",
        403 => "Forbidden",
        500 => "Internal Server Error",
        _ => "Test Status",
    }
}
