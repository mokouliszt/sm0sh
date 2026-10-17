use sm0sh::{
    command::encode,
    config::Config,
    memory::{Completion, Memory, Status},
    protocol::{self, Device, EndCode, Operation, ReadBlock, WriteBlock},
};

fn frame(
    command: u16,
    subcommand: u16,
    word_blocks: u8,
    bit_blocks: u8,
    blocks: &[(u16, u32, u16, &[u16])],
) -> Vec<u8> {
    let mut body = vec![16, 0];
    body.extend(command.to_le_bytes());
    body.extend(subcommand.to_le_bytes());
    body.extend([word_blocks, bit_blocks]);
    for &(code, start, count, data) in blocks {
        if subcommand == 2 {
            body.extend(start.to_le_bytes());
            body.extend(code.to_le_bytes());
        } else {
            body.extend(&start.to_le_bytes()[..3]);
            body.push(code as u8);
        }
        body.extend(count.to_le_bytes());
        body.extend(data.iter().flat_map(|word| word.to_le_bytes()));
    }
    let mut request = vec![0x50, 0, 0, 0xff, 0xff, 3, 0];
    request.extend((body.len() as u16).to_le_bytes());
    request.extend(body);
    request
}

fn read(memory: &mut Memory, device: Device, count: usize) -> Vec<u8> {
    memory
        .apply(Operation::Read {
            device,
            start: 0,
            count,
            bits: false,
        })
        .unwrap()
        .0
}

fn completed() -> Memory {
    let config = Config {
        sd_words: 8,
        ..Config::default()
    };
    let mut memory = Memory::new(&config);
    let (d, w) = encode(&["echo", "previous"], config.d_words, config.w_words).unwrap();
    for (device, words) in [(Device::D, d), (Device::W, w)] {
        memory
            .apply(Operation::WriteWords {
                device,
                start: 0,
                words,
            })
            .unwrap();
    }
    let job = memory
        .apply(Operation::WriteBits {
            device: Device::Sm,
            start: 0,
            bits: vec![true],
        })
        .unwrap()
        .1
        .unwrap();
    memory.finish(
        job.sequence,
        Completion {
            status: Status::Success,
            exit_code: Some(0),
            signal: None,
            text: "previous".into(),
            truncated: false,
        },
    );
    memory
}

#[test]
fn ql_golden_write_has_interleaved_specs_and_word_packed_sm() {
    let request = [
        0x50, 0, 0, 0xff, 0xff, 3, 0, 0x20, 0, 16, 0, 6, 0x14, 0, 0, 2, 1, 0, 0, 0, 0xa8, 1, 0,
        0x34, 0x12, 0, 0, 0, 0xb4, 1, 0, 7, 0, 0, 0, 0, 0x91, 1, 0, 0, 0,
    ];
    assert_eq!(
        protocol::decode(&request).unwrap(),
        Operation::WriteBlocks(vec![
            WriteBlock {
                device: Device::D,
                start: 0,
                words: vec![0x1234]
            },
            WriteBlock {
                device: Device::W,
                start: 0,
                words: vec![7]
            },
            WriteBlock {
                device: Device::Sm,
                start: 0,
                words: vec![0]
            },
        ])
    );
}

#[test]
fn ql_and_iqr_reads_keep_word_blocks_before_bit_blocks() {
    for subcommand in [0, 2] {
        let request = frame(
            0x0406,
            subcommand,
            2,
            1,
            &[(0xb5, 0, 8, &[]), (0xa9, 0, 4, &[]), (0x91, 0, 1, &[])],
        );
        assert_eq!(
            protocol::decode(&request).unwrap(),
            Operation::ReadBlocks(vec![
                ReadBlock {
                    device: Device::Sw,
                    start: 0,
                    count: 8
                },
                ReadBlock {
                    device: Device::Sd,
                    start: 0,
                    count: 4
                },
                ReadBlock {
                    device: Device::Sm,
                    start: 0,
                    count: 1
                },
            ])
        );
        let mut memory = completed();
        let expected = [
            read(&mut memory, Device::Sw, 8),
            read(&mut memory, Device::Sd, 4),
            vec![2, 0],
        ]
        .concat();
        assert_eq!(
            memory.apply(protocol::decode(&request).unwrap()).unwrap().0,
            expected
        );
    }
}

#[test]
fn iqr_block_addresses_keep_all_32_bits() {
    let request = frame(0x0406, 2, 1, 0, &[(0xa8, 0x0100_0000, 1, &[])]);
    assert_eq!(
        protocol::decode(&request).unwrap(),
        Operation::ReadBlocks(vec![ReadBlock {
            device: Device::D,
            start: 0x0100_0000,
            count: 1
        },])
    );
}

#[test]
fn bit_only_blocks_use_word_data_and_off_preserves_results() {
    for subcommand in [0, 2] {
        let mut memory = completed();
        let previous = read(&mut memory, Device::Sd, 8);
        let off = frame(0x1406, subcommand, 0, 1, &[(0x91, 0, 1, &[0])]);
        assert!(memory
            .apply(protocol::decode(&off).unwrap())
            .unwrap()
            .1
            .is_none());
        assert_eq!(read(&mut memory, Device::Sd, 8), previous);
        let request = frame(0x0406, subcommand, 0, 1, &[(0x91, 0, 1, &[])]);
        assert_eq!(
            memory.apply(protocol::decode(&request).unwrap()).unwrap().0,
            [2, 0]
        );
        let on = frame(0x1406, subcommand, 0, 1, &[(0x91, 0, 1, &[1])]);
        let job = memory
            .apply(protocol::decode(&on).unwrap())
            .unwrap()
            .1
            .unwrap();
        assert_eq!(job.sequence, 2);
        assert_eq!(read(&mut memory, Device::Sd, 8), [0; 16]);
    }
}

#[test]
fn staging_blocks_preserves_previous_results() {
    for subcommand in [0, 2] {
        let mut memory = completed();
        let previous = read(&mut memory, Device::Sd, 8);
        let (d, w) = encode(&["echo", "next"], 8192, 128).unwrap();
        let request = frame(
            0x1406,
            subcommand,
            2,
            0,
            &[(0xa8, 0, d.len() as u16, &d), (0xb4, 0, w.len() as u16, &w)],
        );
        assert!(memory
            .apply(protocol::decode(&request).unwrap())
            .unwrap()
            .1
            .is_none());
        assert_eq!(read(&mut memory, Device::Sd, 8), previous);
        assert_eq!(read(&mut memory, Device::Sm, 1), [2, 0]);
    }
}

#[test]
fn combined_write_triggers_once_after_staging_and_busy_rejection_is_atomic() {
    for subcommand in [0, 2] {
        let mut memory = completed();
        let (d, w) = encode(&["echo", "next"], 8192, 128).unwrap();
        let request = frame(
            0x1406,
            subcommand,
            2,
            1,
            &[
                (0xa8, 0, d.len() as u16, &d),
                (0xb4, 0, w.len() as u16, &w),
                (0x91, 0, 1, &[1]),
            ],
        );
        let job = memory
            .apply(protocol::decode(&request).unwrap())
            .unwrap()
            .1
            .unwrap();
        assert_eq!(job.sequence, 2);
        assert_eq!(job.command.unwrap().args, ["next"]);
        assert_eq!(read(&mut memory, Device::Sd, 8), [0; 16]);
        assert_eq!(read(&mut memory, Device::Sm, 1), [1, 0]);
        let busy = frame(0x1406, subcommand, 1, 0, &[(0xa8, 0, 1, &[0x1234])]);
        assert_eq!(
            memory.apply(protocol::decode(&busy).unwrap()).unwrap_err(),
            EndCode::Unavailable
        );
        assert_eq!(
            read(&mut memory, Device::D, d.len()),
            d.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>()
        );
        assert_eq!(read(&mut memory, Device::Sd, 8), [0; 16]);
    }
}

#[test]
fn invalid_later_blocks_leave_devices_and_previous_results_unchanged() {
    for subcommand in [0, 2] {
        let requests = [
            (
                frame(
                    0x1406,
                    subcommand,
                    2,
                    0,
                    &[(0xa8, 0, 1, &[42]), (0xb4, 127, 2, &[1, 2])],
                ),
                EndCode::OutOfRange,
            ),
            (
                frame(
                    0x1406,
                    subcommand,
                    2,
                    0,
                    &[(0xa8, 0, 1, &[42]), (0xa9, 0, 1, &[1])],
                ),
                EndCode::InvalidDevice,
            ),
            (
                frame(
                    0x1406,
                    subcommand,
                    1,
                    1,
                    &[(0xa8, 0, 1, &[42]), (0x91, 0, 1, &[2])],
                ),
                EndCode::InvalidDevice,
            ),
            (
                frame(
                    0x1406,
                    subcommand,
                    1,
                    2,
                    &[(0xa8, 0, 1, &[42]), (0x91, 0, 1, &[1]), (0x91, 0, 1, &[0])],
                ),
                EndCode::InvalidRequest,
            ),
        ];
        for (request, expected) in requests {
            let mut memory = completed();
            let previous = [
                read(&mut memory, Device::D, 8),
                read(&mut memory, Device::W, 4),
                read(&mut memory, Device::Sd, 8),
                read(&mut memory, Device::Sw, 8),
                read(&mut memory, Device::Sm, 1),
            ];
            assert_eq!(
                memory
                    .apply(protocol::decode(&request).unwrap())
                    .unwrap_err(),
                expected
            );
            assert_eq!(
                [
                    read(&mut memory, Device::D, 8),
                    read(&mut memory, Device::W, 4),
                    read(&mut memory, Device::Sd, 8),
                    read(&mut memory, Device::Sw, 8),
                    read(&mut memory, Device::Sm, 1)
                ],
                previous
            );
        }
    }
}

#[test]
fn overlapping_word_blocks_follow_request_order() {
    let mut memory = Memory::new(&Config::default());
    let request = frame(
        0x1406,
        0,
        2,
        0,
        &[(0xa8, 0, 2, &[1, 2]), (0xa8, 1, 2, &[3, 4])],
    );
    memory.apply(protocol::decode(&request).unwrap()).unwrap();
    assert_eq!(read(&mut memory, Device::D, 3), [1, 0, 3, 0, 4, 0]);
}

#[test]
fn block_counts_and_total_read_points_are_bounded() {
    for (subcommand, maximum) in [(0, 120), (2, 60)] {
        for count in [maximum, maximum + 1] {
            let specs = vec![(0xa8, 0, 1, &[][..]); count];
            let request = frame(0x0406, subcommand, count as u8, 0, &specs);
            assert_eq!(protocol::decode(&request).is_ok(), count == maximum);
        }
        for total in [960, 961] {
            let request = frame(
                0x0406,
                subcommand,
                2,
                0,
                &[(0xa8, 0, 480, &[]), (0xa8, 1000, total - 480, &[])],
            );
            assert_eq!(protocol::decode(&request).is_ok(), total == 960);
        }
    }
}

#[test]
fn write_point_limits_include_per_block_overhead() {
    for (subcommand, maximum) in [(0, 952), (2, 942)] {
        for total in [maximum, maximum + 1] {
            let data = vec![0; total - 1];
            let request = frame(
                0x1406,
                subcommand,
                2,
                0,
                &[(0xa8, 0, data.len() as u16, &data), (0xb4, 0, 1, &[0])],
            );
            assert_eq!(protocol::decode(&request).is_ok(), total == maximum);
        }
    }
}

#[test]
fn malformed_block_requests_are_rejected() {
    for subcommand in [0, 2] {
        for request in [
            frame(0x0406, subcommand, 0, 0, &[]),
            frame(0x0406, subcommand, 1, 0, &[(0xa8, 0, 0, &[])]),
            frame(0x0406, subcommand, 1, 0, &[(0x91, 0, 1, &[])]),
            frame(0x0406, subcommand, 0, 1, &[(0xa8, 0, 1, &[])]),
            frame(0x0406, subcommand, 1, 0, &[(0xff, 0, 1, &[])]),
        ] {
            assert!(protocol::decode(&request).is_err());
        }
        let request = frame(
            0x1406,
            subcommand,
            2,
            1,
            &[
                (0xa8, 0, 2, &[1, 2]),
                (0xb4, 0, 1, &[0xffff]),
                (0x91, 0, 1, &[1]),
            ],
        );
        for length in 15..request.len() {
            let mut truncated = request[..length].to_vec();
            truncated[7..9].copy_from_slice(&((length - 9) as u16).to_le_bytes());
            assert!(protocol::decode(&truncated).is_err(), "prefix {length}");
        }
        let mut trailing = request;
        trailing.push(0);
        let size = trailing.len() - 9;
        trailing[7..9].copy_from_slice(&(size as u16).to_le_bytes());
        assert_eq!(
            protocol::decode(&trailing).unwrap_err(),
            EndCode::InvalidRequest
        );
    }
}

#[test]
fn random_access_and_bit_subcommands_remain_unsupported() {
    for (command, subcommand) in [(0x0403, 0), (0x1402, 0), (0x0406, 1), (0x1406, 3)] {
        let request = frame(command, subcommand, 1, 0, &[(0xa8, 0, 1, &[0])]);
        assert_eq!(
            protocol::decode(&request).unwrap_err(),
            EndCode::UnsupportedCommand
        );
    }
}
