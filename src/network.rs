use std::{net::IpAddr, time::Duration};

use reqwest::{Client, redirect::Policy};
use tokio::net::lookup_host;
use url::{Host, Url};

pub fn normalize_public_https_origin(value: &str) -> Result<Url, String> {
    let mut url = Url::parse(value.trim()).map_err(|_| "Git server URL is invalid.".to_owned())?;
    if url.scheme() != "https"
        || !matches!(url.host(), Some(Host::Domain(_)))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(
            "Git server URL must be a public HTTPS origin without credentials, a path, query, or fragment."
                .to_owned(),
        );
    }
    url.set_path("");
    Ok(url)
}

pub async fn pinned_public_https_client(
    origin: &Url,
    timeout: Duration,
    redirects: Policy,
) -> Result<Client, String> {
    let host = match origin.host() {
        Some(Host::Domain(host)) => host,
        _ => return Err("Git server URL must use a public DNS hostname.".to_owned()),
    };
    let normalized_host = host.trim_end_matches('.');
    if origin.scheme() != "https"
        || normalized_host.eq_ignore_ascii_case("localhost")
        || normalized_host.to_ascii_lowercase().ends_with(".localhost")
    {
        return Err("Git server URL must use a public DNS hostname.".to_owned());
    }
    let port = origin.port_or_known_default().unwrap_or(443);
    let addresses = lookup_host((normalized_host, port))
        .await
        .map_err(|error| format!("Could not resolve git server: {error}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(
            "Git server must not resolve to a private or reserved network address.".to_owned(),
        );
    }
    Client::builder()
        .no_proxy()
        .redirect(redirects)
        .timeout(timeout)
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|error| format!("Could not prepare git server connection: {error}"))
}
pub async fn pinned_public_http_client(
    endpoint: &Url,
    timeout: Duration,
) -> Result<Client, String> {
    if let Some(Host::Domain(host)) = endpoint.host() {
        let normalized_host = host.trim_end_matches('.');
        if normalized_host.eq_ignore_ascii_case("localhost")
            || normalized_host.to_ascii_lowercase().ends_with(".localhost")
        {
            return Err("Endpoint must use a public network address.".to_owned());
        }
    }
    pinned_http_client(
        endpoint,
        timeout,
        is_public_ip,
        "Endpoint must use a public network address.",
    )
    .await
}

/// Build a client for a service that may live on a private network or VPN
/// mesh, but never on this host or a link-local (cloud metadata) address.
///
/// The hostname is resolved once and the client is pinned to those addresses,
/// so a later DNS answer cannot redirect the request elsewhere. Redirects are
/// not followed.
pub async fn pinned_service_http_client(
    endpoint: &Url,
    timeout: Duration,
) -> Result<Client, String> {
    pinned_http_client(
        endpoint,
        timeout,
        is_service_ip,
        "Endpoint must not use a loopback, link-local, or otherwise reserved address.",
    )
    .await
}

async fn pinned_http_client(
    endpoint: &Url,
    timeout: Duration,
    allowed: fn(IpAddr) -> bool,
    refusal: &str,
) -> Result<Client, String> {
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err("Endpoint must use HTTP or HTTPS.".to_owned());
    }
    let port = endpoint
        .port_or_known_default()
        .ok_or_else(|| "Endpoint port is invalid.".to_owned())?;
    let mut builder = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(timeout);
    match endpoint.host() {
        Some(Host::Domain(host)) => {
            let normalized_host = host.trim_end_matches('.');
            let addresses = lookup_host((normalized_host, port))
                .await
                .map_err(|error| format!("Could not resolve endpoint: {error}"))?
                .collect::<Vec<_>>();
            if addresses.is_empty() || addresses.iter().any(|address| !allowed(address.ip())) {
                return Err(refusal.to_owned());
            }
            builder = builder.resolve_to_addrs(host, &addresses);
        }
        Some(Host::Ipv4(address)) if allowed(IpAddr::V4(address)) => {}
        Some(Host::Ipv6(address)) if allowed(IpAddr::V6(address)) => {}
        _ => return Err(refusal.to_owned()),
    }
    builder
        .build()
        .map_err(|error| format!("Could not prepare endpoint connection: {error}"))
}

/// Whether an address may host a user-configured service: anything except
/// loopback, link-local, unspecified, multicast, and broadcast addresses.
/// Private ranges (RFC 1918, CGNAT, unique local) are deliberately allowed.
pub fn is_service_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !(address.octets()[0] == 0
                || address.is_loopback()
                || address.is_link_local()
                || address.is_multicast()
                || address.is_broadcast())
        }
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_service_ip(IpAddr::V4(mapped));
            }
            !(address.is_unspecified()
                || address.is_loopback()
                || address.is_unicast_link_local()
                || address.is_multicast())
        }
    }
}

pub fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let [first, second, third, _] = address.octets();
            !(first == 0
                || first == 10
                || first == 127
                || (first == 100 && (64..=127).contains(&second))
                || (first == 169 && second == 254)
                || (first == 172 && (16..=31).contains(&second))
                || (first == 192 && second == 0 && third == 0)
                || (first == 192 && second == 0 && third == 2)
                || (first == 192 && second == 88 && third == 99)
                || (first == 192 && second == 168)
                || (first == 198 && (second == 18 || second == 19))
                || (first == 198 && second == 51 && third == 100)
                || (first == 203 && second == 0 && third == 113)
                || first >= 224)
        }
        IpAddr::V6(address) => {
            let segments = address.segments();
            (segments[0] & 0xe000) == 0x2000
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && !(segments[0] == 0x2001 && segments[1] == 0)
                && segments[0] != 0x2002
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn public_http_client_rejects_non_public_literal_addresses() {
        for endpoint in [
            "http://127.0.0.1:8080/",
            "http://169.254.169.254/",
            "http://10.0.0.1/",
            "http://[::1]/",
        ] {
            let endpoint = Url::parse(endpoint).unwrap();
            assert!(
                pinned_public_http_client(&endpoint, Duration::from_secs(1))
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    fn service_addresses_allow_private_networks_but_not_this_host() {
        for allowed in [
            "10.0.0.5",
            "172.16.4.1",
            "192.168.1.20",
            "100.64.0.5",
            "8.8.8.8",
            "fd00::1",
            "2001:4860:4860::8888",
        ] {
            assert!(is_service_ip(allowed.parse().unwrap()), "{allowed}");
        }
        for refused in [
            "127.0.0.1",
            "127.8.9.1",
            "0.0.0.0",
            "0.1.2.3",
            "169.254.169.254",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(!is_service_ip(refused.parse().unwrap()), "{refused}");
        }
    }

    #[tokio::test]
    async fn service_http_client_rejects_loopback_and_metadata_endpoints() {
        for endpoint in [
            "http://127.0.0.1:8080/",
            "http://localhost:8080/",
            "http://169.254.169.254/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://0.0.0.0/",
            "ftp://10.0.0.1/",
        ] {
            let endpoint = Url::parse(endpoint).unwrap();
            assert!(
                pinned_service_http_client(&endpoint, Duration::from_secs(1))
                    .await
                    .is_err(),
                "{endpoint}"
            );
        }
        for endpoint in ["http://10.0.0.1:3000/", "https://100.64.0.5/"] {
            let endpoint = Url::parse(endpoint).unwrap();
            assert!(
                pinned_service_http_client(&endpoint, Duration::from_secs(1))
                    .await
                    .is_ok(),
                "{endpoint}"
            );
        }
    }
}
