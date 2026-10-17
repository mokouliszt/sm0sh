//! Binary 3E only. Decoding never changes application state.

pub const HEADER_LEN: usize = 9;
pub const MAX_BODY: usize = 4096;
pub const MAX_WORD_POINTS: usize = 960;
pub const MAX_BIT_POINTS: usize = 7168;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EndCode {
    InvalidRequest = 0xc050,
    WordPoints = 0xc051,
    BitPoints = 0xc052,
    OutOfRange = 0xc056,
    UnsupportedCommand = 0xc059,
    InvalidDevice = 0xc05b,
    InvalidBit = 0xc060,
    Unavailable = 0xcee0,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    D,
    W,
    Sd,
    Sm,
    Sw,
}

impl TryFrom<u16> for Device {
    type Error = EndCode;
    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0xa8 => Ok(Self::D),
            0xb4 => Ok(Self::W),
            0xa9 => Ok(Self::Sd),
            0x91 => Ok(Self::Sm),
            0xb5 => Ok(Self::Sw),
            _ => Err(EndCode::InvalidDevice),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReadBlock {
    pub device: Device,
    pub start: usize,
    pub count: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct WriteBlock {
    pub device: Device,
    pub start: usize,
    pub words: Vec<u16>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Operation {
    Read {
        device: Device,
        start: usize,
        count: usize,
        bits: bool,
    },
    WriteWords {
        device: Device,
        start: usize,
        words: Vec<u16>,
    },
    WriteBits {
        device: Device,
        start: usize,
        bits: Vec<bool>,
    },
    ReadBlocks(Vec<ReadBlock>),
    WriteBlocks(Vec<WriteBlock>),
}

pub fn body_length(header: &[u8]) -> Result<usize, EndCode> {
    if header.len() != HEADER_LEN || header[..2] != [0x50, 0] {
        return Err(EndCode::InvalidRequest);
    }
    let size = u16::from_le_bytes([header[7], header[8]]) as usize;
    if !(6..=MAX_BODY).contains(&size) {
        return Err(EndCode::InvalidRequest);
    }
    Ok(size)
}

pub fn decode(frame: &[u8]) -> Result<Operation, EndCode> {
    if frame.len() < HEADER_LEN {
        return Err(EndCode::InvalidRequest);
    }
    let size = body_length(&frame[..HEADER_LEN])?;
    if frame.len() != HEADER_LEN + size {
        return Err(EndCode::InvalidRequest);
    }
    let command = u16::from_le_bytes([frame[11], frame[12]]);
    let subcommand = u16::from_le_bytes([frame[13], frame[14]]);
    if !matches!(command, 0x0401 | 0x1401 | 0x0406 | 0x1406) {
        return Err(EndCode::UnsupportedCommand);
    }
    let (spec_size, bits) = match subcommand {
        0 => (4, false),
        1 => (4, true),
        2 => (6, false),
        3 => (6, true),
        _ => return Err(EndCode::UnsupportedCommand),
    };
    let payload = &frame[15..];
    if matches!(command, 0x0406 | 0x1406) {
        if bits {
            return Err(EndCode::UnsupportedCommand);
        }
        return decode_blocks(payload, spec_size, command == 0x1406);
    }
    if payload.len() < spec_size + 2 {
        return Err(EndCode::InvalidRequest);
    }
    let (start, code) = if spec_size == 4 {
        (
            u32::from_le_bytes([payload[0], payload[1], payload[2], 0]),
            payload[3] as u16,
        )
    } else {
        (
            u32::from_le_bytes(payload[..4].try_into().unwrap()),
            u16::from_le_bytes([payload[4], payload[5]]),
        )
    };
    let device = Device::try_from(code)?;
    if bits && device != Device::Sm {
        return Err(EndCode::InvalidDevice);
    }
    let count = u16::from_le_bytes([payload[spec_size], payload[spec_size + 1]]) as usize;
    let maximum = if bits {
        MAX_BIT_POINTS
    } else {
        MAX_WORD_POINTS
    };
    if count == 0 || count > maximum {
        return Err(if bits {
            EndCode::BitPoints
        } else {
            EndCode::WordPoints
        });
    }
    let data = &payload[spec_size + 2..];
    if command == 0x0401 {
        if !data.is_empty() {
            return Err(EndCode::InvalidRequest);
        }
        return Ok(Operation::Read {
            device,
            start: start as usize,
            count,
            bits,
        });
    }
    let expected = if bits { count.div_ceil(2) } else { count * 2 };
    if data.len() != expected {
        return Err(EndCode::InvalidRequest);
    }
    if bits {
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            let value = if index % 2 == 0 {
                data[index / 2] >> 4
            } else {
                data[index / 2] & 0xf
            };
            match value {
                0 => values.push(false),
                1 => values.push(true),
                _ => return Err(EndCode::InvalidBit),
            }
        }
        if count % 2 != 0 && data[data.len() - 1] & 0xf != 0 {
            return Err(EndCode::InvalidBit);
        }
        Ok(Operation::WriteBits {
            device,
            start: start as usize,
            bits: values,
        })
    } else {
        Ok(Operation::WriteWords {
            device,
            start: start as usize,
            words: data
                .chunks_exact(2)
                .map(|v| u16::from_le_bytes([v[0], v[1]]))
                .collect(),
        })
    }
}

fn decode_blocks(payload: &[u8], spec_size: usize, write: bool) -> Result<Operation, EndCode> {
    if payload.len() < 2 {
        return Err(EndCode::InvalidRequest);
    }
    let word_blocks = payload[0] as usize;
    let blocks = word_blocks + payload[1] as usize;
    let maximum_blocks = if spec_size == 4 { 120 } else { 60 };
    if blocks == 0 || blocks > maximum_blocks {
        return Err(EndCode::WordPoints);
    }
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut cursor = 2;
    let mut total_points = 0;
    for index in 0..blocks {
        let spec = payload
            .get(cursor..cursor + spec_size + 2)
            .ok_or(EndCode::InvalidRequest)?;
        let (start, code) = if spec_size == 4 {
            (
                u32::from_le_bytes([spec[0], spec[1], spec[2], 0]),
                spec[3] as u16,
            )
        } else {
            (
                u32::from_le_bytes(spec[..4].try_into().unwrap()),
                u16::from_le_bytes([spec[4], spec[5]]),
            )
        };
        let device = Device::try_from(code)?;
        if (device == Device::Sm) != (index >= word_blocks) {
            return Err(EndCode::InvalidDevice);
        }
        let count = u16::from_le_bytes([spec[spec_size], spec[spec_size + 1]]) as usize;
        total_points += count;
        if count == 0 || total_points > MAX_WORD_POINTS {
            return Err(EndCode::WordPoints);
        }
        cursor += spec_size + 2;
        if write {
            let data = payload
                .get(cursor..cursor + count * 2)
                .ok_or(EndCode::InvalidRequest)?;
            writes.push(WriteBlock {
                device,
                start: start as usize,
                words: data
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect(),
            });
            cursor += count * 2;
        } else {
            reads.push(ReadBlock {
                device,
                start: start as usize,
                count,
            });
        }
    }
    if cursor != payload.len() {
        return Err(EndCode::InvalidRequest);
    }
    let overhead = if spec_size == 4 { 4 } else { 9 };
    if write && total_points + blocks * overhead > MAX_WORD_POINTS {
        return Err(EndCode::WordPoints);
    }
    Ok(if write {
        Operation::WriteBlocks(writes)
    } else {
        Operation::ReadBlocks(reads)
    })
}

pub fn response(frame: &[u8], result: Result<Vec<u8>, EndCode>) -> Vec<u8> {
    let route = &frame[2..7]; // Caller provides at least a validated 9-byte header.
    let (code, payload) = match result {
        Ok(data) => (0u16, data),
        Err(code) => {
            let mut info = route.to_vec();
            if frame.len() >= 15 {
                info.extend_from_slice(&frame[11..15]);
            }
            (code as u16, info)
        }
    };
    let mut output = vec![0xd0, 0];
    output.extend_from_slice(route);
    output.extend_from_slice(&((2 + payload.len()) as u16).to_le_bytes());
    output.extend_from_slice(&code.to_le_bytes());
    output.extend_from_slice(&payload);
    output
}
