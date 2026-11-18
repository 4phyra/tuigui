use super::*;

/// A refusal names the command, its arguments, and how much data it carried.
///
/// The terminal's own answer says none of this: `ENOENT: image not found` is the
/// same sentence for a transmit and for a placement, so on its own it cannot say
/// which of the two was refused.
#[test]
fn a_rejected_command_reports_what_was_asked_for() {
    let command = Message::new()
        .cd('a', "p")
        .cd('i', 7)
        .cd('p', 3)
        .payload(vec![b'A'; 128]);
    let reply = Reply {
        message: Some(Message::new().payload("ENOENT: image not found")),
        diagnostic: Diagnostic::of(&command),
    };

    let error = Result::<Reply, TGPError>::from(reply).expect_err("a rejection is an error");

    assert_eq!(
        error.to_string(),
        "Terminal emulator returned an error: ENOENT: image not found \
         (for a=p,i=7,p=3 with 128 bytes of payload)"
    );
}

/// The payload's bytes never reach the message: a frame's payload is megabytes of
/// base64, and none of it says which command failed.
#[test]
fn the_payload_bytes_are_not_reported() {
    let command = Message::new().cd('a', "T").payload(b"pixels".to_vec());

    assert_eq!(
        Diagnostic::of(&command).to_string(),
        "a=T with 6 bytes of payload"
    );
}

/// An answer with no payload carries nothing to refuse, so it stays a success:
/// only a terminal's error sentence becomes one.
#[test]
fn an_answer_without_a_payload_is_not_a_refusal() {
    let reply = Reply {
        message: Some(Message {
            cds: vec![],
            payload: None,
        }),
        diagnostic: Diagnostic::of(&Message::new().cd('a', "q")),
    };

    assert!(Result::<Reply, TGPError>::from(reply).is_ok());
}
