use crate::protocol::{self, Channel, Frame, FrameIoError, CONTROL_ADDR, INPUT_ADDR};
use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransportAddrs {
    pub control: SocketAddr,
    pub input: SocketAddr,
}

impl TransportAddrs {
    pub fn loopback(control_port: u16, input_port: u16) -> Self {
        Self {
            control: SocketAddr::from((Ipv4Addr::LOCALHOST, control_port)),
            input: SocketAddr::from((Ipv4Addr::LOCALHOST, input_port)),
        }
    }
}

impl Default for TransportAddrs {
    fn default() -> Self {
        Self::loopback(28473, 28474)
    }
}

#[derive(Debug)]
pub struct TransportListeners {
    control: TcpListener,
    input: TcpListener,
}

impl TransportListeners {
    pub fn bind(addrs: TransportAddrs) -> Result<Self, TransportError> {
        let control = TcpListener::bind(addrs.control)?;
        let input = TcpListener::bind(addrs.input)?;
        Ok(Self { control, input })
    }

    pub fn bind_default() -> Result<Self, TransportError> {
        Self::bind(TransportAddrs::default())
    }

    pub fn local_addrs(&self) -> Result<TransportAddrs, TransportError> {
        Ok(TransportAddrs {
            control: self.control.local_addr()?,
            input: self.input.local_addr()?,
        })
    }

    pub fn accept(&self) -> Result<TransportConnection, TransportError> {
        let (control, _) = self.control.accept()?;
        let (input, _) = self.input.accept()?;

        Ok(TransportConnection {
            control: ChannelStream::new(Channel::Control, control)?,
            input: ChannelStream::new(Channel::Input, input)?,
        })
    }
}

#[derive(Debug)]
pub struct TransportConnection {
    pub control: ChannelStream,
    pub input: ChannelStream,
}

impl TransportConnection {
    pub fn connect(addrs: TransportAddrs) -> Result<Self, TransportError> {
        let control = TcpStream::connect(addrs.control)?;
        let input = TcpStream::connect(addrs.input)?;

        Ok(Self {
            control: ChannelStream::new(Channel::Control, control)?,
            input: ChannelStream::new(Channel::Input, input)?,
        })
    }

    pub fn connect_default() -> Result<Self, TransportError> {
        Self::connect(TransportAddrs::default())
    }

    pub fn connect_timeout(
        addrs: TransportAddrs,
        timeout: Duration,
    ) -> Result<Self, TransportError> {
        let control = TcpStream::connect_timeout(&addrs.control, timeout)?;
        let input = TcpStream::connect_timeout(&addrs.input, timeout)?;

        Ok(Self {
            control: ChannelStream::new(Channel::Control, control)?,
            input: ChannelStream::new(Channel::Input, input)?,
        })
    }
}

#[derive(Debug)]
pub struct ChannelStream {
    channel: Channel,
    stream: TcpStream,
}

impl ChannelStream {
    pub fn new(channel: Channel, stream: TcpStream) -> Result<Self, TransportError> {
        stream.set_nodelay(true)?;
        Ok(Self { channel, stream })
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.stream.local_addr().map_err(Into::into)
    }

    pub fn peer_addr(&self) -> Result<SocketAddr, TransportError> {
        self.stream.peer_addr().map_err(Into::into)
    }

    pub fn nodelay(&self) -> Result<bool, TransportError> {
        self.stream.nodelay().map_err(Into::into)
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<(), TransportError> {
        self.stream.set_read_timeout(timeout).map_err(Into::into)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> Result<(), TransportError> {
        self.stream.set_write_timeout(timeout).map_err(Into::into)
    }

    pub fn read_frame(&mut self) -> Result<Frame, TransportError> {
        protocol::read_frame(&mut self.stream, self.channel).map_err(Into::into)
    }

    pub fn write_frame(&mut self, frame: &Frame) -> Result<(), TransportError> {
        protocol::write_frame(&mut self.stream, frame, self.channel).map_err(Into::into)
    }

    pub fn into_inner(self) -> TcpStream {
        self.stream
    }
}

#[derive(Debug)]
pub enum TransportError {
    Io(io::Error),
    Frame(FrameIoError),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Frame(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Frame(error) => Some(error),
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<FrameIoError> for TransportError {
    fn from(value: FrameIoError) -> Self {
        Self::Frame(value)
    }
}

pub fn default_addrs() -> TransportAddrs {
    TransportAddrs::default()
}

pub fn default_addr_strings() -> (&'static str, &'static str) {
    (CONTROL_ADDR, INPUT_ADDR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MessageType, INPUT_PAYLOAD_LIMIT};
    use std::sync::mpsc;
    use std::thread;

    fn ephemeral_addrs() -> TransportAddrs {
        TransportAddrs::loopback(0, 0)
    }

    fn set_test_timeouts(connection: &TransportConnection) {
        let timeout = Some(Duration::from_secs(2));
        connection.control.set_read_timeout(timeout).unwrap();
        connection.control.set_write_timeout(timeout).unwrap();
        connection.input.set_read_timeout(timeout).unwrap();
        connection.input.set_write_timeout(timeout).unwrap();
    }

    #[test]
    fn default_addrs_match_protocol_constants() {
        let addrs = default_addrs();
        assert_eq!(CONTROL_ADDR, "127.0.0.1:28473");
        assert_eq!(INPUT_ADDR, "127.0.0.1:28474");
        assert_eq!(addrs.control, CONTROL_ADDR.parse().unwrap());
        assert_eq!(addrs.input, INPUT_ADDR.parse().unwrap());
    }

    #[test]
    fn bind_uses_two_distinct_loopback_ports() {
        let listeners = TransportListeners::bind(ephemeral_addrs()).unwrap();
        let addrs = listeners.local_addrs().unwrap();

        assert_eq!(addrs.control.ip(), Ipv4Addr::LOCALHOST);
        assert_eq!(addrs.input.ip(), Ipv4Addr::LOCALHOST);
        assert_ne!(addrs.control.port(), addrs.input.port());
    }

    #[test]
    fn connects_control_and_input_with_tcp_nodelay() {
        let listeners = TransportListeners::bind(ephemeral_addrs()).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let connection = listeners.accept().unwrap();
            assert_eq!(connection.control.channel(), Channel::Control);
            assert_eq!(connection.input.channel(), Channel::Input);
            assert!(connection.control.nodelay().unwrap());
            assert!(connection.input.nodelay().unwrap());
            connection
        });

        ready_rx.recv().unwrap();
        let client = TransportConnection::connect(addrs).unwrap();
        assert!(client.control.nodelay().unwrap());
        assert!(client.input.nodelay().unwrap());

        let server = server.join().unwrap();
        assert_eq!(client.control.peer_addr().unwrap(), addrs.control);
        assert_eq!(client.input.peer_addr().unwrap(), addrs.input);
        assert_eq!(
            server.control.peer_addr().unwrap(),
            client.control.local_addr().unwrap()
        );
        assert_eq!(
            server.input.peer_addr().unwrap(),
            client.input.local_addr().unwrap()
        );
    }

    #[test]
    fn exchanges_frames_on_separate_channels() {
        let listeners = TransportListeners::bind(ephemeral_addrs()).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let mut connection = listeners.accept().unwrap();
            set_test_timeouts(&connection);

            let hello = connection.control.read_frame().unwrap();
            assert_eq!(hello.header.message_type, MessageType::Hello);
            assert_eq!(hello.header.id, 1);
            assert_eq!(hello.payload, vec![0x44, 0x33, 0x22, 0x11]);

            let input_attach = connection.input.read_frame().unwrap();
            assert_eq!(input_attach.header.message_type, MessageType::InputAttach);
            assert_eq!(input_attach.header.id, 2);

            connection
                .input
                .write_frame(&Frame::new(
                    MessageType::InputReport,
                    10,
                    vec![2, 1, 2, 0, 0x79, 0x02],
                ))
                .unwrap();
            connection
                .control
                .write_frame(&Frame::new(MessageType::HelloOk, 1, vec![0, 0]))
                .unwrap();
        });

        ready_rx.recv().unwrap();
        let mut client = TransportConnection::connect(addrs).unwrap();
        set_test_timeouts(&client);

        client
            .control
            .write_frame(&Frame::new(
                MessageType::Hello,
                1,
                vec![0x44, 0x33, 0x22, 0x11],
            ))
            .unwrap();
        client
            .input
            .write_frame(&Frame::new(MessageType::InputAttach, 2, vec![0; 8]))
            .unwrap();

        let hello_ok = client.control.read_frame().unwrap();
        assert_eq!(hello_ok.header.message_type, MessageType::HelloOk);
        assert_eq!(hello_ok.header.id, 1);
        assert_eq!(hello_ok.payload, vec![0, 0]);

        let input_report = client.input.read_frame().unwrap();
        assert_eq!(input_report.header.message_type, MessageType::InputReport);
        assert_eq!(input_report.header.id, 10);
        assert_eq!(input_report.payload, vec![2, 1, 2, 0, 0x79, 0x02]);

        server.join().unwrap();
    }

    #[test]
    fn input_channel_rejects_oversized_frame_without_network_write() {
        let listeners = TransportListeners::bind(ephemeral_addrs()).unwrap();
        let addrs = listeners.local_addrs().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let connection = listeners.accept().unwrap();
            set_test_timeouts(&connection);
            connection
        });

        ready_rx.recv().unwrap();
        let mut client = TransportConnection::connect(addrs).unwrap();
        let frame = Frame::new(
            MessageType::InputReport,
            1,
            vec![0_u8; INPUT_PAYLOAD_LIMIT + 1],
        );

        assert!(matches!(
            client.input.write_frame(&frame),
            Err(TransportError::Frame(FrameIoError::PayloadTooLarge {
                channel: Channel::Input,
                payload_len,
                limit: INPUT_PAYLOAD_LIMIT,
            })) if payload_len == INPUT_PAYLOAD_LIMIT + 1
        ));

        drop(client);
        let _ = server.join().unwrap();
    }
}
