use std::error::Error;
use std::io::{self, IsTerminal};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::{Request, Version};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{lookup_host, TcpStream};
use tokio_rustls::{rustls, TlsConnector};
use url::{Position, Url};

use crate::args::TraceRouteArgs;
use crate::commands::{EXIT_INVALID_ARGS, EXIT_OK, EXIT_RPC_FAILURE};
use crate::formatters::is_color_disabled;

const TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_REQUEST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"getHealth"}"#;

type ProbeError = Box<dyn Error + Send + Sync>;

#[derive(Serialize)]
struct TraceTimings {
    dns_lookup_ms: f64,
    tcp_connect_ms: f64,
    tls_handshake_ms: Option<f64>,
    http_ttfb_ms: f64,
    http_version: &'static str,
    http_status: u16,
}

pub async fn run_trace_route(args: TraceRouteArgs) -> i32 {
    let url = match Url::parse(&args.rpc_url) {
        Ok(url) if matches!(url.scheme(), "http" | "https") && url.has_host() => url,
        _ => {
            eprintln!("trace-route requires a valid HTTP or HTTPS RPC URL");
            return EXIT_INVALID_ARGS;
        }
    };
    let timings = match tokio::time::timeout(TIMEOUT, probe(&url)).await {
        Ok(Ok(timings)) => timings,
        Ok(Err(err)) => {
            eprintln!("trace-route failed: {err}");
            return EXIT_RPC_FAILURE;
        }
        Err(_) => {
            eprintln!("trace-route timed out after {} seconds", TIMEOUT.as_secs());
            return EXIT_RPC_FAILURE;
        }
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&timings).expect("serializable timings")
        );
    } else {
        let color = io::stdout().is_terminal() && !is_color_disabled(args.no_color);
        print_timing("DNS lookup", timings.dns_lookup_ms, color);
        print_timing("TCP connect", timings.tcp_connect_ms, color);
        if let Some(ms) = timings.tls_handshake_ms {
            print_timing("TLS handshake", ms, color);
        } else {
            println!("TLS handshake: n/a (plain HTTP)");
        }
        print_timing("HTTP TTFB", timings.http_ttfb_ms, color);
        println!(
            "HTTP protocol: {} (status {})",
            timings.http_version, timings.http_status
        );
    }
    EXIT_OK
}

fn print_timing(label: &str, ms: f64, color: bool) {
    let (rating, ansi) = if ms < 150.0 {
        ("green", 32)
    } else if ms < 500.0 {
        ("yellow", 33)
    } else {
        ("red", 31)
    };
    if color {
        println!("{label}: \x1b[{ansi}m{ms:.2} ms ({rating})\x1b[0m");
    } else {
        println!("{label}: {ms:.2} ms ({rating})");
    }
}

async fn probe(url: &Url) -> Result<TraceTimings, ProbeError> {
    let host = url.host_str().ok_or("URL has no host")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = url.port_or_known_default().ok_or("URL has no port")?;
    let start = Instant::now();
    let addresses: Vec<_> = lookup_host((host, port)).await?.collect();
    let dns_lookup_ms = milliseconds(start.elapsed());
    let start = Instant::now();
    let stream = TcpStream::connect(addresses.as_slice()).await?;
    let tcp_connect_ms = milliseconds(start.elapsed());
    let authority = &url[Position::BeforeHost..Position::AfterPort];
    let target = &url[Position::BeforePath..Position::AfterQuery];
    let (tls_handshake_ms, ttfb, version, status) = if url.scheme() == "https" {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let server_name = rustls::pki_types::ServerName::try_from(host.to_owned())?;
        let start = Instant::now();
        let tls = TlsConnector::from(Arc::new(config))
            .connect(server_name, stream)
            .await?;
        let handshake_ms = milliseconds(start.elapsed());
        let http2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
        let (ttfb, version, status) = request(tls, authority, target, http2).await?;
        (Some(handshake_ms), ttfb, version, status)
    } else {
        let (ttfb, version, status) = request(stream, authority, target, false).await?;
        (None, ttfb, version, status)
    };
    Ok(TraceTimings {
        dns_lookup_ms,
        tcp_connect_ms,
        tls_handshake_ms,
        http_ttfb_ms: milliseconds(ttfb),
        http_version: match version {
            Version::HTTP_2 => "HTTP/2",
            Version::HTTP_10 => "HTTP/1.0",
            _ => "HTTP/1.1",
        },
        http_status: status,
    })
}

async fn request<T: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: T,
    authority: &str,
    target: &str,
    http2: bool,
) -> Result<(Duration, Version, u16), ProbeError> {
    let mut builder = Request::post(target).header("content-type", "application/json");
    if http2 {
        builder = builder
            .uri(format!("https://{authority}{target}"))
            .version(Version::HTTP_2);
    } else {
        builder = builder.header("host", authority);
    }
    let request = builder.body(Full::new(Bytes::from_static(HEALTH_REQUEST.as_bytes())))?;

    let (ttfb, response) = if http2 {
        let (mut sender, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
                .await?;
        let driver = AbortOnDrop(tokio::spawn(connection));
        // HTTP/2 control frames can arrive before the response. Use the first
        // decoded response header block as the response first-byte boundary.
        let started = Instant::now();
        let response = sender.send_request(request).await?;
        let elapsed = started.elapsed();
        drop(driver);
        (elapsed, response)
    } else {
        let first_byte = Arc::new(OnceLock::new());
        let io = FirstByte {
            stream,
            first_byte: first_byte.clone(),
        };
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
        let driver = AbortOnDrop(tokio::spawn(connection));
        let started = Instant::now();
        let response = sender.send_request(request).await?;
        let elapsed = first_byte.get().map_or_else(
            || started.elapsed(),
            |at| at.saturating_duration_since(started),
        );
        drop(driver);
        (elapsed, response)
    };
    Ok((ttfb, response.version(), response.status().as_u16()))
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct FirstByte<T> {
    stream: T,
    first_byte: Arc<OnceLock<Instant>>,
}

impl<T: AsyncRead + Unpin> AsyncRead for FirstByte<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buf);
        if buf.filled().len() > before {
            self.first_byte.get_or_init(Instant::now);
        }
        result
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for FirstByte<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
