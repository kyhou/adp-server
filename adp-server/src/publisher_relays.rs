//! Outbound connections to publisher-preferred relays (ADP-01 §Relay
//! selection, gap S8), under the security policy in
//! `docs/outbound-relay-policy.md`.
//!
//! A publisher's NIP-65 relay list is untrusted input, even when correctly
//! signed: any publisher can name any URL. Dialing those URLs from the
//! operator's network is a server-side request forgery risk, so every
//! connection goes through [`PolicyTransport`]:
//!
//! 1. the URL must be `wss://`, with a host, no credentials and no fragment;
//! 2. the host is resolved **once**, and the relay is rejected unless every
//!    returned address is public (no loopback, private, link-local,
//!    multicast, reserved, documentation, or other special-use range,
//!    including IPv4 embedded in IPv6);
//! 3. TCP connects to one of those validated addresses directly, and TLS and
//!    the WebSocket handshake run over that socket with the original hostname,
//!    so a second DNS answer can never redirect the connection (DNS
//!    rebinding) and certificates are still checked against the hostname;
//! 4. HTTP redirects during the handshake are never followed.
//!
//! Publisher relays are an optimization only. They can add candidate events,
//! such as a newer signed listing, but never count toward completeness or
//! freshness thresholds, and their failures never fail an operation.
use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use async_wsocket::futures_util::stream::SplitSink;
use async_wsocket::futures_util::{Sink, SinkExt, StreamExt, TryStreamExt};
use async_wsocket::{ConnectionMode, Message, WebSocket};
use nostr_sdk::pool::transport::error::TransportError;
use nostr_sdk::pool::transport::websocket::{WebSocketSink, WebSocketStream, WebSocketTransport};
use nostr_sdk::util::BoxedFuture;
use nostr_sdk::{Client, Event, Kind, PublicKey, RelayOptions, RelayUrl, Url};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;

/// Maximum publisher-preferred relays taken from one relay list.
pub const MAX_RELAYS_PER_PUBLISHER: usize = 5;
/// Maximum publisher-preferred relays held at once, across all publishers.
pub const MAX_PREFERRED_RELAYS_TOTAL: usize = 25;
/// Maximum simultaneous outbound connection attempts.
pub const MAX_CONCURRENT_DIALS: usize = 8;
/// Time limit for one connection attempt (DNS, TCP, TLS and handshake).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a publisher's resolved relay list is reused.
pub const RELAY_LIST_TTL: Duration = Duration::from_secs(600);
/// Maximum publishers whose relay lists are cached.
const MAX_CACHED_PUBLISHERS: usize = 1024;

/// NIP-65 relay list metadata.
pub const RELAY_LIST_KIND: u16 = 10002;

/// Why a publisher relay was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RelayRejection {
    #[error("relay URL is malformed")]
    Malformed,
    #[error("relay URL must use wss://")]
    NotWss,
    #[error("relay URL must not contain credentials")]
    Credentials,
    #[error("relay URL must not contain a fragment")]
    Fragment,
    #[error("relay URL has no host")]
    NoHost,
    #[error("relay host did not resolve")]
    Unresolvable,
    #[error("relay resolves to a non-public address: {0}")]
    NonPublicAddress(IpAddr),
    #[error("proxied connection modes are not allowed for publisher relays")]
    ConnectionMode,
    #[error("outbound relay connection limit reached")]
    ConnectionLimit,
    #[error("relay connection timed out")]
    Timeout,
    #[error("relay connection failed: {0}")]
    Connect(String),
    #[error("relay handshake failed: {0}")]
    Handshake(String),
}

/// Validates the syntax of a publisher relay URL (policy rules 1 and, for IP
/// literals, 2). Hostnames are checked again after resolution.
pub fn validate_relay_url(raw: &str) -> Result<Url, RelayRejection> {
    let url = Url::parse(raw).map_err(|_| RelayRejection::Malformed)?;
    if url.scheme() != "wss" {
        return Err(RelayRejection::NotWss);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(RelayRejection::Credentials);
    }
    if url.fragment().is_some() {
        return Err(RelayRejection::Fragment);
    }
    match url.host() {
        None => Err(RelayRejection::NoHost),
        Some(url::Host::Domain("")) => Err(RelayRejection::NoHost),
        Some(url::Host::Ipv4(ip)) if !is_public_ip(IpAddr::V4(ip)) => {
            Err(RelayRejection::NonPublicAddress(IpAddr::V4(ip)))
        }
        Some(url::Host::Ipv6(ip)) if !is_public_ip(IpAddr::V6(ip)) => {
            Err(RelayRejection::NonPublicAddress(IpAddr::V6(ip)))
        }
        Some(_) => Ok(url),
    }
}

/// Whether `ip` is a public, globally routable unicast address. Anything else
/// (and anything this function does not recognize as public) is refused.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_ipv4(v4),
        IpAddr::V6(v6) => is_public_ipv6(v6),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0                                   // 0.0.0.0/8 "this network"
        || a == 10                             // 10.0.0.0/8 private
        || a == 127                            // 127.0.0.0/8 loopback
        || (a == 100 && (64..=127).contains(&b)) // 100.64.0.0/10 shared (CGNAT)
        || (a == 169 && b == 254)              // 169.254.0.0/16 link-local, cloud metadata
        || (a == 172 && (16..=31).contains(&b)) // 172.16.0.0/12 private
        || (a == 192 && b == 0 && c == 0)      // 192.0.0.0/24 IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)      // 192.0.2.0/24 documentation
        || (a == 192 && b == 88 && c == 99)    // 192.88.99.0/24 6to4 relay anycast
        || (a == 192 && b == 168)              // 192.168.0.0/16 private
        || (a == 198 && (18..=19).contains(&b)) // 198.18.0.0/15 benchmarking
        || (a == 198 && b == 51 && c == 100)   // 198.51.100.0/24 documentation
        || (a == 203 && b == 0 && c == 113)    // 203.0.113.0/24 documentation
        || a >= 224) // 224.0.0.0/4 multicast, 240.0.0.0/4 reserved, broadcast
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let embedded_v4 = |high: u16, low: u16| {
        Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8)
    };
    // IPv4-mapped ::ffff:0:0/96: judge the embedded IPv4 address.
    if segments[..5] == [0, 0, 0, 0, 0] && segments[5] == 0xffff {
        return is_public_ipv4(embedded_v4(segments[6], segments[7]));
    }
    // NAT64 64:ff9b::/96: judge the embedded IPv4 address.
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_public_ipv4(embedded_v4(segments[6], segments[7]));
    }
    // Only global unicast 2000::/3 can be public.
    if segments[0] & 0xe000 != 0x2000 {
        return false; // ::, ::1, IPv4-compatible, fc00::/7, fe80::/10, ff00::/8, ...
    }
    !(segments[0] == 0x2001 && segments[1] < 0x0200 // 2001::/23 IETF (incl. Teredo 2001::/32)
        || segments[0] == 0x2001 && segments[1] == 0x0db8 // 2001:db8::/32 documentation
        || segments[0] == 0x2002 // 2002::/16 6to4 (tunnels to arbitrary IPv4)
        || segments[0] == 0x3fff && segments[1] < 0x1000) // 3fff::/20 documentation
}

/// Resolves a hostname. Injectable so tests can simulate DNS answers.
#[async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>>;
}

/// The operating system resolver.
pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        Ok(tokio::net::lookup_host((host, port)).await?.collect())
    }
}

/// Opens a TCP connection to an already validated address. Injectable so
/// tests can route a validated public address to a local test relay.
#[async_trait]
pub trait Dialer: Send + Sync {
    async fn dial(&self, addr: SocketAddr) -> io::Result<TcpStream>;
}

/// Direct TCP connection to exactly the given address.
pub struct SystemDialer;

#[async_trait]
impl Dialer for SystemDialer {
    async fn dial(&self, addr: SocketAddr) -> io::Result<TcpStream> {
        TcpStream::connect(addr).await
    }
}

/// Resolves `url` once and returns its addresses only if every one of them is
/// public (policy rule 2). A mixed answer is rejected as a whole.
pub async fn resolve_public(
    url: &Url,
    resolver: &dyn Resolver,
) -> Result<Vec<SocketAddr>, RelayRejection> {
    let port = url
        .port_or_known_default()
        .ok_or(RelayRejection::Malformed)?;
    let addresses = match url.host() {
        Some(url::Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        Some(url::Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        Some(url::Host::Domain(domain)) => resolver
            .resolve(domain, port)
            .await
            .map_err(|_| RelayRejection::Unresolvable)?,
        None => return Err(RelayRejection::NoHost),
    };
    if addresses.is_empty() {
        return Err(RelayRejection::Unresolvable);
    }
    if let Some(address) = addresses.iter().find(|address| !is_public_ip(address.ip())) {
        return Err(RelayRejection::NonPublicAddress(address.ip()));
    }
    Ok(addresses)
}

/// WebSocket transport that enforces the outbound relay policy on every
/// connection, including the library's own reconnection attempts.
pub struct PolicyTransport {
    resolver: Arc<dyn Resolver>,
    dialer: Arc<dyn Dialer>,
    connector: Option<tokio_tungstenite::Connector>,
    dials: Arc<Semaphore>,
}

impl std::fmt::Debug for PolicyTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyTransport").finish_non_exhaustive()
    }
}

impl PolicyTransport {
    /// Production transport: system DNS, direct TCP, rustls with webpki roots.
    pub fn new() -> Self {
        Self::with_network(Arc::new(SystemResolver), Arc::new(SystemDialer), None)
    }

    /// Transport with injected DNS, TCP and TLS. `connector: None` uses the
    /// default rustls connector (webpki roots, hostname verification).
    #[doc(hidden)]
    pub fn with_network(
        resolver: Arc<dyn Resolver>,
        dialer: Arc<dyn Dialer>,
        connector: Option<tokio_tungstenite::Connector>,
    ) -> Self {
        Self {
            resolver,
            dialer,
            connector,
            dials: Arc::new(Semaphore::new(MAX_CONCURRENT_DIALS)),
        }
    }

    /// Connects under the policy. Exposed for tests; the relay pool calls it
    /// through [`WebSocketTransport::connect`].
    pub async fn connect_checked(
        &self,
        raw_url: &str,
        timeout: Duration,
    ) -> Result<WebSocket, RelayRejection> {
        let _permit = self
            .dials
            .clone()
            .try_acquire_owned()
            .map_err(|_| RelayRejection::ConnectionLimit)?;
        let url = validate_relay_url(raw_url)?;
        let attempt = async {
            // Resolve exactly once; connect only to these validated addresses.
            let addresses = resolve_public(&url, self.resolver.as_ref()).await?;
            let mut last_error = RelayRejection::Unresolvable;
            let mut tcp = None;
            for address in addresses {
                match self.dialer.dial(address).await {
                    Ok(stream) => {
                        tcp = Some(stream);
                        break;
                    }
                    Err(error) => last_error = RelayRejection::Connect(error.to_string()),
                }
            }
            let tcp = tcp.ok_or(last_error)?;
            // TLS SNI and certificate checks use the hostname in `url`; no
            // further DNS lookup happens. tungstenite does not follow
            // redirects: a 3xx response fails the handshake.
            let (stream, _) = tokio_tungstenite::client_async_tls_with_config(
                url.as_str(),
                tcp,
                None,
                self.connector.clone(),
            )
            .await
            .map_err(|error| RelayRejection::Handshake(error.to_string()))?;
            Ok::<_, RelayRejection>(stream)
        };
        let stream = tokio::time::timeout(timeout, attempt)
            .await
            .map_err(|_| RelayRejection::Timeout)??;
        Ok(WebSocket::Tokio(stream))
    }
}

impl Default for PolicyTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSocketTransport for PolicyTransport {
    fn support_ping(&self) -> bool {
        true
    }

    fn connect<'a>(
        &'a self,
        url: &'a Url,
        mode: &'a ConnectionMode,
        timeout: Duration,
    ) -> BoxedFuture<'a, Result<(WebSocketSink, WebSocketStream), TransportError>> {
        Box::pin(async move {
            if !matches!(mode, ConnectionMode::Direct) {
                return Err(TransportError::backend(RelayRejection::ConnectionMode));
            }
            let socket = self
                .connect_checked(url.as_str(), timeout.min(CONNECT_TIMEOUT))
                .await
                .map_err(|rejection| {
                    tracing::debug!(%url, %rejection, "publisher relay refused");
                    TransportError::backend(rejection)
                })?;
            let (tx, rx) = socket.split();
            let sink: WebSocketSink = Box::new(PolicySink(tx));
            let stream: WebSocketStream = Box::pin(rx.map_err(TransportError::backend));
            Ok((sink, stream))
        })
    }
}

/// Mirrors the library's private sink adapter (errors mapped per call rather
/// than with `sink_map_err`; see rust-nostr/nostr#984).
struct PolicySink(SplitSink<WebSocket, Message>);

impl Sink<Message> for PolicySink {
    type Error = TransportError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_ready_unpin(cx)
            .map_err(TransportError::backend)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.0)
            .start_send_unpin(item)
            .map_err(TransportError::backend)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_flush_unpin(cx)
            .map_err(TransportError::backend)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_close_unpin(cx)
            .map_err(TransportError::backend)
    }
}

/// Write relays from the newest valid NIP-65 list signed by `publisher`
/// (policy rule 5). Relays marked `write` or unmarked are used; `read` relays
/// are not. Invalid signatures, other authors, and URLs failing
/// [`validate_relay_url`] are ignored. At most [`MAX_RELAYS_PER_PUBLISHER`].
pub fn publisher_write_relays(events: &[Event], publisher: PublicKey) -> Vec<RelayUrl> {
    let Some(list) = events
        .iter()
        .filter(|event| {
            event.pubkey == publisher
                && event.kind == Kind::Custom(RELAY_LIST_KIND)
                && event.verify().is_ok()
        })
        .max_by(|a, b| a.created_at.cmp(&b.created_at).then(b.id.cmp(&a.id)))
    else {
        return Vec::new();
    };
    let mut relays = Vec::new();
    for tag in list.tags.iter() {
        let url = match tag.as_slice() {
            [name, url] if name == "r" => url,
            [name, url, marker, ..] if name == "r" && marker == "write" => url,
            _ => continue,
        };
        if validate_relay_url(url).is_err() {
            continue;
        }
        let Ok(relay) = RelayUrl::parse(url) else {
            continue;
        };
        if !relays.contains(&relay) {
            relays.push(relay);
        }
        if relays.len() == MAX_RELAYS_PER_PUBLISHER {
            break;
        }
    }
    relays
}

/// Publisher-preferred relays: a separate client using [`PolicyTransport`],
/// with no automatic reconnection and bounded size.
pub struct PublisherRelays {
    client: Client,
    held: Mutex<VecDeque<RelayUrl>>,
    lists: Mutex<HashMap<PublicKey, (Instant, Vec<RelayUrl>)>>,
}

impl PublisherRelays {
    /// Production configuration.
    pub fn new() -> Self {
        Self::with_transport(PolicyTransport::new())
    }

    /// Uses a specific transport (tests inject DNS, TCP and TLS).
    #[doc(hidden)]
    pub fn with_transport(transport: PolicyTransport) -> Self {
        Self {
            client: Client::builder().websocket_transport(transport).build(),
            held: Mutex::new(VecDeque::new()),
            lists: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn client(&self) -> Client {
        self.client.clone()
    }

    /// A cached relay list for `publisher`, if still fresh.
    pub(crate) fn cached_list(&self, publisher: &PublicKey) -> Option<Vec<RelayUrl>> {
        let lists = self.lists.lock().ok()?;
        lists
            .get(publisher)
            .filter(|(at, _)| at.elapsed() < RELAY_LIST_TTL)
            .map(|(_, relays)| relays.clone())
    }

    pub(crate) fn cache_list(&self, publisher: PublicKey, relays: Vec<RelayUrl>) {
        if let Ok(mut lists) = self.lists.lock() {
            if lists.len() >= MAX_CACHED_PUBLISHERS {
                lists.retain(|_, (at, _)| at.elapsed() < RELAY_LIST_TTL);
                if lists.len() >= MAX_CACHED_PUBLISHERS {
                    lists.clear();
                }
            }
            lists.insert(publisher, (Instant::now(), relays));
        }
    }

    /// Adds relays to the pool, evicting the oldest beyond
    /// [`MAX_PREFERRED_RELAYS_TOTAL`], and makes one bounded connection
    /// attempt for each new relay. Failures are ignored (policy rule 8).
    pub(crate) async fn ensure(&self, relays: &[RelayUrl]) {
        let mut added = Vec::new();
        let mut evicted = Vec::new();
        if let Ok(mut held) = self.held.lock() {
            for relay in relays.iter().take(MAX_RELAYS_PER_PUBLISHER) {
                if held.contains(relay) {
                    continue;
                }
                held.push_back(relay.clone());
                added.push(relay.clone());
            }
            while held.len() > MAX_PREFERRED_RELAYS_TOTAL {
                if let Some(oldest) = held.pop_front() {
                    evicted.push(oldest);
                }
            }
        }
        for relay in evicted {
            let _ = self.client.remove_relay(relay).await;
        }
        let options = RelayOptions::default().reconnect(false);
        for relay in &added {
            if let Err(error) = self
                .client
                .pool()
                .add_relay(relay.clone(), options.clone())
                .await
            {
                tracing::debug!(%relay, %error, "publisher relay not added");
            }
        }
        let attempts = added.iter().map(|relay| {
            let client = self.client.clone();
            async move {
                let _ = client
                    .try_connect_relay(relay.clone(), CONNECT_TIMEOUT)
                    .await;
            }
        });
        async_wsocket::futures_util::future::join_all(attempts).await;
    }

    /// Number of relays currently held. For tests and diagnostics.
    pub fn held_count(&self) -> usize {
        self.held.lock().map(|held| held.len()).unwrap_or(0)
    }
}

impl Default for PublisherRelays {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn only_wss_urls_without_credentials_or_fragments_are_accepted() {
        assert!(validate_relay_url("wss://relay.example.com").is_ok());
        assert!(validate_relay_url("wss://relay.example.com:7447/path").is_ok());
        for (raw, rejection) in [
            ("ws://relay.example.com", RelayRejection::NotWss),
            ("https://relay.example.com", RelayRejection::NotWss),
            ("file:///etc/passwd", RelayRejection::NotWss),
            ("wss://user@relay.example.com", RelayRejection::Credentials),
            (
                "wss://user:pass@relay.example.com",
                RelayRejection::Credentials,
            ),
            ("wss://relay.example.com/#frag", RelayRejection::Fragment),
            ("not a url", RelayRejection::Malformed),
            ("wss://", RelayRejection::Malformed),
            ("wss://exa mple.com", RelayRejection::Malformed),
        ] {
            assert_eq!(validate_relay_url(raw), Err(rejection), "{raw}");
        }
    }

    #[test]
    fn non_public_ip_literals_are_rejected_in_every_spelling() {
        for raw in [
            "wss://127.0.0.1",
            "wss://2130706433",   // decimal 127.0.0.1
            "wss://0x7f.1",       // hex/short 127.0.0.1
            "wss://017700000001", // octal 127.0.0.1
            "wss://10.1.2.3",
            "wss://169.254.169.254",
            "wss://0.0.0.0",
            "wss://[::1]",
            "wss://[::]",
            "wss://[::ffff:127.0.0.1]",
            "wss://[::ffff:7f00:1]",
            "wss://[::ffff:192.168.1.7]",
            "wss://[fe80::1]",
            "wss://[fc00::1]",
            "wss://[fd12:3456::1]",
            "wss://[ff02::1]",
            "wss://[64:ff9b::7f00:1]",
            "wss://[2002:7f00:1::]",
            "wss://[2001::1]",
        ] {
            assert!(
                matches!(
                    validate_relay_url(raw),
                    Err(RelayRejection::NonPublicAddress(_))
                ),
                "{raw}: {:?}",
                validate_relay_url(raw)
            );
        }
    }

    #[test]
    fn address_classification() {
        let non_public = [
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.7",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a00:1",
            "fc00::1",
            "fdff::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001::1",
            "2001:db8::1",
            "2002:a00:1::",
            "3fff::1",
            "100::1",
        ];
        for ip in non_public {
            assert!(
                !is_public_ip(ip.parse().unwrap()),
                "{ip} must not be public"
            );
        }
        let public = [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "172.32.0.1",
            "100.128.0.1",
            "2606:4700:4700::1111",
            "2a00:1450:4001::1",
            "::ffff:1.1.1.1",
            "64:ff9b::101:101",
        ];
        for ip in public {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip} must be public");
        }
    }

    struct StaticResolver {
        answers: Mutex<VecDeque<Vec<SocketAddr>>>,
        calls: AtomicUsize,
    }

    impl StaticResolver {
        fn new(answers: Vec<Vec<&str>>) -> Arc<Self> {
            Arc::new(Self {
                answers: Mutex::new(
                    answers
                        .into_iter()
                        .map(|a| a.into_iter().map(|s| s.parse().unwrap()).collect())
                        .collect(),
                ),
                calls: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl Resolver for StaticResolver {
        async fn resolve(&self, _host: &str, _port: u16) -> io::Result<Vec<SocketAddr>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.answers.lock().unwrap().pop_front().unwrap_or_default())
        }
    }

    /// Records dialed addresses and refuses the connection.
    #[derive(Default)]
    struct RecordingDialer {
        dialed: Mutex<Vec<SocketAddr>>,
    }

    #[async_trait]
    impl Dialer for RecordingDialer {
        async fn dial(&self, addr: SocketAddr) -> io::Result<TcpStream> {
            self.dialed.lock().unwrap().push(addr);
            Err(io::Error::new(io::ErrorKind::ConnectionRefused, "test"))
        }
    }

    #[tokio::test]
    async fn a_mixed_dns_answer_is_rejected_without_dialing() {
        let resolver = StaticResolver::new(vec![vec!["93.184.216.34:443", "127.0.0.1:443"]]);
        let dialer = Arc::new(RecordingDialer::default());
        let transport = PolicyTransport::with_network(resolver, dialer.clone(), None);
        let result = transport
            .connect_checked("wss://relay.example.com", CONNECT_TIMEOUT)
            .await;
        assert!(matches!(result, Err(RelayRejection::NonPublicAddress(_))));
        assert!(dialer.dialed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_private_dns_answer_is_rejected_without_dialing() {
        for answer in [
            "10.0.0.5:443",
            "[::1]:443",
            "[::ffff:127.0.0.1]:443",
            "169.254.169.254:443",
        ] {
            let resolver = StaticResolver::new(vec![vec![answer]]);
            let dialer = Arc::new(RecordingDialer::default());
            let transport = PolicyTransport::with_network(resolver, dialer.clone(), None);
            assert!(matches!(
                transport
                    .connect_checked("wss://relay.example.com", CONNECT_TIMEOUT)
                    .await,
                Err(RelayRejection::NonPublicAddress(_))
            ));
            assert!(dialer.dialed.lock().unwrap().is_empty(), "{answer}");
        }
    }

    /// DNS rebinding: the first answer is public, a second would be private.
    /// The transport resolves once and dials only the validated address.
    #[tokio::test]
    async fn dns_rebinding_cannot_redirect_the_connection() {
        let resolver = StaticResolver::new(vec![vec!["93.184.216.34:443"], vec!["127.0.0.1:443"]]);
        let dialer = Arc::new(RecordingDialer::default());
        let transport = PolicyTransport::with_network(resolver.clone(), dialer.clone(), None);
        let _ = transport
            .connect_checked("wss://rebind.example.com", CONNECT_TIMEOUT)
            .await;
        assert_eq!(
            resolver.calls.load(Ordering::SeqCst),
            1,
            "resolved exactly once"
        );
        assert_eq!(
            *dialer.dialed.lock().unwrap(),
            vec!["93.184.216.34:443".parse::<SocketAddr>().unwrap()]
        );
    }

    /// A dialer that never completes, to hold connection slots.
    struct HangingDialer;

    #[async_trait]
    impl Dialer for HangingDialer {
        async fn dial(&self, _addr: SocketAddr) -> io::Result<TcpStream> {
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn concurrent_connection_attempts_are_bounded() {
        let transport = Arc::new(PolicyTransport::with_network(
            StaticResolver::new(vec![vec!["93.184.216.34:443"]; MAX_CONCURRENT_DIALS + 1]),
            Arc::new(HangingDialer),
            None,
        ));
        let mut pending = Vec::new();
        for _ in 0..MAX_CONCURRENT_DIALS {
            let transport = transport.clone();
            pending.push(tokio::spawn(async move {
                transport
                    .connect_checked("wss://slow.example.com", Duration::from_secs(30))
                    .await
            }));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            transport
                .connect_checked("wss://one-more.example.com", Duration::from_secs(30))
                .await
                .err(),
            Some(RelayRejection::ConnectionLimit)
        );
        for task in pending {
            task.abort();
        }
    }

    #[tokio::test]
    async fn a_hanging_connection_times_out() {
        let transport = PolicyTransport::with_network(
            StaticResolver::new(vec![vec!["93.184.216.34:443"]]),
            Arc::new(HangingDialer),
            None,
        );
        assert_eq!(
            transport
                .connect_checked("wss://slow.example.com", Duration::from_millis(50))
                .await
                .err(),
            Some(RelayRejection::Timeout)
        );
    }

    fn relay_list(keys: &nostr_sdk::Keys, tags: Vec<Vec<&str>>, at: u64) -> Event {
        nostr_sdk::EventBuilder::new(Kind::Custom(RELAY_LIST_KIND), "")
            .tags(tags.into_iter().map(|t| nostr_sdk::Tag::parse(t).unwrap()))
            .custom_created_at(nostr_sdk::Timestamp::from_secs(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    #[test]
    fn relay_lists_use_newest_valid_publisher_signed_write_relays() {
        let publisher = nostr_sdk::Keys::generate();
        let older = relay_list(&publisher, vec![vec!["r", "wss://old.example.com"]], 100);
        let newer = relay_list(
            &publisher,
            vec![
                vec!["r", "wss://write.example.com", "write"],
                vec!["r", "wss://unmarked.example.com"],
                vec!["r", "wss://read.example.com", "read"],
                vec!["r", "ws://plain.example.com"],
                vec!["r", "wss://127.0.0.1"],
                vec!["r", "wss://user:pw@creds.example.com"],
                vec!["r", "wss://write.example.com", "write"],
            ],
            200,
        );
        let foreign = relay_list(
            &nostr_sdk::Keys::generate(),
            vec![vec!["r", "wss://foreign.example.com"]],
            300,
        );
        let mut forged = relay_list(&publisher, vec![vec!["r", "wss://forged.example.com"]], 400);
        forged.content = "tampered".into();

        let relays =
            publisher_write_relays(&[older, newer, foreign, forged], publisher.public_key());
        let relays: Vec<String> = relays.iter().map(|r| r.to_string()).collect();
        assert_eq!(
            relays,
            vec!["wss://write.example.com", "wss://unmarked.example.com"]
        );
    }

    #[test]
    fn at_most_five_relays_are_taken_from_one_list() {
        let publisher = nostr_sdk::Keys::generate();
        let urls: Vec<String> = (0..12).map(|i| format!("wss://r{i}.example.com")).collect();
        let tags = urls.iter().map(|u| vec!["r", u.as_str()]).collect();
        let list = relay_list(&publisher, tags, 100);
        assert_eq!(
            publisher_write_relays(&[list], publisher.public_key()).len(),
            MAX_RELAYS_PER_PUBLISHER
        );
    }

    #[tokio::test]
    async fn the_preferred_pool_never_exceeds_its_global_cap() {
        let relays = PublisherRelays::with_transport(PolicyTransport::with_network(
            StaticResolver::new(vec![]),
            Arc::new(RecordingDialer::default()),
            None,
        ));
        for publisher in 0..10 {
            let urls: Vec<RelayUrl> = (0..MAX_RELAYS_PER_PUBLISHER)
                .map(|i| RelayUrl::parse(&format!("wss://p{publisher}r{i}.example.com")).unwrap())
                .collect();
            relays.ensure(&urls).await;
            assert!(relays.held_count() <= MAX_PREFERRED_RELAYS_TOTAL);
        }
        assert_eq!(relays.held_count(), MAX_PREFERRED_RELAYS_TOTAL);
        assert!(relays.client().relays().await.len() <= MAX_PREFERRED_RELAYS_TOTAL);
    }
}
