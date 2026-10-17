use plc_comm_slmp::{
    SlmpClient, SlmpCommand, SlmpConnectionOptions, SlmpPlcProfile, SlmpTargetAddress,
    SlmpTransportMode,
};
use std::{
    error::Error,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn spec(device: u16, start: u32, count: u16, iqr: bool) -> Vec<u8> {
    let mut result = Vec::new();
    if iqr {
        result.extend(start.to_le_bytes());
        result.extend(device.to_le_bytes());
    } else {
        result.extend(&start.to_le_bytes()[..3]);
        result.push(device as u8);
    }
    result.extend(count.to_le_bytes());
    result
}

async fn read(
    client: &SlmpClient,
    device: u16,
    start: u32,
    count: u16,
    iqr: bool,
) -> Result<Vec<u16>> {
    let bytes = client
        .raw_command(
            SlmpCommand::DeviceRead,
            if iqr { 2 } else { 0 },
            &spec(device, start, count, iqr),
        )
        .await?;
    Ok(bytes
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect())
}

#[tokio::main]
async fn main() -> Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .ok_or("provide a localhost port")?
        .parse()?;
    for transport in [SlmpTransportMode::Tcp, SlmpTransportMode::Udp] {
        for iqr in [false, true] {
            let options = SlmpConnectionOptions::new(
                "127.0.0.1",
                port,
                transport,
                SlmpTargetAddress::default(),
                SlmpPlcProfile::IqF,
            )?;
            let client = SlmpClient::connect(options).await?;
            let mut d = Vec::<u16>::new();
            let mut w = Vec::<u16>::new();
            for element in ["/usr/bin/printf", "%s|%s|%s", "日本語", "", "a b"] {
                if element.is_empty() {
                    d.push(0);
                }
                for pair in element.as_bytes().chunks(2) {
                    d.push(u16::from_le_bytes([pair[0], *pair.get(1).unwrap_or(&0)]));
                }
                w.push((d.len() - 1) as u16);
            }
            w.push(65535);
            for (device, values) in [(0xa8, d), (0xb4, w)] {
                let mut payload = spec(device, 0, values.len() as u16, iqr);
                payload.extend(values.iter().flat_map(|v| v.to_le_bytes()));
                client
                    .raw_command(SlmpCommand::DeviceWrite, if iqr { 2 } else { 0 }, &payload)
                    .await?;
            }
            let mut payload = spec(0x91, 0, 1, iqr);
            payload.push(0x10);
            client
                .raw_command(SlmpCommand::DeviceWrite, if iqr { 3 } else { 1 }, &payload)
                .await?;
            let deadline = Instant::now() + Duration::from_secs(5);
            let state = loop {
                let state = read(&client, 0xb5, 0, 8, iqr).await?;
                if state[0] >= 2 {
                    break state;
                }
                if Instant::now() >= deadline {
                    return Err("completion timeout".into());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            assert_eq!(state[0], 2);
            let length = state[3] as usize | ((state[4] as usize) << 16);
            let words = read(&client, 0xa9, 0, length.div_ceil(2) as u16, iqr).await?;
            let bytes: Vec<u8> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
            assert_eq!(std::str::from_utf8(&bytes[..length])?, "日本語||a b");
            client.close().await?;
            println!(
                "PASS plc-comm-slmp-rust {transport:?} {}",
                if iqr { "iQR" } else { "Q/L" }
            );
        }
    }
    Ok(())
}
