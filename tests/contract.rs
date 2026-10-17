use sm0sh::{
    command::{decode, encode, END},
    config::Config,
    memory::{Completion, Memory, Status},
    protocol::{self, Device, EndCode, Operation},
};

fn write(device: Device, start: usize, words: Vec<u16>) -> Operation {
    Operation::WriteWords {
        device,
        start,
        words,
    }
}
fn read(memory: &mut Memory, device: Device, start: usize, count: usize) -> Vec<u16> {
    memory
        .apply(Operation::Read {
            device,
            start,
            count,
            bits: false,
        })
        .unwrap()
        .0
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect()
}
fn trigger() -> Operation {
    Operation::WriteBits {
        device: Device::Sm,
        start: 0,
        bits: vec![true],
    }
}

#[test]
fn documented_echo_layout_is_exact() {
    let (d, w) = encode(&["echo", "hello"], 8192, 128).unwrap();
    assert_eq!(d, [0x6365, 0x6f68, 0x6568, 0x6c6c, 0x006f]);
    assert_eq!(w, [1, 4, END]);
    assert_eq!(decode(&d, &w).unwrap().args, ["hello"]);
}

#[test]
fn argv_preserves_empty_strings_spaces_and_japanese() {
    let (d, w) = encode(
        &["/usr/bin/printf", "%s|%s|%s", "", "a b", "日本語"],
        8192,
        128,
    )
    .unwrap();
    let command = decode(&d, &w).unwrap();
    assert_eq!(command.args, ["%s|%s|%s", "", "a b", "日本語"]);
}

#[test]
fn invalid_mailbox_layouts_are_rejected() {
    for (d, w) in [
        (vec![0x6161], vec![0]),
        (vec![0x6161], vec![END]),
        (vec![0x6161], vec![1, END]),
        (vec![0x6161, 0x6262], vec![0, 0, END]),
        (vec![0xffff], vec![0, END]),
        (vec![0x0061, 0x62], vec![1, END]),
        (vec![0], vec![0, END]),
    ] {
        assert!(decode(&d, &w).is_err(), "{d:?}, {w:?}");
    }
}

#[test]
fn trigger_clears_all_sd_and_busy_blocks_changes() {
    let config = Config {
        sd_words: 8,
        ..Config::default()
    };
    let mut memory = Memory::new(&config);
    let (d, w) = encode(&["echo", "old"], config.d_words, config.w_words).unwrap();
    assert!(memory.apply(write(Device::D, 0, d)).unwrap().1.is_none());
    memory.apply(write(Device::W, 0, w)).unwrap();
    let job = memory.apply(trigger()).unwrap().1.unwrap();
    assert_eq!(job.command.as_ref().unwrap().args, ["old"]);
    assert_eq!(memory.apply(trigger()).unwrap_err(), EndCode::Unavailable);
    assert_eq!(
        memory.apply(write(Device::D, 0, vec![123])).unwrap_err(),
        EndCode::Unavailable
    );
    assert_eq!(
        memory
            .apply(Operation::WriteBits {
                device: Device::Sm,
                start: 0,
                bits: vec![false]
            })
            .unwrap_err(),
        EndCode::Unavailable
    );
    memory.finish(
        job.sequence,
        Completion {
            status: Status::Success,
            exit_code: Some(0),
            signal: None,
            text: "old result".into(),
            truncated: false,
        },
    );
    assert_ne!(read(&mut memory, Device::Sd, 0, 8), vec![0; 8]);
    assert_eq!(read(&mut memory, Device::Sm, 0, 1)[0], 2);
    let second = memory.apply(trigger()).unwrap().1.unwrap();
    assert_eq!(read(&mut memory, Device::Sd, 0, 8), vec![0; 8]);
    assert_eq!(
        read(&mut memory, Device::Sw, 0, 1),
        [Status::Running as u16]
    );
    assert_eq!(second.sequence, job.sequence + 1);
}

#[test]
fn invalid_command_still_clears_log_and_completes() {
    let mut memory = Memory::new(&Config::default());
    let job = memory.apply(trigger()).unwrap().1.unwrap();
    assert!(job.command.is_err());
    memory.finish(
        job.sequence,
        Completion::failure(Status::InvalidCommand, "bad W\n".into()),
    );
    assert_eq!(read(&mut memory, Device::Sm, 0, 1), [0b110]);
    assert_eq!(
        read(&mut memory, Device::Sw, 0, 1),
        [Status::InvalidCommand as u16]
    );
}

#[test]
fn log_truncation_preserves_utf8_and_nul() {
    let mut memory = Memory::new(&Config {
        sd_words: 3,
        ..Config::default()
    });
    let job = memory.apply(trigger()).unwrap().1.unwrap();
    memory.finish(
        job.sequence,
        Completion {
            status: Status::Success,
            exit_code: Some(0),
            signal: None,
            text: "日本".into(),
            truncated: false,
        },
    );
    let bytes: Vec<u8> = read(&mut memory, Device::Sd, 0, 3)
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(&bytes[..3], "日".as_bytes());
    assert_eq!(&bytes[3..], [0, 0, 0]);
    assert_eq!(read(&mut memory, Device::Sw, 3, 2), [3, 0]);
    assert_eq!(read(&mut memory, Device::Sm, 0, 1), [0b10010]);
}

#[test]
fn out_of_range_write_is_atomic_and_outputs_are_read_only() {
    let mut memory = Memory::new(&Config {
        d_words: 2,
        ..Config::default()
    });
    assert_eq!(
        memory.apply(write(Device::D, 1, vec![3, 4])).unwrap_err(),
        EndCode::OutOfRange
    );
    assert_eq!(read(&mut memory, Device::D, 0, 2), [0, 0]);
    for device in [Device::Sd, Device::Sw, Device::Sm] {
        assert!(memory.apply(write(device, 1, vec![1])).is_err());
    }
}

#[test]
fn ql_and_iqr_golden_frames_match() {
    // Built independently from the public 3E layouts; each writes D0=0x1234.
    let ql = [
        0x50, 0, 0, 0xff, 0xff, 3, 0, 14, 0, 16, 0, 1, 0x14, 0, 0, 0, 0, 0, 0xa8, 1, 0, 0x34, 0x12,
    ];
    let iqr = [
        0x50, 0, 0, 0xff, 0xff, 3, 0, 16, 0, 16, 0, 1, 0x14, 2, 0, 0, 0, 0, 0, 0xa8, 0, 1, 0, 0x34,
        0x12,
    ];
    let expected = write(Device::D, 0, vec![0x1234]);
    assert_eq!(protocol::decode(&ql).unwrap(), expected);
    assert_eq!(protocol::decode(&iqr).unwrap(), expected);
    assert_eq!(
        protocol::response(&ql, Ok(vec![])),
        [0xd0, 0, 0, 0xff, 0xff, 3, 0, 2, 0, 0, 0]
    );
    for size in 0..ql.len() {
        assert!(protocol::decode(&ql[..size]).is_err());
    }
}

#[test]
fn bit_nibbles_and_error_response_are_validated() {
    let mut frame = vec![
        0x50, 0, 0, 0xff, 0xff, 3, 0, 13, 0, 16, 0, 1, 0x14, 1, 0, 0, 0, 0, 0x91, 1, 0, 0x10,
    ];
    assert_eq!(protocol::decode(&frame).unwrap(), trigger());
    *frame.last_mut().unwrap() = 0x11;
    assert_eq!(protocol::decode(&frame).unwrap_err(), EndCode::InvalidBit);
    *frame.last_mut().unwrap() = 0x20;
    assert_eq!(protocol::decode(&frame).unwrap_err(), EndCode::InvalidBit);
    let error = protocol::response(&frame, Err(EndCode::InvalidBit));
    assert_eq!(&error[9..11], [0x60, 0xc0]);
    assert_eq!(&error[11..], [0, 0xff, 0xff, 3, 0, 1, 0x14, 1, 0]);
}

#[test]
fn arbitrary_frames_do_not_panic() {
    // Deterministic malformed input sampling without a fuzzing dependency.
    let mut seed = 0x12345678u32;
    for length in 0..4096 {
        let mut data = vec![0u8; length];
        for byte in &mut data {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *byte = seed as u8;
        }
        if length >= 15 {
            data[..2].copy_from_slice(&[0x50, 0]);
            data[7..9].copy_from_slice(&((length - 9) as u16).to_le_bytes());
            let command = [0x0401u16, 0x1401, 0x0406, 0x1406][(length / 4) % 4];
            data[11..13].copy_from_slice(&command.to_le_bytes());
            data[13..15].copy_from_slice(&(length as u16 % 4).to_le_bytes());
        }
        let _ = protocol::decode(&data);
    }
}
