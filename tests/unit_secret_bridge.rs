use std::sync::mpsc;
use std::time::Duration;
use termway::session::secret_bridge::{
    is_allowed_secret_call, Arg, Connection, Message, ServiceBridge, METHOD_CALL, METHOD_RETURN,
    SIGNAL,
};
use termway::session::DbusSession;

#[test]
fn test_message_roundtrip_both_endians() {
    for big_endian in [false, true] {
        let mut msg = Message {
            big_endian,
            ..Message::method_call("org.example.Dest", "/org/example/Obj", "org.example.Iface", "Method")
        }
        .with_args(&[Arg::Str("héllo"), Arg::U32(0xdead_beef)]);
        msg.serial = 42;
        msg.sender = Some(":1.7".into());
        msg.reply_serial = Some(9);

        let decoded = Message::decode(&msg.encode()).unwrap();
        assert_eq!(decoded, msg);
        assert_eq!(decoded.signature.as_deref(), Some("su"));
    }
}

#[test]
fn test_message_read_from_stream() {
    let mut msg = Message::method_call("a.b", "/", "a.b", "M").with_args(&[Arg::Str("x")]);
    msg.serial = 1;
    let mut bytes = msg.encode();
    bytes.extend(msg.encode()); // two back-to-back messages
    let mut cursor = std::io::Cursor::new(bytes);
    assert_eq!(Message::read_from(&mut cursor).unwrap(), msg);
    assert_eq!(Message::read_from(&mut cursor).unwrap(), msg);
    assert!(Message::read_from(&mut cursor).is_err());
}

#[test]
fn test_bridge_relays_calls_replies_and_signals() {
    const NAME: &str = "org.termway.BridgeTest";

    // Two independent buses standing in for the host and termway's isolated bus
    let (Ok(host_bus), Ok(isolated_bus)) = (DbusSession::start(), DbusSession::start()) else {
        eprintln!("dbus-daemon unavailable; skipping");
        return;
    };

    // Fake service on the host bus: echoes the call body and emits a signal
    let mut service = Connection::connect(host_bus.address()).unwrap();
    service.request_name(NAME).unwrap();
    std::thread::spawn(move || {
        while let Ok(call) = service.read() {
            if call.msg_type != METHOD_CALL || call.member.as_deref() != Some("Echo") {
                continue;
            }
            let reply = Message {
                msg_type: METHOD_RETURN,
                reply_serial: Some(call.serial),
                destination: call.sender.clone(),
                big_endian: call.big_endian,
                signature: call.signature.clone(),
                body: call.body.clone(),
                ..Default::default()
            };
            let signal = Message {
                msg_type: SIGNAL,
                path: Some("/org/termway/BridgeTest".into()),
                interface: Some(NAME.into()),
                member: Some("Pinged".into()),
                ..Default::default()
            }
            .with_args(&[Arg::Str("pong")]);
            let _ = service.send(reply);
            let _ = service.send(signal);
        }
    });

    let _bridge = ServiceBridge::start(NAME, isolated_bus.address(), host_bus.address()).unwrap();

    // Client on the isolated bus talks to the service as if it were local
    let mut client = Connection::connect(isolated_bus.address()).unwrap();
    client.add_match(&format!("type='signal',sender='{}'", NAME)).unwrap();
    let call = Message::method_call(NAME, "/org/termway/BridgeTest", NAME, "Echo").with_args(&[Arg::Str("hello")]);
    let serial = client.send(call).unwrap();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut reply, mut signal) = (None, None);
        while reply.is_none() || signal.is_none() {
            let Ok(msg) = client.read() else { break };
            if msg.msg_type == METHOD_RETURN && msg.reply_serial == Some(serial) {
                reply = Some(msg.body_string().unwrap());
            } else if msg.msg_type == SIGNAL && msg.member.as_deref() == Some("Pinged") {
                signal = Some(msg.body_string().unwrap());
            }
        }
        let _ = tx.send((reply, signal));
    });
    let (reply, signal) = rx.recv_timeout(Duration::from_secs(5)).expect("bridge timed out");
    assert_eq!(reply.as_deref(), Some("hello"));
    assert_eq!(signal.as_deref(), Some("pong"));
}

#[test]
fn test_bridge_requires_service_on_host() {
    let (Ok(host_bus), Ok(isolated_bus)) = (DbusSession::start(), DbusSession::start()) else {
        return;
    };
    assert!(ServiceBridge::start("org.termway.Missing", isolated_bus.address(), host_bus.address()).is_err());
}

#[test]
fn test_secret_bridge_allow_list_policy() {
    // Allowed Secret Service paths & methods
    assert!(is_allowed_secret_call(
        "/org/freedesktop/secrets",
        "org.freedesktop.Secret.Service",
        "OpenSession"
    ));
    assert!(is_allowed_secret_call(
        "/org/freedesktop/secrets/aliases/default",
        "org.freedesktop.Secret.Collection",
        "SearchItems"
    ));
    assert!(is_allowed_secret_call(
        "/org/freedesktop/secrets/collection/login/42",
        "org.freedesktop.Secret.Item",
        "GetSecret"
    ));
    assert!(is_allowed_secret_call(
        "/org/freedesktop/secrets/session/s1",
        "org.freedesktop.Secret.Session",
        "Close"
    ));
    assert!(is_allowed_secret_call(
        "/org/freedesktop/secrets/prompt/p1",
        "org.freedesktop.Secret.Prompt",
        "Prompt"
    ));
    assert!(is_allowed_secret_call(
        "/org/freedesktop/secrets",
        "org.freedesktop.DBus.Properties",
        "GetAll"
    ));

    // Blocked destructive collection deletion
    assert!(!is_allowed_secret_call(
        "/org/freedesktop/secrets/collection/login",
        "org.freedesktop.Secret.Collection",
        "Delete"
    ));

    // Blocked non-secret paths & arbitrary interfaces
    assert!(!is_allowed_secret_call(
        "/org/freedesktop/DBus",
        "org.freedesktop.Secret.Service",
        "OpenSession"
    ));
    assert!(!is_allowed_secret_call(
        "/org/freedesktop/secrets/evil",
        "org.freedesktop.DBus",
        "Hello"
    ));
    assert!(!is_allowed_secret_call(
        "/org/freedesktop/secrets",
        "org.gnome.Keyring.Internal",
        "DumpAll"
    ));
    assert!(!is_allowed_secret_call(
        "/org/freedesktop/secrets",
        "org.freedesktop.Secret.Service",
        ""
    ));
}

#[test]
fn test_message_decode_and_string_array_bounds_safety() {
    // Truncated header (< 16 bytes) must error cleanly without panic
    assert!(Message::decode(&[b'l', 1, 0, 1]).is_err());

    // Overflowing fields_len in header (0xFFFF_FFFF) must return Err, never panic or wrap
    let mut overflow_fields = vec![b'l', 1, 0, 1];
    overflow_fields.extend_from_slice(&0u32.to_le_bytes()); // body_len = 0
    overflow_fields.extend_from_slice(&1u32.to_le_bytes()); // serial = 1
    overflow_fields.extend_from_slice(&u32::MAX.to_le_bytes()); // fields_len = MAX
    assert!(Message::decode(&overflow_fields).is_err());

    // Overflowing body_len in header must return Err cleanly
    let mut overflow_body = vec![b'l', 1, 0, 1];
    overflow_body.extend_from_slice(&u32::MAX.to_le_bytes()); // body_len = MAX
    overflow_body.extend_from_slice(&1u32.to_le_bytes()); // serial = 1
    overflow_body.extend_from_slice(&0u32.to_le_bytes()); // fields_len = 0
    assert!(Message::decode(&overflow_body).is_err());

    // body_string_array with length exceeding buffer must return Err without panic
    let mut bad_array_msg = Message {
        signature: Some("as".into()),
        body: u32::MAX.to_le_bytes().to_vec(),
        ..Default::default()
    };
    assert!(bad_array_msg.body_string_array().is_err());
    bad_array_msg.body = 100u32.to_le_bytes().to_vec();
    assert!(bad_array_msg.body_string_array().is_err());
}
