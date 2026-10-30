use super::*;

#[test]
fn requests_and_responses_cross_the_socket_in_frames() {
    let (mut a, mut b) = UnixStream::pair().expect("a socket pair");
    let request = Request::Unmount { keep: vec![42, 41] };
    framed_write(&mut a, &request).expect("writes");
    let read: Request = framed_read(&mut b).expect("reads").expect("a message");
    assert_eq!(read, request);
    drop(a);
    assert_eq!(framed_read::<Request>(&mut b).expect("reads"), None);
}

#[test]
fn oversized_messages_are_refused() {
    let (mut a, mut b) = UnixStream::pair().expect("a socket pair");
    a.write_all(&(MAX_MESSAGE + 1).to_le_bytes())
        .expect("writes");
    assert!(framed_read::<Request>(&mut b).is_err());
}

#[test]
fn the_peer_of_a_socket_pair_is_this_process() {
    let (a, _b) = UnixStream::pair().expect("a socket pair");
    assert_eq!(peer_pid(&a), Some(std::process::id()));
}

#[test]
fn a_services_response_carries_what_reconciliation_did() {
    let (mut a, mut b) = UnixStream::pair().expect("a socket pair");
    let request = Request::Services {
        previous: Some(1),
        generation: 2,
    };
    framed_write(&mut a, &request).expect("writes");
    assert_eq!(
        framed_read::<Request>(&mut b).expect("reads"),
        Some(request)
    );
    let response = Response {
        error: None,
        services: Some(Services::Reconciled {
            reboot: vec!["udevd".into()],
            applied: laft_svc::reconcile::Applied {
                not_ready: vec!["hello".into()],
                failed: Vec::new(),
            },
        }),
    };
    framed_write(&mut b, &response).expect("writes");
    assert_eq!(
        framed_read::<Response>(&mut a).expect("reads"),
        Some(response)
    );
}
