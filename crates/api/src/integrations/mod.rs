//! Compatibility facade for the external integration provider library.

pub use gitadel_integrations::*;

// API unit tests use isolated loopback servers. The dependency is built without
// cfg(test), so select its transport explicitly in this test-only registry.
#[cfg(test)]
static TEST_DOKPLOY: DokployIntegration = DokployIntegration::with_client_factory(|base| {
    let url = url::Url::parse(base).map_err(|error| error.to_string())?;
    match url.host() {
        Some(url::Host::Ipv4(address)) if address.is_loopback() => {}
        Some(url::Host::Ipv6(address)) if address.is_loopback() => {}
        _ => return Err("API mock transport requires a loopback IP address".to_owned()),
    }
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())
});

#[cfg(test)]
pub static PROVIDERS: &[Provider] = &[Provider {
    integration: &TEST_DOKPLOY,
    ..gitadel_integrations::PROVIDERS[0]
}];

#[cfg(test)]
pub fn provider(slug: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|provider| provider.slug == slug)
}
