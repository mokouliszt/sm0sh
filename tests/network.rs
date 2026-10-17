use sm0sh::{
    command::encode,
    config::{Config, Transport},
    server::Server,
};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
    sync::watch,
};

fn frame(
    command: u16,
    subcommand: u16,
    device: u16,
    start: u32,
    count: u16,
    data: &[u8],
) -> Vec<u8> {
    let mut body = vec![16, 0];
    body.extend(command.to_le_bytes());
    body.extend(subcommand.to_le_bytes());
    if subcommand >= 2 {
        body.extend(start.to_le_bytes());
        body.extend(device.to_le_bytes());
    } else {
        body.extend(&start.to_le_bytes()[..3]);
        body.push(device as u8);
    }
    body.extend(count.to_le_bytes());
    body.extend(data);
    let mut result = vec![0x50, 0, 0, 0xff, 0xff, 3, 0];
    result.extend((body.len() as u16).to_le_bytes());
    result.extend(body);
    result
}

async fn receive(stream: &mut TcpStream) -> (u16, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut header = [0; 9];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(&header[..7], [0xd0, 0, 0, 0xff, 0xff, 3, 0]);
        let mut body = vec![0; u16::from_le_bytes([header[7], header[8]]) as usize];
        stream.read_exact(&mut body).await.unwrap();
        (u16::from_le_bytes([body[0], body[1]]), body[2..].to_vec())
    })
    .await
    .unwrap()
}

async fn request(stream: &mut TcpStream, data: &[u8]) -> (u16, Vec<u8>) {
    stream.write_all(data).await.unwrap();
    receive(stream).await
}

async fn read_words(stream: &mut TcpStream, code: u16, count: u16) -> Vec<u16> {
    let (code, data) = request(stream, &frame(0x0401, 0, code, 0, count, &[])).await;
    assert_eq!(code, 0);
    data.chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect()
}

async fn start(
    config: Config,
) -> (
    SocketAddr,
    watch::Sender<bool>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let server = Server::bind(Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        ..config
    })
    .await
    .unwrap();
    let address = server.local_addr();
    let (sender, stop) = watch::channel(false);
    (address, sender, tokio::spawn(server.run(stop)))
}

async fn stage(stream: &mut TcpStream, argv: &[&str]) {
    let (d, w) = encode(argv, 8192, 128).unwrap();
    for (code, values) in [(0xa8, d), (0xb4, w)] {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(
            request(
                stream,
                &frame(0x1401, 0, code, 0, values.len() as u16, &bytes)
            )
            .await
            .0,
            0
        );
    }
}

async fn finish(stream: &mut TcpStream) -> Vec<u16> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = read_words(stream, 0xb5, 8).await;
            if state[0] >= 2 {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn fragmented_and_coalesced_frames_are_processed_once() {
    let (address, stop, server) = start(Config::default()).await;
    let mut socket = TcpStream::connect(address).await.unwrap();
    let write = frame(0x1401, 0, 0xa8, 0, 1, &[0x34, 0x12]);
    for byte in write {
        socket.write_all(&[byte]).await.unwrap();
    }
    assert_eq!(receive(&mut socket).await.0, 0);
    let read = frame(0x0401, 0, 0xa8, 0, 1, &[]);
    socket
        .write_all(&[read.clone(), read].concat())
        .await
        .unwrap();
    for _ in 0..2 {
        assert_eq!(receive(&mut socket).await, (0, vec![0x34, 0x12]));
    }
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn two_connections_cannot_start_two_jobs_and_sd_is_reset() {
    let (address, stop, server) = start(Config::default()).await;
    let mut first = TcpStream::connect(address).await.unwrap();
    let mut second = TcpStream::connect(address).await.unwrap();
    stage(&mut first, &["/bin/echo", "previous"]).await;
    let trigger = frame(0x1401, 1, 0x91, 0, 1, &[0x10]);
    assert_eq!(request(&mut first, &trigger).await.0, 0);
    assert_eq!(finish(&mut first).await[0], 2);
    assert_ne!(read_words(&mut first, 0xa9, 8).await, vec![0; 8]);
    stage(&mut first, &["/bin/sleep", "0.2"]).await;
    let (one, two) = tokio::join!(
        request(&mut first, &trigger),
        request(&mut second, &trigger)
    );
    let mut codes = [one.0, two.0];
    codes.sort();
    assert_eq!(codes, [0, 0xcee0]);
    assert_eq!(read_words(&mut first, 0xa9, 8).await, vec![0; 8]);
    let status = finish(&mut first).await;
    assert_eq!(status[0], 2);
    assert_eq!(status[5], 2);
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn udp_supports_iqr_device_spec_and_rejects_extra_datagram_bytes() {
    let (address, stop, server) = start(Config {
        transport: Transport::Both,
        ..Config::default()
    })
    .await;
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    udp.connect(address).await.unwrap();
    let mut buffer = [0u8; 100];
    let write = frame(0x1401, 2, 0xb4, 0, 1, &[0x78, 0x56]);
    udp.send(&[write.clone(), vec![0xff]].concat())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), udp.recv(&mut buffer))
            .await
            .is_err()
    );
    udp.send(&write).await.unwrap();
    let size = tokio::time::timeout(Duration::from_secs(3), udp.recv(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buffer[..size], [0xd0, 0, 0, 0xff, 0xff, 3, 0, 2, 0, 0, 0]);
    udp.send(&frame(0x0401, 2, 0xb4, 0, 1, &[])).await.unwrap();
    let size = tokio::time::timeout(Duration::from_secs(3), udp.recv(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buffer[9..size], [0, 0, 0x78, 0x56]);
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn denied_peers_and_malformed_frames_cannot_modify_the_mailbox() {
    let (address, stop, server) = start(Config {
        allowed_ips: vec!["192.0.2.1".parse().unwrap()],
        ..Config::default()
    })
    .await;
    let mut socket = TcpStream::connect(address).await.unwrap();
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), socket.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();

    let (address, stop, server) = start(Config::default()).await;
    let mut socket = TcpStream::connect(address).await.unwrap();
    let mut malformed = frame(0x1401, 0, 0xa8, 0, 2, &[1, 0]);
    assert_eq!(request(&mut socket, &malformed).await.0, 0xc050);
    assert_eq!(read_words(&mut socket, 0xa8, 2).await, [0, 0]);
    malformed[0] = 0x54; // 4E is intentionally unsupported.
    socket.write_all(&malformed).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), socket.read(&mut byte))
            .await
            .unwrap()
            .unwrap_or(0),
        0
    );
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn shutdown_waits_for_active_command_cleanup() {
    let (address, stop, server) = start(Config::default()).await;
    let mut socket = TcpStream::connect(address).await.unwrap();
    stage(&mut socket, &["/bin/sleep", "30"]).await;
    assert_eq!(
        request(&mut socket, &frame(0x1401, 1, 0x91, 0, 1, &[0x10]))
            .await
            .0,
        0
    );
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

fn block_frame(
    command: u16,
    subcommand: u16,
    word_blocks: u8,
    bit_blocks: u8,
    blocks: &[(u16, u16, &[u16])],
) -> Vec<u8> {
    let mut body = vec![16, 0];
    body.extend(command.to_le_bytes());
    body.extend(subcommand.to_le_bytes());
    body.extend([word_blocks, bit_blocks]);
    for &(device, count, words) in blocks {
        if subcommand == 2 {
            body.extend(0u32.to_le_bytes());
            body.extend(device.to_le_bytes());
        } else {
            body.extend([0, 0, 0, device as u8]);
        }
        body.extend(count.to_le_bytes());
        body.extend(words.iter().flat_map(|word| word.to_le_bytes()));
    }
    let mut frame = vec![0x50, 0, 0, 0xff, 0xff, 3, 0];
    frame.extend((body.len() as u16).to_le_bytes());
    frame.extend(body);
    frame
}

async fn exchange_block(
    tcp: &mut TcpStream,
    udp: &UdpSocket,
    use_udp: bool,
    frame: &[u8],
) -> (u16, Vec<u8>) {
    if !use_udp {
        return request(tcp, frame).await;
    }
    udp.send(frame).await.unwrap();
    let mut response = [0; 2048];
    let length = tokio::time::timeout(Duration::from_secs(5), udp.recv(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&response[..7], [0xd0, 0, 0, 0xff, 0xff, 3, 0]);
    assert_eq!(
        length,
        9 + u16::from_le_bytes([response[7], response[8]]) as usize
    );
    (
        u16::from_le_bytes([response[9], response[10]]),
        response[11..length].to_vec(),
    )
}

#[tokio::test]
async fn block_commands_execute_and_read_results_over_tcp_udp_ql_and_iqr() {
    for use_udp in [false, true] {
        for subcommand in [0, 2] {
            let (address, stop, server) = start(Config {
                transport: Transport::Both,
                sd_words: 16,
                ..Config::default()
            })
            .await;
            let mut tcp = TcpStream::connect(address).await.unwrap();
            let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            udp.connect(address).await.unwrap();
            let invalid = block_frame(
                0x1406,
                subcommand,
                2,
                0,
                &[(0xa8, 1, &[0x1234]), (0xa9, 1, &[1])],
            );
            assert_eq!(
                exchange_block(&mut tcp, &udp, use_udp, &invalid).await.0,
                0xc05b
            );
            assert_eq!(read_words(&mut tcp, 0xa8, 1).await, [0]);
            for (sequence, argument) in [(1, "previous"), (2, "ok")] {
                let (d, w) = encode(&["/bin/echo", argument], 8192, 128).unwrap();
                let write = block_frame(
                    0x1406,
                    subcommand,
                    2,
                    1,
                    &[
                        (0xa8, d.len() as u16, &d),
                        (0xb4, w.len() as u16, &w),
                        (0x91, 1, &[1]),
                    ],
                );
                assert_eq!(
                    exchange_block(&mut tcp, &udp, use_udp, &write).await,
                    (0, vec![])
                );
                let read = block_frame(
                    0x0406,
                    subcommand,
                    2,
                    1,
                    &[(0xb5, 8, &[]), (0xa9, 16, &[]), (0x91, 1, &[])],
                );
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        let (code, data) = exchange_block(&mut tcp, &udp, use_udp, &read).await;
                        assert_eq!(code, 0);
                        assert_eq!(data.len(), 50);
                        let status = u16::from_le_bytes([data[0], data[1]]);
                        if status >= 2 {
                            assert_eq!(status, 2);
                            assert_eq!(u16::from_le_bytes([data[10], data[11]]), sequence);
                            let length = u16::from_le_bytes([data[6], data[7]]) as usize;
                            assert_eq!(&data[16..16 + length], format!("{argument}\n").as_bytes());
                            assert!(data[16 + length..48].iter().all(|&byte| byte == 0));
                            assert_eq!(&data[48..], [2, 0]);
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
            }
            stop.send(true).unwrap();
            server.await.unwrap().unwrap();
        }
    }
}
