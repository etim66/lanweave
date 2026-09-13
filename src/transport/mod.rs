//! TCP/TLS transport with one ordered writer and explicit cancellation.
//!
//! Owns the framed TCP connection and bounded writer (see [`framed`]), and
//! layers the provisional TLS 1.3 profile with ALPN `lanweave/1`, fresh
//! per-connection certificates, and disabled resumption and early data (see
//! [`tls`]). The framing codec in [`crate::framing`] is reused verbatim over
//! arbitrary socket read boundaries.
//!
//! This layer is transport-only: it never decides consent or pairing, and its
//! failures are transport outcomes. The session owner (a later feature) owns
//! mapping frames to protocol state and application events.
#![cfg_attr(not(test), allow(dead_code))]

mod framed;
mod tls;

// The re-exports are the transport API surface for the session owner in a
// later PR, so they are unused for now.
#[allow(unused_imports)]
pub(crate) use framed::{
    CONTROL_QUEUE_CAPACITY, DATA_QUEUE_CAPACITY, FRAME_WRITE_DEADLINE, FramedConnection, Outbound,
    ReadError, SendError, split_frame_io,
};
#[allow(unused_imports)]
pub(crate) use tls::{ALPN_PROTOCOL, EXPORTER_LEN, TlsHandshake, accept, accept_stream, connect};

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use tokio::net::TcpListener;

    use super::{FramedConnection, split_frame_io};
    use crate::framing::Frame;
    use crate::protocol::{Control, Hello, Inbound, Phase, ProtocolState, Role, accept, send};

    /// End-to-end loopback check: real TCP, split frames both ways, and the
    /// exact protocol-level hello exchange ordering.
    #[tokio::test]
    async fn loopback_connection_performs_the_hello_exchange() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();

        let server_side = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            split_frame_io(stream)
        });
        let client = tokio::net::TcpStream::connect(address).await.unwrap();
        let (mut initiator_conn, initiator_out) = split_frame_io(client);
        let (mut responder_conn, responder_out) = server_side.await.unwrap();

        let mut initiator_state = ProtocolState::new(Role::Initiator);
        let mut responder_state = ProtocolState::new(Role::Responder);

        // The initiator sends its hello first...
        let hello = Control::Hello(Hello::new(Some("alpha".to_owned())));
        send(&mut initiator_state, &hello).unwrap();
        initiator_out.send_control(&hello).await.unwrap();

        // ...the responder accepts it and answers in the reverse direction.
        let inbound = receive_control(&mut responder_conn).await;
        assert_eq!(accept(&mut responder_state, inbound).unwrap(), Vec::new());
        let reply = Control::Hello(Hello::new(Some("beta".to_owned())));
        send(&mut responder_state, &reply).unwrap();
        responder_out.send_control(&reply).await.unwrap();
        assert!(matches!(responder_state.phase(), Phase::PairRequest));

        // The initiator consumes the reply; both ends now await pair_request.
        let inbound = receive_control(&mut initiator_conn).await;
        assert_eq!(accept(&mut initiator_state, inbound).unwrap(), Vec::new());
        assert!(matches!(initiator_state.phase(), Phase::PairRequest));
    }

    /// Closing one side is observed exactly once as a terminal outcome.
    #[tokio::test]
    async fn loopback_close_yields_one_terminal_outcome() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();

        let server_side = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            split_frame_io(stream)
        });
        let client = tokio::net::TcpStream::connect(address).await.unwrap();
        let (mut initiator_conn, mut initiator_out) = split_frame_io(client);
        let (mut responder_conn, mut responder_out) = server_side.await.unwrap();

        initiator_out.send_control(&Control::Ready).await.unwrap();
        initiator_out.close().await;

        // The peer drains the frame first, then reports the close once.
        let frame = responder_conn
            .read_frame()
            .await
            .unwrap()
            .expect("ready frame must arrive before EOF");
        assert_eq!(
            frame.into_body(),
            bytes::Bytes::from_static(br#"{"type":"ready"}"#)
        );
        assert_eq!(responder_conn.read_frame().await.unwrap(), None);
        assert_eq!(responder_conn.read_frame().await.unwrap(), None);

        // The initiator side observes the same single terminal outcome after
        // the responder closes its own direction as well.
        responder_out.close().await;
        assert_eq!(initiator_conn.read_frame().await.unwrap(), None);
        assert_eq!(initiator_conn.read_frame().await.unwrap(), None);
    }

    /// Sends a control frame and reads it as an exact JSON body, decoded
    /// strictly, the same way the session owner will.
    async fn receive_control(connection: &mut FramedConnection<tokio::net::TcpStream>) -> Inbound {
        let frame = connection
            .read_frame()
            .await
            .expect("transport read")
            .expect("connection must not close mid-hello");
        match frame {
            Frame::Control(body) => Inbound::Control(Control::decode(&body).unwrap()),
            Frame::Data(body) => panic!("unexpected DATA body: {body:?}"),
        }
    }
}
