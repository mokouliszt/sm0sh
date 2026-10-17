use crate::{
    config::{Config, Transport},
    executor,
    memory::{Completion, Job, Memory, Status},
    protocol,
};
use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{mpsc, watch, Semaphore},
    task::JoinSet,
    time,
};

struct Mailbox {
    memory: Arc<Mutex<Memory>>,
    jobs: mpsc::Sender<Job>,
}

impl Mailbox {
    fn handle(&self, frame: &[u8]) -> Vec<u8> {
        let result = protocol::decode(frame).and_then(|operation| {
            let mut memory = self.memory.lock().unwrap();
            let (data, job) = memory.apply(operation)?;
            if let Some(job) = job {
                let sequence = job.sequence;
                if self.jobs.try_send(job).is_err() {
                    memory.finish(
                        sequence,
                        Completion::failure(
                            Status::Cancelled,
                            "sm0sh: executor unavailable\n".into(),
                        ),
                    );
                    return Err(protocol::EndCode::Unavailable);
                }
            }
            Ok(data)
        });
        protocol::response(frame, result)
    }
}

/// Bind all requested transports before starting any background work.
pub struct Server {
    config: Config,
    tcp: Option<TcpListener>,
    udp: Option<UdpSocket>,
    address: SocketAddr,
}

impl Server {
    pub async fn bind(config: Config) -> io::Result<Self> {
        config
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let tcp = if config.transport != Transport::Udp {
            Some(TcpListener::bind(config.bind).await?)
        } else {
            None
        };
        let address = match &tcp {
            Some(tcp) => tcp.local_addr()?,
            None => config.bind,
        };
        let udp = if config.transport != Transport::Tcp {
            Some(UdpSocket::bind(address).await?)
        } else {
            None
        };
        let address = match &udp {
            Some(udp) => udp.local_addr()?,
            None => address,
        };
        Ok(Self {
            config,
            tcp,
            udp,
            address,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) -> io::Result<()> {
        let config = Arc::new(self.config);
        let (jobs, mut queue) = mpsc::channel::<Job>(1);
        let mailbox = Arc::new(Mailbox {
            memory: Arc::new(Mutex::new(Memory::new(&config))),
            jobs,
        });
        let (stop_sender, stop) = watch::channel(false);
        let worker_memory = mailbox.memory.clone();
        let worker_config = config.clone();
        let mut worker_stop = stop.clone();
        let worker = tokio::spawn(async move {
            loop {
                if *worker_stop.borrow() {
                    queue.close();
                }
                let job = tokio::select! {
                    biased;
                    _ = worker_stop.changed() => { queue.close(); continue; },
                    job = queue.recv() => match job { Some(job) => job, None => break },
                };
                let result = match job.command {
                    Ok(command) => {
                        executor::execute(&command, &worker_config, worker_stop.clone()).await
                    }
                    Err(error) => {
                        Completion::failure(Status::InvalidCommand, format!("sm0sh: {error}\n"))
                    }
                };
                eprintln!(
                    "execution={} status={:?} exit={:?}",
                    job.sequence, result.status, result.exit_code
                );
                worker_memory.lock().unwrap().finish(job.sequence, result);
            }
        });
        let mut listeners = JoinSet::new();
        if let Some(tcp) = self.tcp {
            listeners.spawn(serve_tcp(
                tcp,
                mailbox.clone(),
                config.clone(),
                stop.clone(),
            ));
        }
        if let Some(udp) = self.udp {
            listeners.spawn(serve_udp(
                udp,
                mailbox.clone(),
                config.clone(),
                stop.clone(),
            ));
        }
        let already_stopping = *shutdown.borrow();
        let result = if already_stopping {
            Ok(())
        } else {
            tokio::select! {
                _ = shutdown.changed() => Ok(()),
                result = listeners.join_next() => match result {
                    Some(Ok(result)) => result,
                    Some(Err(error)) => Err(io::Error::other(error)),
                    None => Ok(()),
                },
            }
        };
        let _ = stop_sender.send(true);
        while listeners.join_next().await.is_some() {}
        drop(mailbox);
        worker.await.map_err(io::Error::other)?;
        result
    }
}

async fn serve_tcp(
    listener: TcpListener,
    mailbox: Arc<Mailbox>,
    config: Arc<Config>,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let permits = Arc::new(Semaphore::new(config.max_connections));
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept() => {
                let (socket, peer) = accepted?;
                if !config.allows(peer.ip()) { continue; }
                let permit = match permits.clone().try_acquire_owned() { Ok(permit) => permit, Err(_) => continue };
                let mailbox = mailbox.clone();
                let config = config.clone();
                let mut stop = stop.clone();
                connections.spawn(async move {
                    let _permit = permit;
                    tokio::select! {
                        _ = stop.changed() => {},
                        _ = connection(socket, mailbox, config) => {},
                    }
                });
            }
        }
    }
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    Ok(())
}

async fn connection(
    mut stream: TcpStream,
    mailbox: Arc<Mailbox>,
    config: Arc<Config>,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    loop {
        let frame = time::timeout(config.connection_timeout(), async {
            let mut header = [0u8; protocol::HEADER_LEN];
            stream.read_exact(&mut header).await?;
            let body_length = protocol::body_length(&header)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid 3E header"))?;
            let mut frame = header.to_vec();
            frame.resize(protocol::HEADER_LEN + body_length, 0);
            stream
                .read_exact(&mut frame[protocol::HEADER_LEN..])
                .await?;
            Ok::<_, io::Error>(frame)
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "frame timeout"))??;
        let response = mailbox.handle(&frame);
        time::timeout(config.connection_timeout(), stream.write_all(&response))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "response timeout"))??;
    }
}

async fn serve_udp(
    socket: UdpSocket,
    mailbox: Arc<Mailbox>,
    config: Arc<Config>,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    // Full datagram buffer lets us reject excess data instead of accepting a truncated command.
    let mut buffer = vec![0u8; 65536];
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => return Ok(()),
            received = socket.recv_from(&mut buffer) => {
                let (size, peer) = received?;
                if !config.allows(peer.ip()) || size < protocol::HEADER_LEN { continue; }
                let frame = &buffer[..size];
                let body_length = match protocol::body_length(&frame[..protocol::HEADER_LEN]) {
                    Ok(size) => size, Err(_) => continue,
                };
                if frame.len() != protocol::HEADER_LEN + body_length { continue; }
                let response = mailbox.handle(frame);
                let _ = time::timeout(config.connection_timeout(), socket.send_to(&response, peer)).await;
            }
        }
    }
}
