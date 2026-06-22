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

/// Build a rustls `ServerConfig` for the `--ssl-port` HTTPS listener from PEM
/// cert-chain + private-key files. Uses the process-wide aws-lc-rs provider —
/// call [`install_default_provider`] first. Offers ALPN h2 + http/1.1 so browsers
/// negotiate HTTP/2 over TLS.
pub fn load_server_config(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> anyhow::Result<std::sync::Arc<rustls::ServerConfig>> {
    use anyhow::Context as _;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let cert_pem = std::fs::read(cert_path)
        .with_context(|| format!("reading TLS cert {}", cert_path.display()))?;
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_slice())
        .collect::<Result<_, _>>()
        .with_context(|| format!("parsing TLS cert {}", cert_path.display()))?;
    if certs.is_empty() {
        anyhow::bail!(
            "no PEM certificates found in {} — expected a `-----BEGIN CERTIFICATE-----` block",
            cert_path.display()
        );
    }

    let key_pem = std::fs::read(key_path)
        .with_context(|| format!("reading TLS key {}", key_path.display()))?;
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_pem.as_slice())
        .with_context(|| format!("parsing TLS key {}", key_path.display()))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no PEM private key found in {} — expected a PKCS#8/PKCS#1/SEC1 key block",
                key_path.display()
            )
        })?;

    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("invalid TLS cert/key (do they match?)")?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(std::sync::Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // Throwaway self-signed RSA keypair (CN=helios-test.local) — test-only.
    const TEST_CERT: &str = "-----BEGIN CERTIFICATE-----
MIIC0zCCAbugAwIBAgIJALvhFnZ9WbllMA0GCSqGSIb3DQEBCwUAMBwxGjAYBgNV
BAMMEWhlbGlvcy10ZXN0LmxvY2FsMB4XDTI2MDYyMjAxMjU0MVoXDTM2MDYxOTAx
MjU0MVowHDEaMBgGA1UEAwwRaGVsaW9zLXRlc3QubG9jYWwwggEiMA0GCSqGSIb3
DQEBAQUAA4IBDwAwggEKAoIBAQCxCHBKOBm1V4DxQjoQvsx/J0DjddOWE/8J2rSQ
GyEsNzykgQJDBykdD+9ceXgsw6gRyemfvQRn8uu60iSGm/80DOV6MYBPWRD9DKXu
Euw/BVTfqfQXHckwht7r4h1Yd5Z6GwMwZjhT0V3jtGxTMY6Fos+JA80kCVYHrjKK
KByE4R9NEeKpaCrxO19nPnTXiy122jFTm3MgOwka1TpMRc3sj4Q1Rpytal36BZxt
lpxFNFt517InT6Ym0WjX7xDEUCDKBayVf467gI7ImKuiPPukSSBfx8RIWf5iaIe1
2X/LPmgdOZQ7d6gFdnmPBFzwJ0YI8x5T75GrmdQIOs0L/xhLAgMBAAGjGDAWMBQG
A1UdEQQNMAuCCWxvY2FsaG9zdDANBgkqhkiG9w0BAQsFAAOCAQEAVACiptV6wXMV
zHjOoNaYUsZlRDaF2jG4ogTUgPsmLMgPgsR8b9e37PbhEzavzO+MPYmvzBjs2oY+
XLK94e1dDY5JI763Q5OXxrdvOhFInhw+9AGBdDX1O/YhcaHhOtgs5/R0nRVUvKWb
FrAMtXn0kL+8C9J3Eg+Itjx0x6ygpLVycEe0BqbRbf1/o4zJMbHFvsM2j5YF2bPT
wOdUkKziNF7BkfKgyQLdEEbwrgmgJ2tdxPddF66Bu5J48XeWXR31atuqcNyUsxaI
igXErwPbXJeDnI/ESqvqykfpHIcYnV3xr3aGaFIC0hxqs35CXrwHe1Uu+eKtRB1X
x6azVXGBWQ==
-----END CERTIFICATE-----
";
    const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCxCHBKOBm1V4Dx
QjoQvsx/J0DjddOWE/8J2rSQGyEsNzykgQJDBykdD+9ceXgsw6gRyemfvQRn8uu6
0iSGm/80DOV6MYBPWRD9DKXuEuw/BVTfqfQXHckwht7r4h1Yd5Z6GwMwZjhT0V3j
tGxTMY6Fos+JA80kCVYHrjKKKByE4R9NEeKpaCrxO19nPnTXiy122jFTm3MgOwka
1TpMRc3sj4Q1Rpytal36BZxtlpxFNFt517InT6Ym0WjX7xDEUCDKBayVf467gI7I
mKuiPPukSSBfx8RIWf5iaIe12X/LPmgdOZQ7d6gFdnmPBFzwJ0YI8x5T75GrmdQI
Os0L/xhLAgMBAAECggEAFIbk9kYg/Pzjb5X9Q5nL/mZfyMANw5YX+V2JmDf9FbJl
7rEiwQDgjIUffPp8q7wYDc/6rdHt49uv556cK5uE8NUZ+pwow4qRRLYlu0Aocno8
yB5dthx3CpBo4rL6MhrTsN7W9NK1b0qUd2WhNdhGLUqeg8WUELygZA2XwJs8C1AZ
hfK1eLY0eAvoGNqNBE7RaXGjxIhlKH+STOAv54q371ZZ4/jYI1hj8G7HPDzO/uV/
E+XVb7BlH2LYwi4ir0sWY1RA/0HeLJvey8zVoMNH9GplmhrGhrg2DWeHoJ8WZaMW
TsCkkLrBu0/Qo8xRj0eWcRZYTMujjTFGBMxNdouOQQKBgQDqmKNCc0OutVi0XTdh
7quqgS673IRYSDLi1ECLEBCQrc7NTNGruDRwzQve0iucqGdvIiw1QNs2hQ555CpD
s2F5281zsu0GRetqw6AE+v1/88lZkUY+8akNQrGsx9H78SusjHTwuwiLC4Yx8GTz
98HlaiMzu5YFmK6Izx+EkA4jKwKBgQDBL1ILMUTAv4cqOMyg4NxasWGSkP3ICnBs
MsFlkTeU1W61ajNhInhrE3uWfnpaR1DZS4uBkLkQ8MMqEyuOUs0b4ivQVPvLpu9Y
Yfbif3b6VdTFvtP4KqKhxDJBcHKcUa9roLRbOh1vtpw6bTyONbRRva2buat//ECn
W+4HzpzPYQKBgQC3k4IN+cy45kfnvBoelHnZDwXXFBSsULMhNR7cs1GDJb9yf+6D
Bb5jltD3KFfgWxe1q3QUqA/idfSCBb3dBH3+sbXwF8/K3OP/w91wiEfe3JJveHMT
xl+XdN08a5EyKeMXP0IzLujchcQZSBh3oSUltQye6ufWsUfC3vG29lNZyQKBgES4
YZYLq6ppN1rEo74i3x//83agzzYmyIEkuPk5ZC00k1JDeg12pqFoZ9FMIpgUwGTb
479uTPcCvlosQZU6TS47EVzlrkBunLuy9ZDyyM8aUzsYu+yOthWXZk0zBAIpaJ5/
p0jAbpI7wm1iSGVKI1/kempn7OL1R8aBBDaQv+VhAoGBAOcKoyKWAb5jtr+lxLor
psL1fgFD1XisVdGgT35p/qLTzkMStwK5+/BmbGmfx5UZJum+GiP8sVrxzDG2QGxy
Dj7m8sixAOJT+DlVp2C7GPZPFQVfKdN35R9WlC7Ap1XLbSvQl4PCDSOfu+caoIlS
4PXIkDCI6HNlyRQBzuRuFzWa
-----END PRIVATE KEY-----
";

    fn write_tmp(name: &str, contents: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("helios_tls_{name}_{}", std::process::id()));
        std::fs::File::create(&p)
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
        p
    }

    #[test]
    fn loads_valid_pem_cert_and_key_with_alpn() {
        install_default_provider();
        let cert = write_tmp("ok_cert", TEST_CERT);
        let key = write_tmp("ok_key", TEST_KEY);
        let cfg = load_server_config(&cert, &key).expect("should load");
        assert_eq!(
            cfg.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        let _ = std::fs::remove_file(cert);
        let _ = std::fs::remove_file(key);
    }

    #[test]
    fn rejects_cert_without_pem_block() {
        install_default_provider();
        let cert = write_tmp("bad_cert", "not a certificate");
        let key = write_tmp("bad_key", TEST_KEY);
        assert!(load_server_config(&cert, &key).is_err());
        let _ = std::fs::remove_file(cert);
        let _ = std::fs::remove_file(key);
    }

    #[test]
    fn errors_on_missing_files() {
        let missing = std::path::Path::new("/nonexistent/helios/tls.pem");
        assert!(load_server_config(missing, missing).is_err());
    }
}
