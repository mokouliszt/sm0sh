//! A single shared mailbox. One mutex covers an entire SLMP operation.
use crate::{
    command::{self, CommandSpec},
    config::Config,
    protocol::{Device, EndCode, Operation},
};

#[derive(Debug)]
pub struct Job {
    pub sequence: u32,
    pub command: Result<CommandSpec, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Status {
    Idle = 0,
    Running = 1,
    Success = 2,
    ExitFailure = 3,
    InvalidCommand = 4,
    SpawnFailure = 5,
    Timeout = 6,
    Cancelled = 7,
    IoFailure = 8,
}

#[derive(Debug)]
pub struct Completion {
    pub status: Status,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub text: String,
    pub truncated: bool,
}

impl Completion {
    pub fn failure(status: Status, text: String) -> Self {
        Self {
            status,
            exit_code: None,
            signal: None,
            text,
            truncated: false,
        }
    }
}

pub struct Memory {
    d: Vec<u16>,
    w: Vec<u16>,
    sd: Vec<u16>,
    sm: [bool; 16],
    sw: [u16; 8],
    sequence: u32,
}

impl Memory {
    pub fn new(config: &Config) -> Self {
        let mut sw = [0; 8];
        sw[1] = 0xffff;
        sw[7] = 1; // Mailbox ABI version, independent of application version.
        Self {
            d: vec![0; config.d_words],
            w: vec![command::END; config.w_words],
            sd: vec![0; config.sd_words],
            sm: [false; 16],
            sw,
            sequence: 0,
        }
    }

    pub fn apply(&mut self, operation: Operation) -> Result<(Vec<u8>, Option<Job>), EndCode> {
        match operation {
            Operation::Read {
                device,
                start,
                count,
                bits,
            } => {
                if device == Device::Sm {
                    if bits {
                        let range = checked_range(start, count, self.sm.len())?;
                        let values = &self.sm[range];
                        let data = values
                            .chunks(2)
                            .map(|pair| {
                                ((pair[0] as u8) << 4) | (*pair.get(1).unwrap_or(&false) as u8)
                            })
                            .collect();
                        Ok((data, None))
                    } else {
                        let range = checked_range(
                            start,
                            count.checked_mul(16).ok_or(EndCode::OutOfRange)?,
                            self.sm.len(),
                        )?;
                        let mut bytes = Vec::with_capacity(count * 2);
                        for word in self.sm[range].chunks_exact(16) {
                            let value = word
                                .iter()
                                .enumerate()
                                .fold(0u16, |acc, (i, &bit)| acc | ((bit as u16) << i));
                            bytes.extend_from_slice(&value.to_le_bytes());
                        }
                        Ok((bytes, None))
                    }
                } else {
                    if bits {
                        return Err(EndCode::InvalidDevice);
                    }
                    let values: &[u16] = match device {
                        Device::D => &self.d,
                        Device::W => &self.w,
                        Device::Sd => &self.sd,
                        Device::Sw => &self.sw,
                        Device::Sm => unreachable!(),
                    };
                    let range = checked_range(start, count, values.len())?;
                    Ok((
                        values[range]
                            .iter()
                            .flat_map(|word| word.to_le_bytes())
                            .collect(),
                        None,
                    ))
                }
            }
            Operation::WriteWords {
                device: Device::Sm,
                start: 0,
                words,
            } if words.len() == 1 && words[0] <= 1 => {
                self.trigger(words[0] != 0).map(|job| (Vec::new(), job))
            }
            Operation::WriteWords {
                device,
                start,
                words,
            } => {
                if self.sm[0] {
                    return Err(EndCode::Unavailable);
                }
                let target = match device {
                    Device::D => &mut self.d,
                    Device::W => &mut self.w,
                    _ => return Err(EndCode::InvalidDevice),
                };
                let range = checked_range(start, words.len(), target.len())?;
                target[range].copy_from_slice(&words);
                Ok((Vec::new(), None))
            }
            Operation::WriteBits {
                device: Device::Sm,
                start: 0,
                bits,
            } if bits.len() == 1 => self.trigger(bits[0]).map(|job| (Vec::new(), job)),
            Operation::WriteBits { .. } => Err(EndCode::InvalidDevice),
        }
    }

    fn trigger(&mut self, on: bool) -> Result<Option<Job>, EndCode> {
        if self.sm[0] {
            return Err(EndCode::Unavailable);
        }
        if !on {
            return Ok(None);
        }
        self.sd.fill(0);
        self.sm.fill(false);
        self.sm[0] = true;
        self.sequence = self.sequence.wrapping_add(1);
        self.sw.fill(0);
        self.sw[0] = Status::Running as u16;
        self.sw[1] = 0xffff;
        self.sw[5] = self.sequence as u16;
        self.sw[6] = (self.sequence >> 16) as u16;
        self.sw[7] = 1;
        Ok(Some(Job {
            sequence: self.sequence,
            command: command::decode(&self.d, &self.w),
        }))
    }

    /// Publish all fields atomically, then release SM0. Call under the same mutex as apply().
    pub fn finish(&mut self, sequence: u32, completion: Completion) {
        if sequence != self.sequence || !self.sm[0] {
            return;
        }
        let text = completion.text.replace('\0', "\u{fffd}");
        let capacity = self.sd.len() * 2 - 1; // Always retain a NUL terminator.
        let mut length = text.len().min(capacity);
        while !text.is_char_boundary(length) {
            length -= 1;
        }
        self.sd.fill(0);
        for (index, pair) in text.as_bytes()[..length].chunks(2).enumerate() {
            self.sd[index] = u16::from_le_bytes([pair[0], *pair.get(1).unwrap_or(&0)]);
        }
        self.sw[0] = completion.status as u16;
        self.sw[1] = completion.exit_code.map(|v| v as u16).unwrap_or(0xffff);
        self.sw[2] = completion.signal.unwrap_or(0) as u16;
        self.sw[3] = length as u16;
        self.sw[4] = (length >> 16) as u16;
        self.sm[1] = true;
        self.sm[2] = completion.status != Status::Success;
        self.sm[3] = completion.status == Status::Timeout;
        self.sm[4] = completion.truncated || length < text.len();
        self.sm[0] = false;
    }
}

fn checked_range(
    start: usize,
    count: usize,
    size: usize,
) -> Result<std::ops::Range<usize>, EndCode> {
    let end = start.checked_add(count).ok_or(EndCode::OutOfRange)?;
    if count == 0 || end > size {
        return Err(EndCode::OutOfRange);
    }
    Ok(start..end)
}
