// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! TLS crypto provider wiring on aws-lc-rs: [`install_default_provider`] sets the
//! process-wide rustls `CryptoProvider`; [`aws_http_client`] builds the HTTP client
//! injected into every AWS SDK config.

use aws_smithy_http_client::tls::{rustls_provider::CryptoMode, Provider};
use aws_smithy_http_client::Builder;
use aws_smithy_runtime_api::client::http::SharedHttpClient;

/// Install the default rustls crypto provider. Idempotent — a second call (or a
/// provider already installed elsewhere) is ignored.
pub fn install_default_provider() {
    #[cfg(feature = "fips")]
    let provider = rustls::crypto::default_fips_provider();
    #[cfg(not(feature = "fips"))]
    let provider = rustls::crypto::aws_lc_rs::default_provider();

    if provider.install_default().is_err() {
        tracing::debug!("tls: a rustls crypto provider was already installed");
    }
}

/// HTTPS client for the AWS SDK (S3, Bedrock), TLS via aws-lc-rs — the FIPS
/// provider under `fips`. Inject at every SDK config: `.http_client(...)`.
pub fn aws_http_client() -> SharedHttpClient {
    #[cfg(feature = "fips")]
    let mode = CryptoMode::AwsLcFips;
    #[cfg(not(feature = "fips"))]
    let mode = CryptoMode::AwsLc;

    Builder::new()
        .tls_provider(Provider::Rustls(mode))
        .build_https()
}
