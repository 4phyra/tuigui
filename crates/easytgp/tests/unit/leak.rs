use super::*;

/// How many of this process's descriptors are open.
///
/// `/proc/self/fd` is the only place Linux reports it: the directory's own handle is
/// counted too, which is why every measurement here is compared against another one
/// rather than against an absolute number.
fn open_descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .expect("/proc is mounted")
        .count()
}

/// How many shared memory objects this process has left behind.
///
/// Named for the process, so objects from other runs are not counted: a terminal that
/// died without reading one still has its file on disk under its own pid.
fn objects_of_this_process() -> usize {
    let mine = format!("easytgp{:x}-", std::process::id());
    std::fs::read_dir(SHM_DIR)
        .expect("/dev/shm is a directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&mine))
        .count()
}

/// A frame's object is made and dropped without leaving a descriptor or a file behind.
///
/// A shared memory object is an open file and a name, so a path that makes one per frame
/// and keeps either is a slow exhaustion: descriptors run out first, and the failure it
/// gives is `Too many open files` on a frame that has done nothing new. What the pixels
/// cost is not the point of this test — that it costs nothing that accumulates is.
#[test]
fn staging_a_frame_does_not_accumulate_descriptors_or_objects() {
    // One cycle first, so the count that follows is of a path already taken: a first call
    // that allocates something permanent would otherwise be read as growth.
    drop(ShmFrame::create(b"warm up").expect("a warm-up object"));
    let descriptors = open_descriptors();
    let objects = objects_of_this_process();

    for frame in 0..64 {
        let pixels = vec![frame as u8; 64 * 64 * 3];
        drop(ShmFrame::create(&pixels).expect("an object for the frame"));
    }

    assert_eq!(
        open_descriptors(),
        descriptors,
        "a dropped object leaves no descriptor open"
    );
    assert_eq!(
        objects_of_this_process(),
        objects,
        "a dropped object leaves no file behind"
    );
}

/// Re-sending an object recreates it rather than leaving the old one behind.
///
/// The terminal unlinks what it reads, so a re-send needs a fresh name. Making one per
/// re-send is the cost; keeping the previous one as well is the leak, and it is the shape
/// a re-sent frame takes when the old handle is not dropped before the new one replaces it.
#[test]
fn re_sending_an_object_replaces_it_instead_of_adding_one() {
    let frame = ShmFrame::create(b"pixels").expect("an object");
    let before = objects_of_this_process();

    let mut held = frame;
    for _ in 0..16 {
        held = held.recreate().expect("a fresh object under a new name");
    }

    assert_eq!(
        objects_of_this_process(),
        before,
        "each re-send replaces the object it came from"
    );
}

/// A request that is sent again leaves nothing behind but the object it will name.
///
/// This is the frame path a returning window takes: the request is kept after its first
/// transmit, and every later frame renews its object. Two objects alive per request means
/// one is never dropped, and a session that streams exhausts its descriptors.
#[test]
fn a_request_sent_again_holds_one_object_at_a_time() {
    let request = ImageRequest {
        format: Shm,
        size: Some(ImageSize {
            width: 8,
            height: 8,
        }),
        data: vec![],
        shm: None,
    };
    // The object the first frame would have transmitted, made the way a frame makes one.
    let request = ImageRequest {
        shm: Some(Arc::new(
            ShmFrame::create(&[1u8; 8 * 8 * 3]).expect("an object"),
        )),
        ..request
    };
    let before = objects_of_this_process();

    let mut held = request;
    for _ in 0..16 {
        held = held
            .temp_remove_this_api_maybe_resend_shm()
            .expect("a fresh object under a new name");
    }

    assert_eq!(
        objects_of_this_process(),
        before,
        "re-sending a request replaces the object it names"
    );
}
