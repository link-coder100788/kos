use core::{fmt::Display, net::Ipv4Addr};


const NTP_TO_UNIX_EPOCH: u64 = 2_208_988_800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stratum {
    UnspecifiedOrInvalid,    // Stratum 0
    PrimaryReference,        // Stratum 1
    SecondaryReference(u8),  // Stratum 2 to 15
    Reserved(u8),            // Stratum 16 to 255
}

impl From<u8> for Stratum {
    fn from(val: u8) -> Self {
        match val {
            0 => Stratum::UnspecifiedOrInvalid,
            1 => Stratum::PrimaryReference,
            2..=15 => Stratum::SecondaryReference(val),
            _ => Stratum::Reserved(val),
        }
    }
}

/// Fixed-point 64-bit timestamp used by NTP.
/// Contains 32 bits for whole seconds, and 32 bits for fractional seconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NtpTimestamp {
    pub seconds: u32,
    pub fraction: u32,
}

impl NtpTimestamp {
    pub fn empty() -> Self {
        Self { seconds: 0, fraction: 0 }
    }

    /// Creates an NTP timestamp from an array of 8 big-endian bytes.
    pub fn from_be_bytes(bytes: [u8; 8]) -> Self {
        Self {
            seconds: u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            fraction: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }

    /// Converts the timestamp structural components back to big-endian network bytes.
    pub fn to_be_bytes(&self) -> [u8; 8] {
        let s = self.seconds.to_be_bytes();
        let f = self.fraction.to_be_bytes();
        [s[0], s[1], s[2], s[3], f[0], f[1], f[2], f[3]]
    }

    /// Converts the NTP internal clock time to a standard Unix timestamp.
    pub fn to_unix_seconds(&self) -> u64 {
        if (self.seconds as u64) < NTP_TO_UNIX_EPOCH {
            // Handle NTP era rollover wrap-around
            return (self.seconds as u64) + (1u64 << 32) - NTP_TO_UNIX_EPOCH;
        }
        (self.seconds as u64) - NTP_TO_UNIX_EPOCH
    }

    /// Normalizes the 32-bit fractional clock tick component into milliseconds.
    pub fn to_milliseconds(&self) -> u32 {
        ((self.fraction as u64 * 1000) >> 32) as u32
    }
}

/// A strongly typed representation of a standard 48-byte NTP packet payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NtpPacket {
    pub li_vn_mode: u8,
    pub stratum: Stratum,
    pub poll: i16,
    pub precision: i16,
    pub root_delay: [u8; 4],
    pub root_dispersion: [u8; 4],
    pub reference_id: [u8; 4],
    pub reference_timestamp: NtpTimestamp,
    pub origin_timestamp: NtpTimestamp,
    pub receive_timestamp: NtpTimestamp,
    pub transmit_timestamp: NtpTimestamp,
}

impl NtpPacket {
    pub fn empty() -> Self {
        Self {
            li_vn_mode: 0,
            stratum: Stratum::Reserved(255),
            poll: -255, precision: -255,
            root_delay: [0, 0, 0, 0],
            root_dispersion: [0, 0, 0, 0],
            reference_id: [0, 0, 0, 0],
            reference_timestamp: NtpTimestamp::empty(),
            origin_timestamp: NtpTimestamp::empty(),
            receive_timestamp: NtpTimestamp::empty(),
            transmit_timestamp: NtpTimestamp::empty()
        }
    }

    /// Generates a clean default packet formatted as an NTP Version 4 Client request.
    pub fn new_client_request() -> Self {
        Self {
            // Leap Indicator = 0, Version = 4, Mode = 3 (Client)
            // Binary: 00 100 011 -> 0x23
            li_vn_mode: 0x23,
            stratum: Stratum::UnspecifiedOrInvalid,
            poll: 0,
            precision: 0,
            root_delay: [0; 4],
            root_dispersion: [0; 4],
            reference_id: [0; 4],
            reference_timestamp: NtpTimestamp::default(),
            origin_timestamp: NtpTimestamp::default(),
            receive_timestamp: NtpTimestamp::default(),
            transmit_timestamp: NtpTimestamp::default(),
        }
    }

    /// Serializes the structured properties into a raw 48-byte buffer.
    pub fn serialize(&self) -> [u8; 48] {
        let mut buf = [0u8; 48];
        buf[0] = self.li_vn_mode;
        buf[1] = match self.stratum {
            Stratum::UnspecifiedOrInvalid => 0,
            Stratum::PrimaryReference => 1,
            Stratum::SecondaryReference(v) => v,
            Stratum::Reserved(v) => v,
        };
        buf[2] = self.poll as u8;
        buf[3] = self.precision as u8;
        buf[4..8].copy_from_slice(&self.root_delay);
        buf[8..12].copy_from_slice(&self.root_dispersion);
        buf[12..16].copy_from_slice(&self.reference_id);
        buf[16..24].copy_from_slice(&self.reference_timestamp.to_be_bytes());
        buf[24..32].copy_from_slice(&self.origin_timestamp.to_be_bytes());
        buf[32..40].copy_from_slice(&self.receive_timestamp.to_be_bytes());
        buf[40..48].copy_from_slice(&self.transmit_timestamp.to_be_bytes());
        buf
    }

    pub fn serialize_to(&self, out: &mut [u8]) -> usize {
        let res = self.serialize();
        let len = out.len().min(res.len());
        out[..len].copy_from_slice(&res[..len]);
        len
    }

    /// Safely parses a raw 48-byte network buffer into an `NtpPacket`.
    pub fn deserialize(buf: &[u8]) -> Self {
        if buf.len() < 48 { return Self::empty(); }
        Self {
            li_vn_mode: buf[0],
            stratum: Stratum::from(buf[1]),
            poll: buf[2] as i8 as i16,
            precision: buf[3] as i8 as i16,
            root_delay: [buf[4], buf[5], buf[6], buf[7]],
            root_dispersion: [buf[8], buf[9], buf[10], buf[11]],
            reference_id: [buf[12], buf[13], buf[14], buf[15]],
            reference_timestamp: NtpTimestamp::from_be_bytes([
                buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23],
            ]),
            origin_timestamp: NtpTimestamp::from_be_bytes([
                buf[24], buf[25], buf[26], buf[27], buf[28], buf[29], buf[30], buf[31],
            ]),
            receive_timestamp: NtpTimestamp::from_be_bytes([
                buf[32], buf[33], buf[34], buf[35], buf[36], buf[37], buf[38], buf[39],
            ]),
            transmit_timestamp: NtpTimestamp::from_be_bytes([
                buf[40], buf[41], buf[42], buf[43], buf[44], buf[45], buf[46], buf[47],
            ]),
        }
    }

    pub fn mode(&self) -> u8 {
        self.li_vn_mode & 0x07
    }
}

/// Trait abstraction allowing custom `std`-like UDP network sockets to send data.
pub trait NtpUdpSend {
    type Error;
    fn send_to(&self, buf: &[u8], addr: (Ipv4Addr, u16)) -> Result<usize, Self::Error>;
}

/// Trait abstraction allowing custom `std`-like UDP network sockets to receive data.
pub trait NtpUdpRecv {
    type Error;
    fn recv_from(&self, buf: &mut [u8]) -> Result<usize, Self::Error>;
}

/// Output structure representing unified system-ready time measurements.
#[derive(Debug, Clone, Copy)]
pub struct NtpResult {
    /// Seconds passed since standard Unix Epoch (January 1, 1970)
    pub unix_seconds: u64,
    /// Remainder sub-second fragment expressed directly in milliseconds
    pub milliseconds: u32,
    /// Intact raw high-precision timestamp parsed directly from the server
    pub server_time: NtpTimestamp,
}

/// Driver execution errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtpDriverError<E> {
    SocketSend(E),
    SocketRecv(E),
    IncompleteNetworkWrite,
    IncompleteNetworkRead,
    InvalidPacketMode,
}

impl<E> Display for NtpDriverError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let er = match self {
            Self::SocketSend(_) => "socketsend",
            Self::SocketRecv(_) => "socketrecv",
            Self::IncompleteNetworkWrite => "networkwrite",
            Self::IncompleteNetworkRead => "networkread",
            Self::InvalidPacketMode => "invalid mode"
        };
        write!(f, "{}", er)
    }
}

pub struct NtpClient;

impl NtpClient {
    pub fn query_time<S, E>(socket: &S, server_address: (Ipv4Addr, u16)) -> Result<NtpResult, NtpDriverError<E>>
    where
        S: NtpUdpRecv<Error = E> + NtpUdpSend<Error = E>,
    {
        let request = NtpPacket::new_client_request();
        let payload = request.serialize();
        let bytes_sent = socket.send_to(&payload, server_address).map_err(NtpDriverError::SocketSend)?;
        if bytes_sent != payload.len() { return Err(NtpDriverError::IncompleteNetworkWrite); }
        let mut response_buf = [0u8; 48];
        let bytes_received = socket.recv_from(&mut response_buf).map_err(NtpDriverError::SocketRecv)?;
        if bytes_received < 48 { return Err(NtpDriverError::IncompleteNetworkRead); }
        let response = NtpPacket::deserialize(&response_buf);
        if response.mode() != 4 { return Err(NtpDriverError::InvalidPacketMode); }
        let tx_time = response.transmit_timestamp;
        Ok(NtpResult {
            unix_seconds: tx_time.to_unix_seconds(),
            milliseconds: tx_time.to_milliseconds(),
            server_time: tx_time,
        })
    }
}
