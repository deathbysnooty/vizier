//! Fetching a picture from a URL somebody typed - the dangerous part.
//!
//! The rest of the toolkit only bends pixels. This module takes an address from
//! a member of the server and makes the bot open it, which means the bot can be
//! aimed at anything the bot can reach: its own admin ports, the panel on
//! localhost, another machine on the same network, or a cloud provider's
//! metadata service, which hands out credentials to anyone who asks from the
//! right address. That is server-side request forgery, and it is the one thing
//! in here that can lose something that matters.
//!
//! So, in order:
//!
//! 1. **The scheme.** `http` and `https` and nothing else - no `file:`, no
//!    `gopher:`, no `data:`.
//! 2. **The address.** The host is resolved *before* the request, and every
//!    address it resolves to must be a public one: no loopback, no private
//!    range, no link-local (which is where `169.254.169.254` lives), no
//!    carrier-grade NAT, no multicast, no IPv4-mapped IPv6 sneaking a private
//!    address past an IPv6 check.
//! 3. **The connection is pinned** to the addresses that were checked
//!    ([`reqwest::ClientBuilder::resolve_to_addrs`]), so a name that resolves
//!    to something public now and something private a moment later - DNS
//!    rebinding - cannot be used to slip through between the check and the
//!    connection.
//! 4. **Redirects are followed by hand**, at most [`MAX_HOPS`] of them, with
//!    every step going through 1-3 again. reqwest's own redirect following
//!    would do the second request without any of these checks.
//! 5. **The size and the time are capped**, by reading the body a chunk at a
//!    time and stopping the moment it goes over, so a server that promises a
//!    kilobyte and sends a gigabyte gets cut off.
//! 6. **The bytes are a picture**, by their own magic numbers and not by what
//!    the server said they were.
//!
//! Every refusal is a variant of [`Refusal`], and every one of them is tested.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use reqwest::Url;

/// The most redirects one fetch will follow.
pub const MAX_HOPS: usize = 3;

/// Why the bot would not fetch that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not http or https.
    Scheme,
    /// No host to resolve, or one that cannot be parsed.
    NoHost,
    /// The host is - or resolves to - an address the bot will not open.
    Private,
    /// The host does not resolve at all.
    Unknown,
    /// Over the byte cap, promised or delivered.
    TooBig,
    /// The server took longer than allowed.
    TooSlow,
    /// Too many redirects.
    Bounced,
    /// The server said no.
    Status(u16),
    /// What came back is not a picture.
    NotImage,
    /// The connection itself failed; the detail is for the log.
    Offline(String),
}

impl Refusal {
    /// The sentence the member reads. Deliberately vague about the network: a
    /// member does not need to learn which of our addresses are private.
    pub fn plainly(&self) -> String {
        match self {
            Refusal::Scheme => "That link has to start with http:// or https://.".into(),
            Refusal::NoHost | Refusal::Unknown => "I couldn't make sense of that link.".into(),
            Refusal::Private => "I won't open that address.".into(),
            Refusal::TooBig => "That file is too big for me to fetch.".into(),
            Refusal::TooSlow => "That link took too long to answer.".into(),
            Refusal::Bounced => "That link redirects too many times.".into(),
            Refusal::Status(code) => format!("That link answered with a {code}."),
            Refusal::NotImage => "That link isn't a picture - PNG, JPEG, WEBP or GIF only.".into(),
            Refusal::Offline(_) => "I couldn't reach that link.".into(),
        }
    }
}

// --- which addresses are allowed ---------------------------------------------------------------

/// Whether the bot may open a connection to this address.
///
/// Public means public: anything that is loopback, private, link-local,
/// carrier-grade NAT, multicast, broadcast, unspecified, reserved, or an IPv6
/// unique-local or mapped-IPv4 wrapper around any of those, is refused. The
/// cloud metadata endpoint `169.254.169.254` is link-local, which is why that
/// whole range goes.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            if v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_multicast() || v4.is_broadcast() || v4.is_unspecified() {
                return false;
            }
            // 0.0.0.0/8 - "this network", and on some systems another way of
            // saying localhost.
            if o[0] == 0 {
                return false;
            }
            // 100.64.0.0/10 - carrier-grade NAT, which is a private network
            // wearing a public-looking address.
            if o[0] == 100 && (64..128).contains(&o[1]) {
                return false;
            }
            // 192.0.0.0/24, 192.0.2.0/24, 198.18.0.0/15, 198.51.100.0/24,
            // 203.0.113.0/24 - IETF protocol assignments and the documentation
            // ranges, none of which is a real picture host.
            if o[0] == 192 && o[1] == 0 && (o[2] == 0 || o[2] == 2) {
                return false;
            }
            if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
                return false;
            }
            if o[0] == 198 && o[1] == 51 && o[2] == 100 {
                return false;
            }
            if o[0] == 203 && o[1] == 0 && o[2] == 113 {
                return false;
            }
            // 240.0.0.0/4 - reserved.
            if o[0] >= 240 {
                return false;
            }
            true
        }
        IpAddr::V6(v6) => {
            // An IPv4 address written as IPv6 is still that IPv4 address.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            if v6.is_loopback() || v6.is_multicast() || v6.is_unspecified() {
                return false;
            }
            let s = v6.segments();
            // fc00::/7 unique local, fe80::/10 link local.
            if s[0] & 0xfe00 == 0xfc00 || s[0] & 0xffc0 == 0xfe80 {
                return false;
            }
            // 64:ff9b::/96 - NAT64, another way to reach an IPv4 address.
            if s[0] == 0x0064 && s[1] == 0xff9b {
                return v6.to_ipv4().is_some_and(|v4| is_public(IpAddr::V4(v4)));
            }
            // ::ffff:0:0/96 handled above; a bare ::x is the compatible range.
            if s[..5] == [0, 0, 0, 0, 0] {
                return false;
            }
            // 2001:db8::/32 - documentation.
            if s[0] == 0x2001 && s[1] == 0x0db8 {
                return false;
            }
            true
        }
    }
}

/// The scheme and host checks, which need no network and so can be tested on
/// their own. A host that is written as a literal address is checked here and
/// now; a name has to wait for [`resolve`].
pub fn check_url(raw: &str) -> Result<Url, Refusal> {
    let url = Url::parse(raw.trim()).map_err(|_| Refusal::NoHost)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Refusal::Scheme);
    }
    let host = url.host_str().ok_or(Refusal::NoHost)?;
    if host.is_empty() {
        return Err(Refusal::NoHost);
    }
    // Written as an address rather than a name: no lookup needed, and no
    // lookup wanted - this is the shape most attempts take.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        if !is_public(ip) {
            return Err(Refusal::Private);
        }
        return Ok(url);
    }
    if !plausible_host(bare) {
        return Err(Refusal::NoHost);
    }
    Ok(url)
}

/// Whether a *name* is one a picture could live behind. A name with no dot in
/// it is an intranet name - `http://wiki/`, `http://router/` - and so are the
/// internal-only suffixes below; all of them point inside whatever network the
/// bot is on, which is exactly what the address checks exist to keep it out of.
/// The lookup in [`resolve`] would catch most of these anyway; this catches
/// them without a lookup, and catches the ones that resolve through a split
/// horizon to something that only looks public.
pub fn plausible_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || !host.contains('.') {
        return false;
    }
    if host.contains(char::is_whitespace) || host.contains('_') {
        return false;
    }
    !["localhost", "local", "internal", "intranet", "lan", "home", "corp", "home.arpa"]
        .iter()
        .any(|bad| host == *bad || host.ends_with(&format!(".{bad}")))
}

/// Every address a host resolves to, all of them public or none of them used.
pub async fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, Refusal> {
    let bare = host.trim_start_matches('[').trim_end_matches(']').to_string();
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return if is_public(ip) { Ok(vec![SocketAddr::new(ip, port)]) } else { Err(Refusal::Private) };
    }
    if !plausible_host(&bare) {
        return Err(Refusal::Private);
    }
    let found: Vec<SocketAddr> = tokio::net::lookup_host((bare.as_str(), port)).await.map_err(|_| Refusal::Unknown)?.collect();
    if found.is_empty() {
        return Err(Refusal::Unknown);
    }
    if !all_public(&found) {
        return Err(Refusal::Private);
    }
    Ok(found)
}

// --- what the server said ----------------------------------------------------------------------

/// Whether every address a name resolved to is one the bot may open. One private
/// answer condemns the whole name: a host that resolves to both a public and a
/// private address is a host being used to reach the private one, and which of
/// the two a connection lands on is not ours to choose.
pub fn all_public(addrs: &[SocketAddr]) -> bool {
    !addrs.is_empty() && addrs.iter().all(|a| is_public(a.ip()))
}

/// The headers check: the server has to claim an image, and has to claim it
/// fits. Both are only claims - [`super::imagefx_engine::sniff`] has the last
/// word - but a server that admits to sending a gigabyte of HTML can be turned
/// away before a byte of it arrives.
pub fn check_headers(content_type: Option<&str>, length: Option<u64>, cap: usize) -> Result<(), Refusal> {
    if let Some(n) = length {
        if n > cap as u64 {
            return Err(Refusal::TooBig);
        }
    }
    match content_type {
        // No type at all is allowed through: plenty of CDNs omit it, and the
        // magic-number check still stands behind this.
        None => Ok(()),
        Some(kind) => {
            let kind = kind.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            if kind.is_empty() || kind == "application/octet-stream" || kind == "binary/octet-stream" {
                return Ok(());
            }
            if kind.starts_with("image/") { Ok(()) } else { Err(Refusal::NotImage) }
        }
    }
}

/// Read chunks until they run out, or until they go over the cap. Split out
/// from the request so the cap can be tested without a server to talk to.
pub fn take_capped<E>(chunks: impl IntoIterator<Item = Result<Vec<u8>, E>>, cap: usize) -> Result<Vec<u8>, Refusal>
where
    E: std::fmt::Display,
{
    let mut out: Vec<u8> = Vec::new();
    for chunk in chunks {
        let chunk = chunk.map_err(|e| Refusal::Offline(e.to_string()))?;
        if out.len() + chunk.len() > cap {
            return Err(Refusal::TooBig);
        }
        out.extend_from_slice(&chunk);
    }
    if out.is_empty() { Err(Refusal::NotImage) } else { Ok(out) }
}

/// Where a redirect points, resolved against the page it came from. A redirect
/// to something that is not http(s) is a refusal, not a follow.
pub fn next_hop(from: &Url, location: &str) -> Result<Url, Refusal> {
    let joined = from.join(location.trim()).map_err(|_| Refusal::NoHost)?;
    if !matches!(joined.scheme(), "http" | "https") {
        return Err(Refusal::Scheme);
    }
    Ok(joined)
}

// --- the fetch itself --------------------------------------------------------------------------

/// One picture from a URL a member typed, with every check above applied at
/// every hop.
pub async fn get(raw: &str, cap: usize, wait: Duration) -> Result<Vec<u8>, Refusal> {
    let mut url = check_url(raw)?;
    for _hop in 0..=MAX_HOPS {
        let host = url.host_str().ok_or(Refusal::NoHost)?.to_string();
        let port = url.port_or_known_default().unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
        let addrs = resolve(&host, port).await?;
        let client = reqwest::Client::builder()
            // Followed by hand, so every hop gets checked.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(wait)
            .connect_timeout(wait.min(Duration::from_secs(5)))
            // Pinned to what was just checked: no room between the check and
            // the connection for the name to change its mind.
            .resolve_to_addrs(&host, &addrs)
            .build()
            .map_err(|e| Refusal::Offline(e.to_string()))?;
        let mut resp = match client.get(url.clone()).send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => return Err(Refusal::TooSlow),
            Err(e) => return Err(Refusal::Offline(e.to_string())),
        };
        let status = resp.status();
        if status.is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(Refusal::Status(status.as_u16()))?
                .to_string();
            url = next_hop(&url, &location)?;
            continue;
        }
        if !status.is_success() {
            return Err(Refusal::Status(status.as_u16()));
        }
        let kind = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(String::from);
        check_headers(kind.as_deref(), resp.content_length(), cap)?;
        let mut body: Vec<u8> = Vec::new();
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > cap {
                        return Err(Refusal::TooBig);
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(e) if e.is_timeout() => return Err(Refusal::TooSlow),
                Err(e) => return Err(Refusal::Offline(e.to_string())),
            }
        }
        // The server's word counts for nothing; the bytes' own header decides.
        if super::imagefx_engine::sniff(&body).is_none() {
            return Err(Refusal::NotImage);
        }
        return Ok(body);
    }
    Err(Refusal::Bounced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_addresses_the_bot_must_never_open() {
        for bad in [
            // Loopback, in both families and both spellings.
            "127.0.0.1",
            "127.1.2.3",
            "0.0.0.0",
            "0.1.2.3",
            "::1",
            "::ffff:127.0.0.1",
            // Private networks.
            "10.0.0.1",
            "10.255.255.254",
            "172.16.0.1",
            "172.31.255.1",
            "192.168.0.1",
            "192.168.31.7",
            "::ffff:10.0.0.1",
            "fd00::1",
            "fc00::1234",
            // Link local - and with it the cloud metadata service, which is
            // the single most valuable thing an SSRF can reach.
            "169.254.169.254",
            "169.254.0.1",
            "fe80::1",
            // Carrier-grade NAT.
            "100.64.0.1",
            "100.127.255.255",
            // Multicast, broadcast, reserved, documentation.
            "224.0.0.1",
            "239.1.2.3",
            "255.255.255.255",
            "240.0.0.1",
            "192.0.2.5",
            "198.18.0.1",
            "198.51.100.9",
            "203.0.113.4",
            "2001:db8::1",
            "ff02::1",
            "::",
        ] {
            let ip: IpAddr = bad.parse().expect(bad);
            assert!(!is_public(ip), "{} was allowed", bad);
        }
    }

    #[test]
    fn ordinary_public_addresses_are_allowed() {
        for good in ["8.8.8.8", "1.1.1.1", "162.159.135.232", "104.16.0.1", "2606:4700::1111", "2a03:2880:f10c::1"] {
            let ip: IpAddr = good.parse().expect(good);
            assert!(is_public(ip), "{} was refused", good);
        }
    }

    #[test]
    fn only_http_and_https_get_through() {
        for bad in [
            "file:///etc/passwd",
            "ftp://example.com/a.png",
            "gopher://example.com/a.png",
            "data:image/png;base64,iVBORw0KGgo=",
            "ws://example.com/a.png",
        ] {
            assert_eq!(check_url(bad).err(), Some(Refusal::Scheme), "{} got through", bad);
        }
        assert!(check_url("https://cdn.discordapp.com/a.png").is_ok());
        assert!(check_url("http://example.com/a.png").is_ok());
    }

    #[test]
    fn an_address_written_into_the_link_is_refused_without_a_lookup() {
        for bad in [
            "http://127.0.0.1/x.png",
            "http://127.0.0.1:8080/panel",
            "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
            "http://[::1]:3000/x.png",
            "http://10.0.0.5/x.png",
            "http://192.168.1.1/x.png",
            "http://[fd00::1]/x.png",
            "http://0.0.0.0:9000/",
            "https://100.64.3.4/x.png",
        ] {
            assert_eq!(check_url(bad).err(), Some(Refusal::Private), "{} got through", bad);
        }
    }

    #[test]
    fn nonsense_is_not_a_link() {
        for bad in ["", "   ", "not a url", "http://", "https:///path"] {
            assert!(check_url(bad).is_err(), "{:?} got through", bad);
        }
    }

    /// An intranet name is a private address wearing a name. A dotless host is
    /// always one of these, and so are the internal-only suffixes.
    #[test]
    fn an_intranet_name_is_refused_without_a_lookup() {
        for bad in [
            "http://localhost/x.png",
            "http://localhost:3000/panel",
            "http://wiki/x.png",
            "http://router/x.png",
            "http://vizier.local/x.png",
            "http://db.internal/x.png",
            "http://files.lan/x.png",
            "http://nas.home/x.png",
            "http://thing.home.arpa/x.png",
            "http://app.localhost/x.png",
        ] {
            assert_eq!(check_url(bad).err(), Some(Refusal::NoHost), "{} got through", bad);
        }
        assert!(check_url("https://cdn.discordapp.com/x.png").is_ok());
        assert!(check_url("https://i.imgur.com/x.png").is_ok());
        assert!(check_url("https://media.tenor.co.uk/x.gif").is_ok());
    }

    #[test]
    fn what_counts_as_a_host_worth_looking_up() {
        for good in ["example.com", "a.b.c.example.com", "EXAMPLE.COM", "example.com."] {
            assert!(plausible_host(good), "{} was refused", good);
        }
        for bad in ["", "localhost", "wiki", "x y.com", "under_score.com", "box.local", "svc.internal"] {
            assert!(!plausible_host(bad), "{} was allowed", bad);
        }
    }

    /// `localhost` is a name, not an address, and the check has to catch it
    /// after the lookup. This is the one network-shaped test that works with
    /// no network: every machine resolves it from its own hosts file.
    #[tokio::test]
    async fn localhost_is_refused_after_it_resolves() {
        assert_eq!(resolve("localhost", 80).await.err(), Some(Refusal::Private));
        assert_eq!(resolve("127.0.0.1", 80).await.err(), Some(Refusal::Private));
        assert_eq!(resolve("::1", 80).await.err(), Some(Refusal::Private));
        // And the whole fetch stops before it opens a socket. Which of the two
        // refusals it is depends on how far it got; neither connects.
        let err = get("http://localhost:1/x.png", 1024, Duration::from_millis(200)).await.err();
        assert!(matches!(err, Some(Refusal::NoHost) | Some(Refusal::Private)), "localhost was fetched: {:?}", err);
        let err = get("http://127.0.0.1:1/x.png", 1024, Duration::from_millis(200)).await.err();
        assert_eq!(err, Some(Refusal::Private));
        let err = get("http://169.254.169.254/latest/meta-data/", 1024, Duration::from_millis(200)).await.err();
        assert_eq!(err, Some(Refusal::Private), "the metadata service must never be reachable");
    }

    /// The split-answer case: a name that resolves to a public address *and* a
    /// private one is refused outright, because which one a connection lands on
    /// is not ours to pick.
    #[test]
    fn one_private_answer_condemns_the_whole_name() {
        let public: SocketAddr = "8.8.8.8:80".parse().expect("addr");
        let private: SocketAddr = "10.0.0.5:80".parse().expect("addr");
        let loopback: SocketAddr = "127.0.0.1:80".parse().expect("addr");
        assert!(all_public(&[public]));
        assert!(all_public(&[public, "1.1.1.1:80".parse().expect("addr")]));
        assert!(!all_public(&[public, private]));
        assert!(!all_public(&[private, public]));
        assert!(!all_public(&[loopback]));
        assert!(!all_public(&[]), "no answer is not a good answer");
    }

    #[tokio::test]
    async fn a_name_that_does_not_exist_is_refused_not_retried() {
        let host = "no-such-host-for-the-image-toolkit.invalid";
        assert_eq!(resolve(host, 80).await.err(), Some(Refusal::Unknown));
    }

    #[test]
    fn a_body_over_the_cap_is_cut_off() {
        let chunks: Vec<Result<Vec<u8>, String>> = vec![Ok(vec![0u8; 400]), Ok(vec![0u8; 400]), Ok(vec![0u8; 400])];
        assert_eq!(take_capped(chunks.clone(), 1000).err(), Some(Refusal::TooBig));
        assert_eq!(take_capped(chunks, 2000).map(|b| b.len()), Ok(1200));
        // Nothing at all is not a picture either.
        let empty: Vec<Result<Vec<u8>, String>> = vec![];
        assert_eq!(take_capped(empty, 100).err(), Some(Refusal::NotImage));
        // A broken connection half way through is a failure, not a short file.
        let broken: Vec<Result<Vec<u8>, String>> = vec![Ok(vec![1u8; 10]), Err("reset".to_string())];
        assert!(matches!(take_capped(broken, 100).err(), Some(Refusal::Offline(_))));
    }

    #[test]
    fn a_promised_size_over_the_cap_is_refused_before_the_body() {
        assert_eq!(check_headers(Some("image/png"), Some(9_000_000), 8_000_000).err(), Some(Refusal::TooBig));
        assert!(check_headers(Some("image/png"), Some(1_000), 8_000_000).is_ok());
    }

    #[test]
    fn what_the_server_calls_it_has_to_be_an_image() {
        for bad in ["text/html", "application/json", "text/plain; charset=utf-8", "video/mp4", "application/pdf"] {
            assert_eq!(check_headers(Some(bad), None, 1000).err(), Some(Refusal::NotImage), "{} got through", bad);
        }
        for good in ["image/png", "image/jpeg", "IMAGE/GIF", "image/webp; charset=binary", "application/octet-stream"] {
            assert!(check_headers(Some(good), None, 1000).is_ok(), "{} was refused", good);
        }
        // A CDN that says nothing is still allowed - the magic numbers decide.
        assert!(check_headers(None, None, 1000).is_ok());
    }

    #[test]
    fn a_redirect_is_resolved_and_kept_to_http() {
        let from = Url::parse("https://example.com/a/b.png").expect("url");
        assert_eq!(next_hop(&from, "/c/d.png").expect("hop").as_str(), "https://example.com/c/d.png");
        assert_eq!(next_hop(&from, "https://other.example/e.png").expect("hop").host_str(), Some("other.example"));
        assert_eq!(next_hop(&from, "file:///etc/passwd").err(), Some(Refusal::Scheme));
        // A redirect to a private address still has to pass check_url when the
        // loop comes round; the hop itself only checks the scheme.
        let hop = next_hop(&from, "http://127.0.0.1/x.png").expect("hop");
        assert_eq!(check_url(hop.as_str()).err(), Some(Refusal::Private));
    }

    #[test]
    fn every_refusal_has_something_to_say_and_gives_nothing_away() {
        for r in [
            Refusal::Scheme,
            Refusal::NoHost,
            Refusal::Private,
            Refusal::Unknown,
            Refusal::TooBig,
            Refusal::TooSlow,
            Refusal::Bounced,
            Refusal::Status(404),
            Refusal::NotImage,
            Refusal::Offline("connection refused to 10.0.0.1".into()),
        ] {
            let said = r.plainly();
            assert!(!said.trim().is_empty(), "{:?} says nothing", r);
            assert!(!said.contains("10.0.0.1"), "{:?} leaked the address", r);
        }
    }
}
