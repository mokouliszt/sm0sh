//! D holds UTF-8 argv bytes; W holds inclusive end-word offsets.

pub const END: u16 = 0xffff;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

/// Every element occupies at least one word. Only trailing NUL padding is removed.
pub fn decode(d: &[u16], w: &[u16]) -> Result<CommandSpec, String> {
    let count = w
        .iter()
        .position(|&value| value == END)
        .ok_or("W needs a 65535 terminator after the last element")?;
    if count == 0 {
        return Err("W0 contains the terminator; no command was supplied".into());
    }
    let mut start = 0usize;
    let mut argv = Vec::with_capacity(count);
    for (index, &end) in w[..count].iter().enumerate() {
        let end = end as usize;
        if end < start || end >= d.len() {
            return Err(format!(
                "W{index:X}={end} is outside the next D range (start {start})"
            ));
        }
        let mut bytes: Vec<u8> = d[start..=end]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        if bytes.contains(&0) {
            return Err(format!("element {index} contains an embedded NUL"));
        }
        let value =
            String::from_utf8(bytes).map_err(|_| format!("element {index} is not valid UTF-8"))?;
        argv.push(value);
        start = end + 1;
    }
    let program = argv.remove(0);
    if program.is_empty() {
        return Err("command is empty".into());
    }
    Ok(CommandSpec {
        program,
        args: argv,
    })
}

/// Encode argv for examples, PLC-side implementations and contract tests.
pub fn encode(
    argv: &[&str],
    d_words: usize,
    w_words: usize,
) -> Result<(Vec<u16>, Vec<u16>), String> {
    if argv.is_empty() || argv[0].is_empty() || argv.len() >= w_words {
        return Err("a command and room for the W terminator are required".into());
    }
    let mut d = Vec::new();
    let mut w = Vec::new();
    for value in argv {
        if value.as_bytes().contains(&0) {
            return Err("NUL is not allowed in argv".into());
        }
        let bytes = value.as_bytes();
        if bytes.is_empty() {
            d.push(0);
        }
        for pair in bytes.chunks(2) {
            d.push(u16::from_le_bytes([pair[0], *pair.get(1).unwrap_or(&0)]));
        }
        if d.len() > d_words || d.len() > END as usize {
            return Err("argv does not fit in D".into());
        }
        w.push((d.len() - 1) as u16);
    }
    w.push(END);
    Ok((d, w))
}
