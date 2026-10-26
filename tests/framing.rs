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
fn dashboard_shortcut_preserves_arrows_tmux_prefix_and_other_native_keys() {
    let mut leader = Leader::default();
    let keys = b"\x03\x04\x18\x1b[D\x1bOD\x02d\x02\t\xff";
    assert_eq!(leader.filter(keys), (keys.to_vec(), false));
    assert_eq!(leader.filter(b"\x1b["), (b"\x1b[".to_vec(), false));
    assert_eq!(leader.filter(b"D"), (b"D".to_vec(), false));
    assert_eq!(
        leader.filter(b"hello\x07ignored"),
        (b"hello".to_vec(), true)
    );
    assert_eq!(leader.filter(b"\x07"), (vec![], true));
}

#[test]
fn enriched_created_reply_preserves_the_existing_wire_shape() {
    let original = serde_json::json!({"Created": {
        "id":"wrapper", "cwd":"/tmp/project", "agent":null,
        "generation":"v2", "pid":42, "state":"live", "exit_code":null,"notice":null
    }});
    let reply: kiro_agents::protocol::Reply = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(&reply).unwrap(), original);
    if let kiro_agents::protocol::Reply::Created(mut row) = reply {
        row.name = Some("Fix picker".into());
        let encoded = serde_json::to_value(kiro_agents::protocol::Reply::Created(row)).unwrap();
        assert_eq!(encoded["Created"]["name"], "Fix picker");
        assert_eq!(encoded["Created"]["id"], "wrapper");
    } else {
        panic!("Expected created reply")
    }
}
