//! Provisional TLS 1.3 profile over the framed connection.
//!
//! Version 1 runs TLS 1.3 immediately after TCP with the exact ALPN protocol
//! `lanweave/1`. The responder generates a fresh self-signed ECDSA P-256
//! certificate for every connection; the initiator's custom verifier only
//! sits inside the required narrow checks and is NOT public trust validation:
//! it rejects anything that is not a single end-entity certificate, verifies
//! the TLS 1.3 CertificateVerify, and relaxes chain and server-name checking.
//! This is a provisional encrypted connection and is not peer
//! authentication; pairing (a later feature) confirms the live connection.
//!
//! Resumption, tickets, PSKs, early data and 0.5-RTT data are disabled on
//! both ends, and no key logging is configured. Each handshake yields a fresh
//! 32-byte RFC 5705 exporter that pairing later binds to.
//!
//! The `tls12` compile feature is not enabled, so TLS 1.2 is unsupported at
//! compile time in addition to the per-config version restriction.
#![cfg_attr(not(test), allow(dead_code))]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error as TlsError, PeerMisbehaved,
    SignatureScheme,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsStream;
use webpki::EndEntityCert;
use zeroize::Zeroizing;

/// ALPN protocol identifier negotiated on every version 1 connection.
pub(crate) const ALPN_PROTOCOL: &[u8] = b"lanweave/1";
/// Length of the per-connection exporter in bytes.
pub(crate) const EXPORTER_LEN: usize = 32;

/// Label for the provisional exporter; pairing binds its own labels later.
const EXPORTER_LABEL: &[u8] = b"lanweave/v1";
/// The only signature scheme the initiator accepts from the responder.
const RESPONDER_SIGNATURE_SCHEME: SignatureScheme = SignatureScheme::ECDSA_NISTP256_SHA256;
/// Fixed responder subject used for the fresh self-signed certificate.
const PEER_SERVER_NAME: &str = "lanweave.local";

/// A completed TLS connection together with its fresh exporter.
pub(crate) type TlsHandshake = (TlsStream<TcpStream>, Zeroizing<[u8; EXPORTER_LEN]>);

/// Connects to the responder and completes the provisional TLS 1.3 profile.
pub(crate) async fn connect(
    endpoint: SocketAddr,
    deadline: Duration,
) -> anyhow::Result<TlsHandshake> {
    let handshake = async {
        let tcp = TcpStream::connect(endpoint).await?;
        let server_name = ServerName::try_from(PEER_SERVER_NAME.to_owned())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        let connector = tokio_rustls::TlsConnector::from(initiator_config());
        connector
            .connect(server_name, tcp)
            .await
            .map(TlsStream::from)
    };
    let stream = tokio::time::timeout(deadline, handshake)
        .await
        .map_err(|_| anyhow::anyhow!("TLS connection timed out"))??;
    ensure_alpn(&stream)?;
    let exporter = connection_exporter(&stream)?;
    Ok((stream, exporter))
}

/// Accepts one responder connection and completes the provisional TLS 1.3
/// profile with a fresh per-connection certificate.
pub(crate) async fn accept(
    listener: &TcpListener,
    deadline: Duration,
) -> anyhow::Result<TlsHandshake> {
    let (tcp, _) = tokio::time::timeout(deadline, listener.accept())
        .await
        .map_err(|_| anyhow::anyhow!("TLS connection timed out"))??;
    accept_stream(tcp, deadline).await
}

/// Completes the provisional TLS 1.3 profile on an already-accepted socket.
///
/// The session owner accepts sockets through the discovery listener and then
/// hands each one here, so the TLS handshake shares the responder profile
/// without re-accepting from the listener.
pub(crate) async fn accept_stream(
    stream: TcpStream,
    deadline: Duration,
) -> anyhow::Result<TlsHandshake> {
    // A fresh certificate and server config is created per connection.
    let responder_config = responder_config()?;
    let handshake = async {
        let acceptor = tokio_rustls::TlsAcceptor::from(responder_config.clone());
        acceptor.accept(stream).await.map(TlsStream::from)
    };
    let stream = tokio::time::timeout(deadline, handshake)
        .await
        .map_err(|_| anyhow::anyhow!("TLS handshake timed out"))??;
    ensure_alpn(&stream)?;
    let exporter = connection_exporter(&stream)?;
    Ok((stream, exporter))
}

/// Rejects a completed handshake that did not negotiate the exact ALPN
/// protocol. Rustls alone does not fail when the peer sends or selects no
/// ALPN extension, so the profile is enforced here.
fn ensure_alpn(stream: &TlsStream<TcpStream>) -> anyhow::Result<()> {
    let negotiated = match stream {
        TlsStream::Client(client) => client.get_ref().1.alpn_protocol(),
        TlsStream::Server(server) => server.get_ref().1.alpn_protocol(),
    };
    if negotiated != Some(ALPN_PROTOCOL) {
        return Err(anyhow::anyhow!("peer did not negotiate ALPN lanweave/1"));
    }
    Ok(())
}

/// Builds the per-connection responder configuration.
fn responder_config() -> anyhow::Result<Arc<rustls::ServerConfig>> {
    build_responder_config(&[ALPN_PROTOCOL])
}

/// Builds the per-connection responder configuration with the given ALPN list.
fn build_responder_config(alpn: &[&[u8]]) -> anyhow::Result<Arc<rustls::ServerConfig>> {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec![PEER_SERVER_NAME.to_owned()]).map_err(|error| {
            anyhow::anyhow!("failed to generate a responder certificate: {error}")
        })?;
    let key = PrivateKeyDer::Pkcs8(signing_key.serialize_der().into());
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .expect("ring supports TLS 1.3")
    .with_no_client_auth()
    .with_single_cert(vec![cert.der().clone()], key)
    .map_err(|error| anyhow::anyhow!("failed to configure the responder TLS profile: {error}"))?;

    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    // No resumption: nothing is stored and no tickets are issued, so PSKs and
    // 0-RTT have nothing to derive from. Early data is off (default of zero).
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    Ok(Arc::new(config))
}

/// Builds the initiator configuration with the narrow provisional verifier.
fn initiator_config() -> Arc<ClientConfig> {
    Arc::new(initiator_config_inner())
}

/// Builds the unshared initiator configuration, for customizing and unit tests.
fn initiator_config_inner() -> ClientConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("ring supports TLS 1.3")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ProvisionalServerVerifier {}))
        .with_no_client_auth();

    config.alpn_protocols = vec![ALPN_PROTOCOL.to_vec()];
    // No resumption or tickets on the client either, and never any early data.
    config.resumption = rustls::client::Resumption::disabled();
    config
}

/// Derives the fresh 32-byte exporter after the handshake completed.
fn connection_exporter(
    stream: &TlsStream<TcpStream>,
) -> anyhow::Result<Zeroizing<[u8; EXPORTER_LEN]>> {
    let mut exporter = Zeroizing::new([0; EXPORTER_LEN]);
    let exported = match stream {
        TlsStream::Client(client) => {
            client
                .get_ref()
                .1
                .export_keying_material(&mut exporter, EXPORTER_LABEL, None)
        }
        TlsStream::Server(server) => {
            server
                .get_ref()
                .1
                .export_keying_material(&mut exporter, EXPORTER_LABEL, None)
        }
    };
    exported.map_err(|error| anyhow::anyhow!("TLS exporter unavailable: {error}"))?;
    Ok(exporter)
}

/// The narrow initiator verifier for the provisional profile.
///
/// It relaxes only public trust-chain and server-name checks. It still
/// requires a single well-formed end-entity certificate and verifies the
/// CertificateVerify signature with a fixed P-256 scheme, which is also the
/// proof that the responder holds the certificate's private key.
#[derive(Debug)]
struct ProvisionalServerVerifier {}

impl ServerCertVerifier for ProvisionalServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        // The provisional profile expects exactly one fresh end-entity
        // certificate: a peer that sends intermediates presents an unexpected
        // chain shape.
        if !intermediates.is_empty() {
            return Err(TlsError::InvalidCertificate(
                CertificateError::UnknownIssuer,
            ));
        }
        // Required shape: a parseable X.509 end-entity certificate.
        EndEntityCert::try_from(end_entity)
            .map_err(|_| TlsError::InvalidCertificate(CertificateError::BadEncoding))?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        // TLS 1.2 is disabled at compile time and in the configured versions,
        // so this method is never called. Fail closed if it somehow is.
        Err(TlsError::General("TLS 1.2 is not supported".to_owned()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        if dss.scheme != RESPONDER_SIGNATURE_SCHEME {
            return Err(TlsError::PeerMisbehaved(
                PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme,
            ));
        }
        // Verify the CertificateVerify: proof that the responder holds the
        // private key for the presented certificate. The fixed P-256
        // algorithm also binds the public key to the required curve.
        let cert = EndEntityCert::try_from(cert)
            .map_err(|_| TlsError::InvalidCertificate(CertificateError::BadEncoding))?;
        cert.verify_signature(webpki::ring::ECDSA_P256_SHA256, message, dss.signature())
            .map_err(|_| TlsError::InvalidCertificate(CertificateError::BadSignature))?;
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![RESPONDER_SIGNATURE_SCHEME]
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::Arc;

    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::ServerName;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::TlsStream;

    use super::{
        ALPN_PROTOCOL, EXPORTER_LEN, ProvisionalServerVerifier, accept, build_responder_config,
        connect, responder_config,
    };
    use crate::protocol::Control;
    use crate::transport::framed::split_frame_io;

    /// Spawns a server-side `accept` and completes one client-side `connect`.
    async fn handshake_pair() -> (
        super::TlsHandshake,
        tokio::task::JoinHandle<super::TlsHandshake>,
    ) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            accept(&listener, std::time::Duration::from_secs(5))
                .await
                .unwrap()
        });
        let client = connect(address, std::time::Duration::from_secs(5))
            .await
            .unwrap();
        (client, server)
    }

    fn session(stream: &TlsStream<TcpStream>) -> &rustls::CommonState {
        stream.get_ref().1
    }

    #[tokio::test]
    async fn handshake_uses_tls13_and_the_exact_alpn() {
        let ((client_stream, _), server) = handshake_pair().await;
        let (server_stream, _) = server.await.unwrap();

        let client = session(&client_stream);
        let server = session(&server_stream);
        assert_eq!(
            client.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        assert_eq!(
            server.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        assert_eq!(client.alpn_protocol(), Some(ALPN_PROTOCOL));
        assert_eq!(server.alpn_protocol(), Some(ALPN_PROTOCOL));
        assert!(!client.is_handshaking());
        assert!(!server.is_handshaking());
    }

    #[tokio::test]
    async fn exporter_matches_at_both_ends() {
        let ((_client_stream, client_exporter), server) = handshake_pair().await;
        let (_server_stream, server_exporter) = server.await.unwrap();

        assert_eq!(client_exporter.len(), EXPORTER_LEN);
        assert_eq!(*client_exporter, *server_exporter);
    }

    #[tokio::test]
    async fn certificates_and_exporters_differ_across_connections() {
        let ((client_stream, first_exporter), server) = handshake_pair().await;
        let (_server_stream, _) = server.await.unwrap();
        let first_cert = session(&client_stream)
            .peer_certificates()
            .and_then(|certs| certs.first())
            .map(|der| der.as_ref().to_vec())
            .expect("initiator must have seen one responder certificate");

        let ((client_stream, second_exporter), server) = handshake_pair().await;
        let (_server_stream, _) = server.await.unwrap();
        let second_cert = session(&client_stream)
            .peer_certificates()
            .and_then(|certs| certs.first())
            .map(|der| der.as_ref().to_vec())
            .expect("initiator must have seen one responder certificate");

        assert_ne!(first_cert, second_cert, "responder certificate was reused");
        assert_ne!(*first_exporter, *second_exporter, "exporter was reused");
    }

    #[tokio::test]
    async fn mismatched_alpn_fails_the_handshake() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server =
            tokio::spawn(async move { accept(&listener, std::time::Duration::from_secs(5)).await });

        let mut config = super::initiator_config_inner();
        config.alpn_protocols = vec![b"other/1".to_vec()];
        let client_result = tokio::spawn(async move {
            let tcp = TcpStream::connect(address).await.unwrap();
            let server_name = ServerName::try_from(super::PEER_SERVER_NAME.to_owned()).unwrap();
            let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
            connector.connect(server_name, tcp).await
        })
        .await
        .unwrap();
        assert!(
            client_result.is_err(),
            "a mismatched ALPN must not complete the handshake"
        );
        let server_result = server.await.unwrap();
        assert!(
            server_result.as_ref().is_err(),
            "the responder must reject an unknown ALPN"
        );
    }

    #[tokio::test]
    async fn responder_rejects_a_client_that_skips_alpn() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server =
            tokio::spawn(async move { accept(&listener, std::time::Duration::from_secs(5)).await });

        let mut config = super::initiator_config_inner();
        config.alpn_protocols = vec![];
        let client_result = tokio::spawn(async move {
            let tcp = TcpStream::connect(address).await.unwrap();
            let server_name = ServerName::try_from(super::PEER_SERVER_NAME.to_owned()).unwrap();
            let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
            connector.connect(server_name, tcp).await
        })
        .await
        .unwrap();
        assert!(
            client_result.is_ok(),
            "a raw handshake without ALPN completes; only the profile check rejects it"
        );
        assert!(
            server.await.unwrap().is_err(),
            "the responder must reject a client that offers no ALPN"
        );
    }

    #[tokio::test]
    async fn initiator_rejects_a_server_that_skips_alpn() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let config = build_responder_config(&[]).unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio_rustls::TlsAcceptor::from(config)
                .accept(tcp)
                .await
                .unwrap()
        });

        assert!(
            connect(address, std::time::Duration::from_secs(5))
                .await
                .is_err(),
            "the initiator must reject a server that selects no ALPN"
        );
        // The TLS handshake itself still completes; only the profile check
        // fails, which is what the assertion above proves.
        server.await.unwrap();
    }

    #[tokio::test]
    async fn framed_controls_cross_the_tls_connection() {
        let ((client_stream, _), server) = handshake_pair().await;
        let (server_stream, _) = server.await.unwrap();
        let (mut client_conn, client_outbound) = split_frame_io(client_stream);
        let (mut server_conn, server_outbound) = split_frame_io(server_stream);

        server_outbound.send_control(&Control::Ready).await.unwrap();
        let frame = client_conn.read_frame().await.unwrap().unwrap();
        assert_eq!(
            frame.into_body(),
            bytes::Bytes::from_static(br#"{"type":"ready"}"#)
        );

        client_outbound
            .send_control(&Control::PairRequest)
            .await
            .unwrap();
        let frame = server_conn.read_frame().await.unwrap().unwrap();
        assert_eq!(
            frame.into_body(),
            bytes::Bytes::from_static(br#"{"type":"pair_request"}"#)
        );
    }

    #[test]
    fn verifier_rejects_unexpected_certificate_shapes() {
        let verifier = ProvisionalServerVerifier {};
        let now = rustls::pki_types::UnixTime::since_unix_epoch(std::time::Duration::new(
            1_700_000_000,
            0,
        ));
        let server_name = ServerName::try_from(super::PEER_SERVER_NAME.to_owned()).unwrap();

        let result = verifier.verify_server_cert(
            &rustls::pki_types::CertificateDer::from(b"not a certificate".to_vec()),
            &[],
            &server_name,
            &[],
            now,
        );
        assert!(
            result.is_err(),
            "garbage certificate bytes must be rejected"
        );

        let certified = generated_cert();
        let result = verifier.verify_server_cert(
            &certified.0,
            std::slice::from_ref(&certified.0),
            &server_name,
            &[],
            now,
        );
        assert!(
            result.is_err(),
            "an unexpected intermediate must be rejected"
        );

        let result = verifier.verify_server_cert(&certified.0, &[], &server_name, &[], now);
        assert!(
            result.is_ok(),
            "a single fresh end-entity certificate is the accepted shape"
        );

        assert_eq!(
            verifier.supported_verify_schemes(),
            vec![rustls::SignatureScheme::ECDSA_NISTP256_SHA256]
        );
    }

    fn generated_cert() -> (
        rustls::pki_types::CertificateDer<'static>,
        rustls::pki_types::PrivateKeyDer<'static>,
    ) {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec![super::PEER_SERVER_NAME.to_owned()]).unwrap();
        (
            cert.der().clone(),
            rustls::pki_types::PrivateKeyDer::Pkcs8(signing_key.serialize_der().into()),
        )
    }

    #[test]
    fn responder_config_builds_one_fresh_profile() {
        let first = responder_config().unwrap();
        let second = responder_config().unwrap();
        assert_eq!(first.alpn_protocols, vec![ALPN_PROTOCOL.to_vec()]);
        assert_eq!(second.alpn_protocols, vec![ALPN_PROTOCOL.to_vec()]);
        assert!(!Arc::ptr_eq(&first, &second), "config must not be shared");
    }
}
