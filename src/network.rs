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

pub async fn pinned_public_https_client(origin: &Url) -> Result<Client, String> {
    let host = match origin.host() {
        Some(Host::Domain(host)) => host.trim_end_matches('.'),
        _ => return Err("Git server URL must use a public DNS hostname.".to_owned()),
    };
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return Err("Git server URL must use a public DNS hostname.".to_owned());
    }
    let port = origin.port_or_known_default().unwrap_or(443);
    let addresses = lookup_host((host, port))
        .await
        .map_err(|error| format!("Could not resolve git server: {error}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(
            "Git server must not resolve to a private or reserved network address.".to_owned(),
        );
    }
    Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(20))
        .resolve(host, addresses[0])
        .build()
        .map_err(|error| format!("Could not prepare git server connection: {error}"))
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
