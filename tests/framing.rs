use kiro_agents::{
    attach::Leader,
    protocol::{self, Action, FrameError, Request},
};
use std::io::{BufReader, Cursor};

#[test]
fn fragmented_unicode_and_multiple_frames_round_trip() {
    let mut data = Vec::new();
    protocol::send(
        &mut data,
        &protocol::request(Action::New {
            cwd: "/tmp/日本".into(),
            agent: None,
        }),
    )
    .unwrap();
    protocol::send(&mut data, &protocol::request(Action::Detach)).unwrap();
    let mut reader = BufReader::with_capacity(1, Cursor::new(data));
    let first: Request = protocol::receive(&mut reader).unwrap();
    assert!(matches!(first.action, Action::New { cwd, .. } if cwd == "/tmp/日本"));
    let second: Request = protocol::receive(&mut reader).unwrap();
    assert!(matches!(second.action, Action::Detach));
}

#[test]
fn oversized_and_truncated_frames_are_rejected() {
    let mut oversized = Cursor::new(vec![b'x'; 128 * 1024 + 1]);
    assert!(matches!(
        protocol::receive::<Request>(&mut oversized),
        Err(FrameError::TooLarge)
    ));
    let mut partial = Cursor::new(b"{\"version\":1".to_vec());
    assert!(matches!(
        protocol::receive::<Request>(&mut partial),
        Err(FrameError::Closed)
    ));
}

#[test]
fn leader_preserves_native_keys_and_handles_chunk_boundaries() {
    let mut leader = Leader::default();
    let keys = b"\x03\x04\x07\x18\x1b\t\xff";
    assert_eq!(leader.filter(keys), (keys.to_vec(), false));
    assert_eq!(leader.filter(b"\x02"), (vec![], false));
    assert_eq!(leader.filter(b"\x02"), (vec![2], false));
    assert_eq!(leader.filter(b"\x02x"), (vec![2, b'x'], false));
    assert_eq!(leader.filter(b"hello\x02"), (b"hello".to_vec(), false));
    assert_eq!(leader.filter(b"dignored"), (vec![], true));
}
